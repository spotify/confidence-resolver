use std::cell::RefCell;
use std::sync::{LazyLock, OnceLock};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::Bytes;
use confidence_resolver::proto::confidence::flags::resolver::v1::{Sdk, WriteFlagLogsRequest};
use confidence_resolver::proto::google::Struct;
use confidence_resolver::telemetry::TelemetrySnapshot;
use confidence_resolver::{
    assign_logger, resolve_logger, Client, FlagToApply, Host, ResolvedValue, ResolverState,
};
use prost::Message;

#[derive(Clone, PartialEq, Message)]
pub struct SetResolverStateRequest {
    #[prost(bytes = "bytes", tag = "1")]
    pub state: Bytes,
    #[prost(string, tag = "2")]
    pub account_id: String,
}

const CDN_STATE_BYTES: &[u8] = include_bytes!("../../data/resolver_state_current.pb");
const ENCRYPTION_KEY_BASE64: &str = include_str!("../../data/encryption_key");

static CDN_STATE_REQUEST: LazyLock<SetResolverStateRequest> = LazyLock::new(|| {
    SetResolverStateRequest::decode(Bytes::from_static(CDN_STATE_BYTES))
        .expect("Failed to decode SetResolverStateRequest from CDN state")
});

pub static RESOLVER_STATE: LazyLock<ResolverState> = LazyLock::new(|| {
    let cdn_request = &*CDN_STATE_REQUEST;
    ResolverState::from_proto(
        cdn_request.state.to_vec().try_into().unwrap(),
        &cdn_request.account_id,
        None,
    )
    .unwrap()
});

pub static ENCRYPTION_KEY: LazyLock<Bytes> = LazyLock::new(|| {
    let trimmed = ENCRYPTION_KEY_BASE64.trim();
    if trimmed.is_empty() {
        Bytes::from_static(&[0u8; 16])
    } else {
        Bytes::from(STANDARD.decode(trimmed).expect("Invalid base64 encryption key"))
    }
});

pub static CONFIDENCE_CLIENT_SECRET: OnceLock<String> = OnceLock::new();

thread_local! {
    pub static FLAG_LOG: RefCell<Option<WriteFlagLogsRequest>> = const { RefCell::new(None) };
}

pub const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

pub struct LambdaHost;

impl Host for LambdaHost {
    fn log(message: &str) {
        tracing::debug!("{}", message);
    }

    fn log_resolve(
        _resolve_id: &str,
        evaluation_context: &Struct,
        values: &[ResolvedValue<'_>],
        client: &Client,
    ) {
        FLAG_LOG.with(|f| {
            if let Some(req) = f.borrow_mut().as_mut() {
                let (flag_infos, client_info) = resolve_logger::build_resolve_log(
                    evaluation_context,
                    client.client_credential_name.as_str(),
                    values,
                );
                req.flag_resolve_info.extend(flag_infos);
                req.client_resolve_info.push(client_info);
            }
        });
    }

    fn log_assign(
        resolve_id: &str,
        assigned_flags: &[FlagToApply],
        client: &Client,
        sdk: &Option<Sdk>,
    ) {
        FLAG_LOG.with(|f| {
            if let Some(req) = f.borrow_mut().as_mut() {
                req.flag_assigned.push(assign_logger::build_flag_assigned(
                    resolve_id,
                    assigned_flags,
                    client,
                    sdk,
                ));
            }
        });
    }
}

pub fn sdk_info() -> Sdk {
    Sdk {
        sdk: Some(
            confidence_resolver::proto::confidence::flags::resolver::v1::sdk::Sdk::Id(
                confidence_resolver::proto::confidence::flags::resolver::v1::SdkId::CloudflareResolver as i32,
            ),
        ),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

pub fn env_var(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

pub fn env_var_opt(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

pub async fn send_flags_logs(
    client_secret: &str,
    message: &WriteFlagLogsRequest,
) -> Result<(), String> {
    let client = reqwest::Client::new();
    let json = serde_json::to_string(message).map_err(|e| e.to_string())?;
    let response = client
        .post("https://resolver.confidence.dev/v1/clientFlagLogs:write")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("ClientSecret {}", client_secret))
        .body(json)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !response.status().is_success() {
        return Err(format!(
            "Failed to send flag logs: {}",
            response.status()
        ));
    }
    Ok(())
}

pub fn add_cors_headers(
    builder: http::response::Builder,
    allowed_origin: &str,
) -> http::response::Builder {
    builder
        .header("Access-Control-Allow-Origin", allowed_origin)
        .header("Access-Control-Allow-Methods", "POST, GET, OPTIONS")
        .header("Access-Control-Allow-Headers", "*")
}

pub async fn update_prometheus_dynamo(
    client: &aws_sdk_dynamodb::Client,
    table_name: &str,
    req: &WriteFlagLogsRequest,
) {
    use aws_sdk_dynamodb::types::AttributeValue;

    let mut cumulative = match client
        .get_item()
        .table_name(table_name)
        .key("pk", AttributeValue::S("snapshot".to_string()))
        .consistent_read(true)
        .send()
        .await
    {
        Ok(output) => output
            .item()
            .and_then(|item| item.get("data"))
            .and_then(|v| v.as_s().ok())
            .and_then(|text| serde_json::from_str::<TelemetrySnapshot>(text).ok())
            .unwrap_or_default(),
        Err(_) => TelemetrySnapshot::default(),
    };

    if let Some(td) = &req.telemetry_data {
        cumulative.accumulate_delta(td);
    }

    let prom_text = cumulative.to_prometheus(
        "lambda-resolver",
        &confidence_resolver::telemetry::PrometheusConfig::default(),
    );

    let snapshot_json = serde_json::to_string(&cumulative).unwrap_or_default();

    let _ = client
        .put_item()
        .table_name(table_name)
        .item("pk", AttributeValue::S("snapshot".to_string()))
        .item("data", AttributeValue::S(snapshot_json))
        .send()
        .await;

    let _ = client
        .put_item()
        .table_name(table_name)
        .item("pk", AttributeValue::S("prometheus".to_string()))
        .item("data", AttributeValue::S(prom_text))
        .send()
        .await;
}
