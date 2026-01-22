//! Benchmark for the Confidence OpenFeature provider.
//!
//! This benchmark is similar to the Go and JS benchmarks in the same repository.
//!
//! ## Running
//!
//! ```bash
//! # Against mock server (default)
//! cargo run --release --example bench -- --mock-addr localhost:8081
//!
//! # With custom duration and threads
//! cargo run --release --example bench -- --duration 60 --threads 4
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use open_feature::{EvaluationContext, OpenFeature, StructValue};
use spotify_confidence_openfeature_provider_local::{ConfidenceProvider, ProviderOptions};
use tokio::signal;
use tokio::sync::broadcast;

// Default configuration
const DEFAULT_MOCK_ADDR: &str = "http://localhost:8081";
const DEFAULT_DURATION_SECS: u64 = 30;
const DEFAULT_WARMUP_SECS: u64 = 5;
const DEFAULT_FLAG_KEY: &str = "web-sdk-e2e-flag";
const DEFAULT_CLIENT_SECRET: &str = "ti5Sipq5EluCYRG7I5cdbpWC3xq7JTWv";

struct Args {
    mock_addr: String,
    duration_secs: u64,
    warmup_secs: u64,
    threads: usize,
    flag_key: String,
    client_secret: String,
}

impl Args {
    fn parse() -> Self {
        let mut args = Args {
            mock_addr: DEFAULT_MOCK_ADDR.to_string(),
            duration_secs: DEFAULT_DURATION_SECS,
            warmup_secs: DEFAULT_WARMUP_SECS,
            threads: num_cpus::get(),
            flag_key: DEFAULT_FLAG_KEY.to_string(),
            client_secret: DEFAULT_CLIENT_SECRET.to_string(),
        };

        let argv: Vec<String> = std::env::args().collect();
        let mut i = 1;
        while i < argv.len() {
            match argv[i].as_str() {
                "--mock-addr" | "-mock-addr" => {
                    i += 1;
                    if i < argv.len() {
                        args.mock_addr = argv[i].clone();
                        // Ensure it has a scheme
                        if !args.mock_addr.starts_with("http://")
                            && !args.mock_addr.starts_with("https://")
                        {
                            args.mock_addr = format!("http://{}", args.mock_addr);
                        }
                    }
                }
                "--duration" | "-duration" => {
                    i += 1;
                    if i < argv.len() {
                        args.duration_secs = argv[i].parse().unwrap_or(DEFAULT_DURATION_SECS);
                    }
                }
                "--warmup" | "-warmup" => {
                    i += 1;
                    if i < argv.len() {
                        args.warmup_secs = argv[i].parse().unwrap_or(DEFAULT_WARMUP_SECS);
                    }
                }
                "--threads" | "-threads" => {
                    i += 1;
                    if i < argv.len() {
                        args.threads = argv[i].parse().unwrap_or(num_cpus::get());
                    }
                }
                "--flag" | "-flag" => {
                    i += 1;
                    if i < argv.len() {
                        args.flag_key = argv[i].clone();
                    }
                }
                "--client-secret" | "-client-secret" => {
                    i += 1;
                    if i < argv.len() {
                        args.client_secret = argv[i].clone();
                    }
                }
                "--help" | "-h" => {
                    eprintln!(
                        "Usage: bench [OPTIONS]

Options:
  --mock-addr <ADDR>       Mock server address (default: {})
  --duration <SECS>        Benchmark duration in seconds (default: {})
  --warmup <SECS>          Warmup duration in seconds (default: {})
  --threads <N>            Number of concurrent tasks (default: num_cpus)
  --flag <KEY>             Flag key to evaluate (default: {})
  --client-secret <SECRET> Client secret (default: {})
  --help                   Show this help message",
                        DEFAULT_MOCK_ADDR,
                        DEFAULT_DURATION_SECS,
                        DEFAULT_WARMUP_SECS,
                        DEFAULT_FLAG_KEY,
                        DEFAULT_CLIENT_SECRET
                    );
                    std::process::exit(0);
                }
                _ => {}
            }
            i += 1;
        }

        args
    }
}

struct Stats {
    completed: AtomicU64,
    errors: AtomicU64,
}

impl Stats {
    fn new() -> Self {
        Self {
            completed: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        }
    }

    fn reset(&self) {
        self.completed.store(0, Ordering::SeqCst);
        self.errors.store(0, Ordering::SeqCst);
    }
}

