use std::sync::OnceLock;
use std::time::Instant;

use aws_sdk_dynamodb::Client as DynamoClient;
use aws_sdk_sqs::Client as SqsClient;
use lambda_http::{service_fn, Body, Error, Request, Response};
use serde_json::{from_slice, json};

use confidence_resolver::proto::confidence::flags::resolver::v1::{
    resolve_process_response, ApplyFlagsRequest, ApplyFlagsResponse, MaterializationRecord,
    ResolveProcessRequest, ResolveReason, ResolveFlagsRequest, ResolveFlagsResponse,
    WriteFlagLogsRequest,
};
use confidence_resolver::proto::google::Struct;
use confidence_resolver::telemetry;

use crate::common::{
    add_cors_headers, env_var, env_var_opt, sdk_info, LambdaHost, CONFIDENCE_CLIENT_SECRET,
    ENCRYPTION_KEY, FLAG_LOG, PROMETHEUS_CONTENT_TYPE, RESOLVER_STATE,
};
use crate::materialization::{
    materialization_records_to_read_ops, materialization_records_to_write_ops,
    read_results_to_materialization_records, DynamoDbMaterializationStore, MaterializationStore,
};

static SQS_CLIENT: OnceLock<SqsClient> = OnceLock::new();
static SQS_QUEUE_URL: OnceLock<String> = OnceLock::new();
static DYNAMO_CLIENT: OnceLock<DynamoClient> = OnceLock::new();
static METRICS_TABLE: OnceLock<String> = OnceLock::new();
static MATERIALIZATION_STORE: OnceLock<DynamoDbMaterializationStore> = OnceLock::new();

pub async fn run() -> Result<(), Error> {
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;

    let _ = SQS_CLIENT.set(SqsClient::new(&config));
    if let Some(url) = env_var_opt("SQS_QUEUE_URL") {
        let _ = SQS_QUEUE_URL.set(url);
    }

    let dynamo = DynamoClient::new(&config);
    if let Some(table) = env_var_opt("DYNAMODB_METRICS_TABLE") {
        let _ = METRICS_TABLE.set(table);
    }
    if let Some(table) = env_var_opt("DYNAMODB_MATERIALIZATIONS_TABLE") {
        let _ = MATERIALIZATION_STORE.set(DynamoDbMaterializationStore::new(
            dynamo.clone(),
            table,
        ));
    }
    let _ = DYNAMO_CLIENT.set(dynamo);

    if let Some(secret) = env_var_opt("CONFIDENCE_CLIENT_SECRET") {
        let _ = CONFIDENCE_CLIENT_SECRET.set(secret);
    }

    let _ = &*RESOLVER_STATE;
    tracing::info!("resolver state initialized");

    lambda_http::run(service_fn(handler)).await
}

async fn handler(req: Request) -> Result<Response<Body>, Error> {
    let allowed_origin = env_var_opt("ALLOWED_ORIGIN").unwrap_or_else(|| "*".to_string());
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    if method == http::Method::OPTIONS {
        return Ok(add_cors_headers(Response::builder(), &allowed_origin)
            .status(200)
            .body(Body::Empty)?);
    }

    FLAG_LOG.with(|f| *f.borrow_mut() = Some(WriteFlagLogsRequest::default()));

    let response = match (method.as_str(), path.as_str()) {
        ("GET", "/metrics") => handle_metrics(&req, &allowed_origin).await,
        ("GET", "/v1/state:etag") => handle_state_etag(&allowed_origin),
        ("POST", "/v1/flags:resolve") => handle_resolve(req, &allowed_origin).await,
        ("POST", "/v1/flags:apply") => handle_apply(req, &allowed_origin).await,
        _ => Ok(add_cors_headers(Response::builder(), &allowed_origin)
            .status(404)
            .body(Body::Text("Not found".to_string()))?),
    };

    let flag_log = FLAG_LOG.with(|f| f.borrow_mut().take());
    if let (Some(log), Some(sqs), Some(url)) =
        (flag_log, SQS_CLIENT.get(), SQS_QUEUE_URL.get())
    {
        let url = url.clone();
        tokio::spawn(async move {
            if let Ok(json) = serde_json::to_string(&log) {
                let _ = sqs.send_message().queue_url(url).message_body(json).send().await;
            }
        });
    }

    response
}

