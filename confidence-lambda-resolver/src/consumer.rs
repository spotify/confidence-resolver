use std::sync::OnceLock;

use lambda_runtime::{service_fn, LambdaEvent};
use serde::Deserialize;

use confidence_resolver::flag_logger;
use confidence_resolver::proto::confidence::flags::resolver::v1::WriteFlagLogsRequest;

use crate::common::{
    env_var_opt, send_flags_logs, update_prometheus_dynamo, CONFIDENCE_CLIENT_SECRET,
};

static DYNAMO_CLIENT: OnceLock<aws_sdk_dynamodb::Client> = OnceLock::new();
static METRICS_TABLE: OnceLock<String> = OnceLock::new();

#[derive(Deserialize)]
struct SqsEvent {
    #[serde(rename = "Records")]
    records: Vec<SqsRecord>,
}

#[derive(Deserialize)]
struct SqsRecord {
    body: String,
}

pub async fn run() -> Result<(), lambda_runtime::Error> {
    if let Some(secret) = env_var_opt("CONFIDENCE_CLIENT_SECRET") {
        let _ = CONFIDENCE_CLIENT_SECRET.set(secret);
    }

    if let Some(table) = env_var_opt("DYNAMODB_METRICS_TABLE") {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let _ = DYNAMO_CLIENT.set(aws_sdk_dynamodb::Client::new(&config));
        let _ = METRICS_TABLE.set(table);
    }

    lambda_runtime::run(service_fn(handler)).await
}

async fn handler(event: LambdaEvent<SqsEvent>) -> Result<(), lambda_runtime::Error> {
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

    if let (Some(client), Some(table)) = (DYNAMO_CLIENT.get(), METRICS_TABLE.get()) {
        update_prometheus_dynamo(client, table, &req).await;
    }

    if let Some(secret) = CONFIDENCE_CLIENT_SECRET.get() {
        if let Err(e) = send_flags_logs(secret, &req).await {
            tracing::error!("Failed to send flag logs: {}", e);
        }
    }

    Ok(())
}