async fn run_worker(
    client: Arc<open_feature::Client>,
    flag_key: String,
    context: EvaluationContext,
    stats: Arc<Stats>,
    mut shutdown_rx: broadcast::Receiver<()>,
    abort_on_error: bool,
) -> bool {
    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.recv() => {
                return true;
            }
            result = client.get_struct_details::<StructValue>(&flag_key, Some(&context), None) => {
                match result {
                    Ok(details) => {
                        stats.completed.fetch_add(1, Ordering::Relaxed);
                        if details.reason == Some(open_feature::EvaluationReason::Error) {
                            stats.errors.fetch_add(1, Ordering::Relaxed);
                            if abort_on_error {
                                eprintln!("Error during evaluation: {:?}", details);
                                return false;
                            }
                        }
                    }
                    Err(e) => {
                        stats.errors.fetch_add(1, Ordering::Relaxed);
                        if abort_on_error {
                            eprintln!("Error during evaluation: {:?}", e);
                            return false;
                        }
                    }
                }
            }
        }
    }
}

async fn run_workers(
    client: Arc<open_feature::Client>,
    flag_key: &str,
    context: &EvaluationContext,
    num_threads: usize,
    stats: Arc<Stats>,
    duration: Duration,
    abort_on_error: bool,
) -> bool {
    let (shutdown_tx, _) = broadcast::channel::<()>(1);

    let mut handles = Vec::with_capacity(num_threads);
    for _ in 0..num_threads {
        let client = Arc::clone(&client);
        let flag_key = flag_key.to_string();
        let context = context.clone();
        let stats = Arc::clone(&stats);
        let shutdown_rx = shutdown_tx.subscribe();

        handles.push(tokio::spawn(async move {
            run_worker(
                client,
                flag_key,
                context,
                stats,
                shutdown_rx,
                abort_on_error,
            )
            .await
        }));
    }

    // Wait for duration or SIGINT/SIGTERM
    tokio::select! {
        _ = tokio::time::sleep(duration) => {}
        _ = signal::ctrl_c() => {
            eprintln!("\nReceived interrupt signal, stopping...");
        }
    }

    // Signal all workers to stop
    drop(shutdown_tx);

    // Wait for all workers
    let mut success = true;
    for handle in handles {
        match handle.await {
            Ok(worker_success) => {
                if !worker_success {
                    success = false;
                }
            }
            Err(e) => {
                eprintln!("Worker task panicked: {:?}", e);
                success = false;
            }
        }
    }

    success
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing for debug output
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    // Create provider options with gateway URL pointing to mock server
    let options = ProviderOptions::new(&args.client_secret).with_gateway_url(&args.mock_addr);

    // Create the Confidence provider
    let provider = ConfidenceProvider::new(options)?;

    // Set the provider on the OpenFeature singleton
    OpenFeature::singleton_mut()
        .await
        .set_provider(provider)
        .await;

    // Create an OpenFeature client
    let client = Arc::new(OpenFeature::singleton().await.create_client());

    // Create evaluation context
    let context = EvaluationContext::default()
        .with_targeting_key("tutorial_visitor")
        .with_custom_field("visitor_id", "tutorial_visitor")
        .with_custom_field("sticky", false);

    let stats = Arc::new(Stats::new());

    // Warmup phase
    if args.warmup_secs > 0 {
        eprintln!(
            "Warming up for {} seconds with {} threads...",
            args.warmup_secs, args.threads
        );
        let success = run_workers(
            Arc::clone(&client),
            &args.flag_key,
            &context,
            args.threads,
            Arc::clone(&stats),
            Duration::from_secs(args.warmup_secs),
            true, // abort on error during warmup
        )
        .await;

        if !success || stats.errors.load(Ordering::SeqCst) > 0 {
            eprintln!("Aborting: error during warmup");
            std::process::exit(1);
        }

        stats.reset();
    }

    // Measurement phase
    eprintln!(
        "Starting measurement for {} seconds with {} threads...",
        args.duration_secs, args.threads
    );

    let start = Instant::now();
    run_workers(
        Arc::clone(&client),
        &args.flag_key,
        &context,
        args.threads,
        Arc::clone(&stats),
        Duration::from_secs(args.duration_secs),
        true, // abort on error
    )
    .await;
    let elapsed = start.elapsed();

    let completed = stats.completed.load(Ordering::SeqCst);
    let errors = stats.errors.load(Ordering::SeqCst);
    let qps = completed as f64 / elapsed.as_secs_f64();

    println!(
        "flag={} threads={} duration={}ms ops={} errors={} throughput={:.0} ops/s",
        args.flag_key,
        args.threads,
        elapsed.as_millis(),
        completed,
        errors,
        qps
    );

    Ok(())
}