async fn handle_metrics(req: &Request, allowed_origin: &str) -> Result<Response<Body>, Error> {
    if let Some(expected) = CONFIDENCE_CLIENT_SECRET.get() {
        let authorized = req
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("ClientSecret "))
            .map(|v| v == expected.as_str())
            .unwrap_or(false);
        if !authorized {
            return Ok(add_cors_headers(Response::builder(), allowed_origin)
                .status(401)
                .body(Body::Text("Unauthorized".to_string()))?);
        }
    }

    let body = if let (Some(client), Some(table)) = (DYNAMO_CLIENT.get(), METRICS_TABLE.get()) {
        use aws_sdk_dynamodb::types::AttributeValue;
        client
            .get_item()
            .table_name(table)
            .key("pk", AttributeValue::S("prometheus".to_string()))
            .send()
            .await
            .ok()
            .and_then(|out| out.item().cloned())
            .and_then(|item| item.get("data").cloned())
            .and_then(|v| v.as_s().ok().cloned())
            .unwrap_or_default()
    } else {
        String::new()
    };

    Ok(add_cors_headers(Response::builder(), allowed_origin)
        .status(200)
        .header("Content-Type", PROMETHEUS_CONTENT_TYPE)
        .header("Cache-Control", "no-store")
        .body(Body::Text(body))?)
}

fn handle_state_etag(allowed_origin: &str) -> Result<Response<Body>, Error> {
    let etag = env_var("RESOLVER_STATE_ETAG");
    let version = env_var("DEPLOYER_VERSION");
    let body = json!({ "etag": etag, "version": version });

    Ok(add_cors_headers(Response::builder(), allowed_origin)
        .status(200)
        .header("Content-Type", "application/json")
        .body(Body::Text(body.to_string()))?)
}

async fn handle_resolve(req: Request, allowed_origin: &str) -> Result<Response<Body>, Error> {
    let body_bytes = req.into_body();
    let bytes: &[u8] = match &body_bytes {
        Body::Text(s) => s.as_bytes(),
        Body::Binary(b) => b.as_ref(),
        Body::Empty => &[],
    };

    let mut resolver_request: ResolveFlagsRequest = match from_slice(bytes) {
        Ok(r) => r,
        Err(e) => {
            return Ok(add_cors_headers(Response::builder(), allowed_origin)
                .status(400)
                .body(Body::Text(format!("Invalid request payload: {}", e)))?)
        }
    };
    resolver_request.apply = true;

    let evaluation_context = resolver_request
        .evaluation_context
        .clone()
        .unwrap_or_default();

    let t0 = Instant::now();
    let state = &*RESOLVER_STATE;

    let (reasons, resp_body, status) = match state.get_resolver::<LambdaHost>(
        &resolver_request.client_secret,
        evaluation_context,
        &ENCRYPTION_KEY,
    ) {
        Ok(resolver) => {
            let has_mat_store = MATERIALIZATION_STORE.get().is_some();
            let process_request = if has_mat_store {
                ResolveProcessRequest::deferred_materializations(resolver_request)
            } else {
                ResolveProcessRequest::without_materializations(resolver_request)
            };

            match resolve_with_materializations(&resolver, process_request).await {
                Ok((response, writes, reasons)) => {
                    if !writes.is_empty() {
                        if let Some(store) = MATERIALIZATION_STORE.get() {
                            let write_ops = materialization_records_to_write_ops(&writes);
                            tokio::spawn(async move {
                                if let Err(e) = store.write_materializations(write_ops).await {
                                    tracing::warn!("Failed to write materializations: {}", e);
                                }
                            });
                        }
                    }

                    let json = serde_json::to_string(&response)
                        .unwrap_or_else(|_| "{}".to_string());
                    (reasons, json, 200)
                }
                Err(msg) => (vec![ResolveReason::Error], msg, 500),
            }
        }
        Err(msg) => (vec![ResolveReason::Error], msg, 500),
    };

    let elapsed_us = Some(t0.elapsed().as_micros().min(u32::MAX as u128) as u32);
    let mut td = telemetry::build_request_telemetry(elapsed_us, &reasons);
    td.sdk = Some(sdk_info());
    FLAG_LOG.with(|f| {
        if let Some(req) = f.borrow_mut().as_mut() {
            req.telemetry_data = Some(td);
        }
    });

    Ok(add_cors_headers(Response::builder(), allowed_origin)
        .status(status)
        .header("Content-Type", "application/json")
        .body(Body::Text(resp_body))?)
}

