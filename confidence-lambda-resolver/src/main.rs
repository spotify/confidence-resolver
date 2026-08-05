mod common;
mod consumer;
mod materialization;
mod resolver;

use tracing_subscriber::EnvFilter;

use common::env_var_opt;

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .with_target(false)
        .without_time()
        .init();

    let mode = env_var_opt("HANDLER_MODE").unwrap_or_else(|| "resolver".to_string());

    match mode.as_str() {
        "consumer" => consumer::run().await,
        _ => resolver::run().await,
    }
}
