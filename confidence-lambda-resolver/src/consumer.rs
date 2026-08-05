#[allow(dead_code)]
mod common;
#[allow(dead_code)]
mod materialization;

use lambda_runtime::{service_fn, Error, LambdaEvent};
use serde::Deserialize;
use tracing_subscriber::EnvFilter;

use confidence_resolver::flag_logger;
use confidence_resolver::proto::confidence::flags::resolver::v1::WriteFlagLogsRequest;

use common::{env_var_opt, send_flags_logs, update_prometheus_dynamo, CONFIDENCE_CLIENT_SECRET};

#[derive(Deserialize)]
struct SqsEvent {
    #[serde(rename = "Records")]
    records: Vec<SqsRecord>,
}

#[derive(Deserialize)]
struct SqsRecord {
    body: String,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .with_target(false)
        .without_time()
        .init();

    if let Some(secret) = env_var_opt("CONFIDENCE_CLIENT_SECRET") {
        let _ = CONFIDENCE_CLIENT_SECRET.set(secret);
    }

    lambda_runtime::run(service_fn(handler)).await
}

async fn handler(event: LambdaEvent<SqsEvent>) -> Result<(), Error> {
    let (sqs_event, _context) = event.into_parts();

    let logs: Vec<WriteFlagLogsRequest> = sqs_event
        .records
        .iter()
        .filter_map(|r| serde_json::from_str::<WriteFlagLogsRequest>(&r.body).ok())
        .collect();

    if logs.is_empty() {
        return Ok(());
    }

    let req = flag_logger::aggregate_batch(logs);

    if let Some(table) = env_var_opt("DYNAMODB_METRICS_TABLE") {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let dynamo = aws_sdk_dynamodb::Client::new(&config);
        update_prometheus_dynamo(&dynamo, &table, &req).await;
    }

    if let Some(secret) = CONFIDENCE_CLIENT_SECRET.get() {
        if let Err(e) = send_flags_logs(secret, &req).await {
            tracing::error!("Failed to send flag logs: {}", e);
        }
    }

    Ok(())
}