async fn resolve_with_materializations(
    resolver: &confidence_resolver::AccountResolver<'_, LambdaHost>,
    process_request: ResolveProcessRequest,
) -> Result<(ResolveFlagsResponse, Vec<MaterializationRecord>, Vec<ResolveReason>), String> {
    let response = resolver.resolve_flags(process_request)?;

    let resolved = match response.result {
        Some(resolve_process_response::Result::Resolved(r)) => r,
        Some(resolve_process_response::Result::Suspended(suspended)) => {
            let store = MATERIALIZATION_STORE
                .get()
                .ok_or("Suspended but no materialization store configured")?;
            let read_ops =
                materialization_records_to_read_ops(&suspended.materializations_to_read);
            let read_results = store
                .read_materializations(read_ops)
                .await
                .map_err(|e| format!("Materialization read failed: {}", e))?;
            let records = read_results_to_materialization_records(read_results);
            let resume_request = ResolveProcessRequest::resume(records, suspended.state);

            match resolver.resolve_flags(resume_request)?.result {
                Some(resolve_process_response::Result::Resolved(r)) => r,
                Some(resolve_process_response::Result::Suspended(_)) => {
                    return Err("Unexpected second suspension after resume".to_string());
                }
                None => return Err("No resolve result after resume".to_string()),
            }
        }
        None => return Err("No resolve result".to_string()),
    };

    let response = resolved
        .response
        .ok_or_else(|| "Missing response in resolved result".to_string())?;
    let reasons: Vec<ResolveReason> = response
        .resolved_flags
        .iter()
        .map(|f| f.reason())
        .collect();

    Ok((response, resolved.materializations_to_write, reasons))
}

async fn handle_apply(req: Request, allowed_origin: &str) -> Result<Response<Body>, Error> {
    let body_bytes = req.into_body();
    let bytes: &[u8] = match &body_bytes {
        Body::Text(s) => s.as_bytes(),
        Body::Binary(b) => b.as_ref(),
        Body::Empty => &[],
    };

    let apply_req: ApplyFlagsRequest = match from_slice(bytes) {
        Ok(r) => r,
        Err(e) => {
            return Ok(add_cors_headers(Response::builder(), allowed_origin)
                .status(400)
                .body(Body::Text(format!("Invalid request payload: {}", e)))?)
        }
    };

    let state = &*RESOLVER_STATE;
    match state.get_resolver::<LambdaHost>(
        &apply_req.client_secret,
        Struct::default(),
        &ENCRYPTION_KEY,
    ) {
        Ok(resolver) => match resolver.apply_flags(&apply_req) {
            Ok(()) => {
                let json =
                    serde_json::to_string(&ApplyFlagsResponse::default()).unwrap_or_default();
                Ok(add_cors_headers(Response::builder(), allowed_origin)
                    .status(200)
                    .header("Content-Type", "application/json")
                    .body(Body::Text(json))?)
            }
            Err(msg) => Ok(add_cors_headers(Response::builder(), allowed_origin)
                .status(500)
                .body(Body::Text(msg))?),
        },
        Err(msg) => Ok(add_cors_headers(Response::builder(), allowed_origin)
            .status(500)
            .body(Body::Text(msg))?),
    }
}
