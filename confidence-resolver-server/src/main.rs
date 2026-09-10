use confidence_resolver::Host;
use confidence_resolver_server::{
    backend::{Backend, Metadata},
    config::Config,
    grpc,
    service::ResolverService,
    state::StateFetcher,
    transport,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{net::TcpListener, sync::watch, task::JoinSet};
use tokio_stream::wrappers::TcpListenerStream;
use tonic_health::ServingStatus;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--health-check") {
        let port = std::env::var("CONFIDENCE_RESOLVER_HTTP_PORT")
            .unwrap_or_else(|_| "8090".into())
            .parse::<u16>()?;
        reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/v1/health"))
            .timeout(Duration::from_secs(2))
            .send()
            .await?
            .error_for_status()?;
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "confidence_resolver_server=info".into()),
        )
        .init();
    let config = Config::from_env()?;
    let backend = Backend::new(&config)?;
    let service = Arc::new(ResolverService::new(
        backend.clone(),
        config.send_apply_logs,
        config.log_capacity,
    ));
    let http_listener = TcpListener::bind(config.http_address).await?;
    let grpc_listener = TcpListener::bind(config.grpc_address).await?;
    let (stop, stopped) = watch::channel(false);
    let mut tasks: JoinSet<Result<(), String>> = JoinSet::new();
    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    let service_name = "confidence.flags.resolver.v1.FlagResolverService";
    health_reporter
        .set_service_status("", ServingStatus::NotServing)
        .await;
    health_reporter
        .set_service_status(service_name, ServingStatus::NotServing)
        .await;

    let router = transport::router(service.clone());
    let shutdown = stopped.clone();
    tasks.spawn(async move {
        axum::serve(http_listener, router)
            .with_graceful_shutdown(wait_for_stop(shutdown))
            .await
            .map_err(|_| "HTTP server stopped unexpectedly".into())
    });
    let reflection = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(grpc::DESCRIPTOR)
        .register_encoded_file_descriptor_set(tonic_health::pb::FILE_DESCRIPTOR_SET)
        .build_v1()?;
    let reflection_alpha = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(grpc::DESCRIPTOR)
        .register_encoded_file_descriptor_set(tonic_health::pb::FILE_DESCRIPTOR_SET)
        .build_v1alpha()?;
    let rpc = grpc::flag_resolver_service_server::FlagResolverServiceServer::new(
        transport::GrpcResolver(service.clone()),
    );
    let shutdown = stopped.clone();
    tasks.spawn(async move {
        tonic::transport::Server::builder()
            .add_service(health_service)
            .add_service(reflection)
            .add_service(reflection_alpha)
            .add_service(rpc)
            .serve_with_incoming_shutdown(
                TcpListenerStream::new(grpc_listener),
                wait_for_stop(shutdown),
            )
            .await
            .map_err(|_| "gRPC server stopped unexpectedly".into())
    });

    let mut fetcher = StateFetcher::new(&config, backend.clone());
    let app = service.clone();
    let mut shutdown = stopped.clone();
    tasks.spawn(async move {
        let mut ticks = tokio::time::interval(config.poll_interval);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = ticks.tick() => {
                    match fetcher.reload(&app).await {
                        Ok(true) => {
                            health_reporter.set_service_status("", ServingStatus::Serving).await;
                            health_reporter.set_service_status(service_name, ServingStatus::Serving).await;
                            tracing::info!("Loaded account state");
                        }
                        Ok(false) => {},
                        Err(error) => tracing::warn!(%error, "State refresh failed; retaining last good state"),
                    }
                }
            }
        }
        health_reporter.set_service_status("", ServingStatus::NotServing).await;
        health_reporter.set_service_status(service_name, ServingStatus::NotServing).await;
        Ok(())
    });
    let app = service.clone();
    let target = backend.clone();
    let mut shutdown = stopped.clone();
    tasks.spawn(async move {
        let mut ticks = tokio::time::interval(config.log_interval);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! { _ = shutdown.changed() => break, _ = ticks.tick() => app.reporting.flush(&target).await }
        }
        Ok(())
    });
    let app = service.clone();
    let target = backend.clone();
    let mut shutdown = stopped.clone();
    tasks.spawn(async move {
        let mut ticks = tokio::time::interval(config.metadata_interval);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut previous = app.request_counts();
        let mut last = Instant::now();
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = ticks.tick() => {
                    let current = app.request_counts();
                    let seconds = last.elapsed().as_secs_f64().max(1.0);
                    let metadata = Metadata {
                        server_id: std::env::var("HOSTNAME").unwrap_or_else(|_| "resolver".into()),
                        sidecar_version: env!("CARGO_PKG_VERSION").into(),
                        rps_resolves: current.0.saturating_sub(previous.0) as f64 / seconds,
                        rps_applies: current.1.saturating_sub(previous.1) as f64 / seconds,
                        measurement_time: Some(SystemHost::current_time()), ..Default::default()
                    };
                    if target.send_metadata(metadata).await.is_err() { tracing::warn!("Metadata delivery failed"); }
                    previous = current; last = Instant::now();
                }
            }
        }
        Ok(())
    });
    tracing::info!(http = %config.http_address, grpc = %config.grpc_address, "Resolver started");
    let failure = tokio::select! {
        result = signal() => result.err().map(|_| "Signal handler failed".to_string()),
        result = tasks.join_next() => Some(format!("Server task exited: {result:?}")),
    };
    stop.send_replace(true);
    // Stop accepting work, finish in-flight requests, then flush their reports.
    let drained = tokio::time::timeout(Duration::from_secs(30), async {
        while tasks.join_next().await.is_some() {}
        service.reporting.flush(&backend).await;
    })
    .await;
    if drained.is_err() {
        tasks.abort_all();
        tracing::warn!("Shutdown drain deadline reached");
    }
    if let Some(error) = failure {
        return Err(error.into());
    }
    Ok(())
}
async fn wait_for_stop(mut stopped: watch::Receiver<bool>) {
    if !*stopped.borrow_and_update() {
        let _ = stopped.changed().await;
    }
}
async fn signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { result = tokio::signal::ctrl_c() => result?, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}
struct SystemHost;
impl Host for SystemHost {
    fn log_resolve(
        _: &str,
        _: &confidence_resolver::proto::google::Struct,
        _: &[confidence_resolver::ResolvedValue<'_>],
        _: &confidence_resolver::Client,
    ) {
    }
    fn log_assign(
        _: &str,
        _: &[confidence_resolver::FlagToApply<'_>],
        _: &confidence_resolver::Client,
        _: &Option<confidence_resolver_server::api::Sdk>,
    ) {
    }
}
