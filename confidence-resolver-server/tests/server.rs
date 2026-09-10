use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use confidence_resolver::proto::{
    confidence::flags::admin::v1::ResolverState as StateProto, google::Timestamp,
};
use confidence_resolver_server::{
    api, backend::Backend, config::Config, grpc, state::StateFetcher, transport, ResolverService,
};
use http_body_util::BodyExt;
use prost::Message;
use serde_json::json;
use std::{collections::BTreeSet, sync::Arc};
use tower::ServiceExt;
use wiremock::{
    matchers::{header, method, path},
    Mock, MockServer, ResponseTemplate,
};

fn state() -> StateProto {
    serde_json::from_str(include_str!("fixtures/account.json")).unwrap()
}
fn config(server: &MockServer, apply: bool) -> Config {
    Config::from_lookup(|name| match name {
        "CONFIDENCE_RESOLVER_STATE_URL" => Some(format!("{}/state", server.uri())),
        "CONFIDENCE_RESOLVER_API_URL" => Some(server.uri()),
        "CONFIDENCE_ACCOUNT" => Some("accounts/test".into()),
        "CONFIDENCE_SEND_APPLY_LOGS" => Some(apply.to_string()),
        _ => None,
    })
    .unwrap()
}
async fn app(
    server: &MockServer,
    apply: bool,
) -> (Arc<ResolverService>, Arc<Backend>, StateFetcher) {
    let config = config(server, apply);
    let backend = Backend::new(&config).unwrap();
    let app = Arc::new(ResolverService::new(backend.clone(), apply, 1024 * 1024));
    let fetcher = StateFetcher::new(&config, backend.clone());
    (app, backend, fetcher)
}
fn request(secret: &str, apply: bool) -> api::ResolveFlagsRequest {
    serde_json::from_value(
        json!({"clientSecret":secret, "evaluationContext":{"targeting_key":"user-1"},
        "apply":apply, "sdk":{"customId":secret, "version":"test"}}),
    )
    .unwrap()
}
async fn serve_state(server: &MockServer, bytes: Vec<u8>) {
    Mock::given(method("GET"))
        .and(path("/state"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "v1")
                .set_body_bytes(bytes),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn full_account_routes_isolate_clients_and_keep_last_good_state() {
    let server = MockServer::start().await;
    let (app, _, mut fetcher) = app(&server, true).await;
    let router = transport::router(app.clone());
    let health = router
        .clone()
        .oneshot(Request::get("/v1/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::SERVICE_UNAVAILABLE);
    serve_state(&server, state().encode_to_vec()).await;
    fetcher.reload(&app).await.unwrap();
    for (secret, expected) in [
        ("test-a", "flags/a"),
        ("test-ab", "flags/ab"),
        ("test-a", "flags/a"),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::post("/v1/flags:resolve")
                    .body(Body::from(
                        serde_json::to_vec(&request(secret, true)).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response: api::ResolveFlagsResponse =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(
            response
                .resolved_flags
                .iter()
                .map(|f| f.flag.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([expected, "flags/shared"])
        );
        assert!(response
            .resolved_flags
            .iter()
            .all(|f| f.reason() == api::ResolveReason::Match));
    }
    let invalid = app
        .resolve(request("not-a-client", true))
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(invalid, "Not authenticated");
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/state"))
        .and(header("if-none-match", "v1"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    assert!(!fetcher.reload(&app).await.unwrap());
    server.reset().await;
    serve_state(&server, vec![0xff]).await;
    assert!(fetcher.reload(&app).await.is_err());
    assert!(app.ready());
    assert!(app.resolve(request("test-a", true)).await.is_ok());
    server.reset().await;
    let mut revoked = state();
    revoked
        .client_credentials
        .retain(|c| c.name.starts_with("clients/ab/"));
    serve_state(&server, revoked.encode_to_vec()).await;
    fetcher.reload(&app).await.unwrap();
    assert!(app.resolve(request("test-a", true)).await.is_err());
    assert!(app.resolve(request("test-ab", true)).await.is_ok());
}

#[tokio::test]
async fn applies_preserve_request_sdk_and_forwarding_toggle_with_retry() {
    for enabled in [true, false] {
        let server = MockServer::start().await;
        let (app, backend, mut fetcher) = app(&server, enabled).await;
        serve_state(&server, state().encode_to_vec()).await;
        fetcher.reload(&app).await.unwrap();
        let response = app.resolve(request("test-a", false)).await.unwrap();
        assert!(!response.resolve_token.is_empty());
        let now = Timestamp {
            seconds: 1_700_000_000,
            nanos: 0,
        };
        let apply = api::ApplyFlagsRequest {
            client_secret: "test-a".into(),
            resolve_token: response.resolve_token.clone(),
            send_time: Some(now),
            flags: vec![api::AppliedFlag {
                flag: "flags/a".into(),
                apply_time: Some(now),
            }],
            sdk: request("apply-sdk", false).sdk,
        };
        let mut wrong_client = apply.clone();
        wrong_client.client_secret = "test-ab".into();
        assert!(app.apply(wrong_client).await.is_err());
        let mut corrupt = apply.clone();
        corrupt.resolve_token[15] ^= 1;
        assert!(app.apply(corrupt).await.is_err());
        // A snapshot refresh must not invalidate outstanding tokens.
        fetcher.reload(&app).await.unwrap();
        app.apply(apply).await.unwrap();
        // Default 404 causes retry retention; a later successful request releases the reservation.
        app.reporting.flush(&backend).await;
        assert!(app.reporting.pending_bytes() > 0);
        Mock::given(method("POST"))
            .and(path("/v1/clientFlagLogs:write"))
            .and(header("authorization", "ClientSecret test-a"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        app.reporting.flush(&backend).await;
        assert_eq!(app.reporting.pending_bytes(), 0);
        let requests = server.received_requests().await.unwrap();
        let log_request = requests
            .iter()
            .rev()
            .find(|r| r.url.path() == "/v1/clientFlagLogs:write")
            .unwrap();
        let logs = api::WriteFlagLogsRequest::decode(log_request.body.as_slice()).unwrap();
        assert!(!logs.client_resolve_info.is_empty());
        assert_eq!(logs.flag_assigned.len(), usize::from(enabled));
        if enabled {
            assert_eq!(
                logs.flag_assigned[0].client_info.as_ref().unwrap().sdk,
                request("apply-sdk", false).sdk
            );
        }
    }
}

#[tokio::test]
async fn grpc_and_http_share_the_same_native_service() {
    let server = MockServer::start().await;
    let (app, _, mut fetcher) = app(&server, false).await;
    serve_state(&server, state().encode_to_vec()).await;
    fetcher.reload(&app).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(
                grpc::flag_resolver_service_server::FlagResolverServiceServer::new(
                    transport::GrpcResolver(app.clone()),
                ),
            )
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async {
                    let _ = stopped.await;
                },
            ),
    );
    let mut client = grpc::flag_resolver_service_client::FlagResolverServiceClient::connect(
        format!("http://{address}"),
    )
    .await
    .unwrap();
    assert_eq!(
        client
            .resolve_flags(request("test-ab", true))
            .await
            .unwrap()
            .into_inner()
            .resolved_flags
            .len(),
        2
    );
    assert_eq!(
        client
            .resolve_flags(request("bad", true))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert!(app
        .metrics()
        .contains("confidence_server_resolve_requests_total 2"));
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn remote_materializations_use_each_requests_client_and_write_assignments() {
    let server = MockServer::start().await;
    let (app, _, mut fetcher) = app(&server, false).await;
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/account.json")).unwrap();
    value["flags"][2]["rules"][0]["materializationSpec"] = json!({
        "readMaterialization":"materializations/sticky", "writeMaterialization":"materializations/sticky"});
    let state: StateProto = serde_json::from_value(value).unwrap();
    serve_state(&server, state.encode_to_vec()).await;
    fetcher.reload(&app).await.unwrap();
    for secret in ["test-a", "test-ab"] {
        Mock::given(method("POST"))
            .and(path("/v1/materialization:readMaterializedOperations"))
            .and(header("authorization", format!("ClientSecret {secret}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(Vec::<u8>::new()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/materialization:writeMaterializedOperations"))
            .and(header("authorization", format!("ClientSecret {secret}")))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
    }
    let (a, b) = tokio::join!(
        app.resolve(request("test-a", false)),
        app.resolve(request("test-ab", false))
    );
    assert_eq!(a.unwrap().resolved_flags.len(), 2);
    assert_eq!(b.unwrap().resolved_flags.len(), 2);
    server.verify().await;
    let calls = server.received_requests().await.unwrap();
    for write in calls
        .iter()
        .filter(|r| r.url.path().ends_with(":writeMaterializedOperations"))
    {
        #[derive(Clone, PartialEq, Message)]
        struct Writes {
            #[prost(message, repeated, tag = "1")]
            records: Vec<api::MaterializationRecord>,
        }
        let write = Writes::decode(write.body.as_slice()).unwrap();
        assert_eq!(write.records.len(), 1);
        assert_eq!(write.records[0].unit, "user-1");
        assert_eq!(write.records[0].materialization, "materializations/sticky");
        assert_eq!(write.records[0].variant, "on");
    }
}

#[tokio::test]
async fn reporting_is_bounded_when_backend_is_down() {
    let server = MockServer::start().await;
    let config = config(&server, true);
    let backend = Backend::new(&config).unwrap();
    let app = ResolverService::new(backend.clone(), true, 2048);
    let mut fetcher = StateFetcher::new(&config, backend.clone());
    serve_state(&server, state().encode_to_vec()).await;
    fetcher.reload(&app).await.unwrap();
    for _ in 0..30 {
        app.resolve(request("test-a", true)).await.unwrap();
    }
    assert!(app.reporting.pending_bytes() <= 2048);
    assert!(
        app.reporting
            .dropped
            .load(std::sync::atomic::Ordering::Relaxed)
            > 0
    );
    app.reporting.flush(&backend).await;
    assert!(app.reporting.pending_bytes() <= 2048);
}
