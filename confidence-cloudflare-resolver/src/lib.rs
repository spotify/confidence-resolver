use confidence_resolver::{
    assign_logger::AssignLogger,
    flag_logger,
    proto::{confidence, google::Struct},
    telemetry::{Telemetry, TelemetrySnapshot},
    FlagToApply, Host, ResolvedValue, ResolverState,
};
use worker::*;

use arc_swap::ArcSwap;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::Bytes;
use prost::Message;
use serde_json::from_slice;
use serde_json::json;

use confidence::flags::resolver::v1::{ApplyFlagsRequest, ApplyFlagsResponse, ResolveFlagsRequest};
use confidence_resolver::proto::confidence::flags::resolver::v1::{ResolveProcessRequest, ResolveReason};

static RESOLVE_LOGGER: LazyLock<ResolveLogger<H>> = LazyLock::new(ResolveLogger::new);
static ASSIGN_LOGGER: LazyLock<AssignLogger> = LazyLock::new(AssignLogger::new);
static TELEMETRY: LazyLock<Telemetry> = LazyLock::new(Telemetry::new);
static LAST_FLUSHED: LazyLock<ArcSwap<TelemetrySnapshot>> =
    LazyLock::new(|| ArcSwap::from_pointee(TelemetrySnapshot::default()));

use confidence_resolver::Client;
use once_cell::sync::Lazy;
use std::cell::RefCell;

/// Per-request resolve metrics captured in the hot path, recorded in wait_until.
struct ResolveMetrics {
    elapsed_us: u32,
    reasons: Vec<ResolveReason>,
}

thread_local! {
    static PENDING_METRICS: RefCell<Vec<ResolveMetrics>> = const { RefCell::new(Vec::new()) };
}

/// SetResolverStateRequest message from the CDN.
/// This matches the protobuf message format returned by the CDN.
#[derive(Clone, PartialEq, Message)]
pub struct SetResolverStateRequest {
    #[prost(bytes = "bytes", tag = "1")]
    pub state: Bytes,
    #[prost(string, tag = "2")]
    pub account_id: String,
}

/// The CDN response containing both the state and account_id
const CDN_STATE_BYTES: &[u8] = include_bytes!("../../data/resolver_state_current.pb");
const ENCRYPTION_KEY_BASE64: &str = include_str!("../../data/encryption_key");

use confidence::flags::resolver::v1::Sdk;
use confidence_resolver::proto::confidence::flags::resolver::v1::WriteFlagLogsRequest;
use confidence_resolver::resolve_logger::ResolveLogger;
use std::sync::{LazyLock, OnceLock};

static FLAGS_LOGS_QUEUE: OnceLock<Queue> = OnceLock::new();

static CONFIDENCE_CLIENT_SECRET: OnceLock<String> = OnceLock::new();

/// Parsed CDN state request containing both state and account_id
static CDN_STATE_REQUEST: Lazy<SetResolverStateRequest> = Lazy::new(|| {
    SetResolverStateRequest::decode(Bytes::from_static(CDN_STATE_BYTES))
        .expect("Failed to decode SetResolverStateRequest from CDN state")
});

static RESOLVER_STATE: Lazy<ResolverState> = Lazy::new(|| {
    let cdn_request = &*CDN_STATE_REQUEST;
    ResolverState::from_proto(
        cdn_request.state.to_vec().try_into().unwrap(),
        &cdn_request.account_id,
        None,
    )
    .unwrap()
});

trait ResponseExt {
    fn with_cors_headers(self, allowed_origin: &str) -> Result<Self>
    where
        Self: Sized;
}

struct H {}

impl Host for H {
    fn log_resolve(
        resolve_id: &str,
        evaluation_context: &Struct,
        values: &[ResolvedValue<'_>],
        client: &Client,
    ) {
        RESOLVE_LOGGER.log_resolve(
            resolve_id,
            evaluation_context,
            client.client_credential_name.as_str(),
            values,
            client,
        );
    }

    fn log_assign(
        resolve_id: &str,
        assigned_flags: &[FlagToApply],
        client: &Client,
        sdk: &Option<Sdk>,
    ) {
        ASSIGN_LOGGER.log_assigns(resolve_id, assigned_flags, client, sdk);
    }
}

fn set_client_secret(env: &Env) {
    if let Ok(var) = env.var("CONFIDENCE_CLIENT_SECRET") {
        let _ = CONFIDENCE_CLIENT_SECRET.set(var.to_string());
    } else {
        console_log!("no confidence client secret provided");
    }
}

#[event(fetch)]
pub async fn main(req: Request, env: Env, ctx: Context) -> Result<Response> {
    match env.queue("flag_logs_queue") {
        Ok(queue) => {
            let _ = FLAGS_LOGS_QUEUE.set(queue);
        }
        Err(_e) => {
            console_log!("flag_logs_queue binding is missing; logging disabled");
        }
    }

    set_client_secret(&env);

    let allowed_origin_env = env
        .var("ALLOWED_ORIGIN")
        .map(|var| var.to_string())
        .unwrap_or("*".to_string()); // Fallback to "*" if the variable is not set

    // Optional env var containing the resolver state ETag for this deployment
    let state_etag_env = env
        .var("RESOLVER_STATE_ETAG")
        .map(|var| var.to_string())
        .unwrap_or_default();

    // Optional env var containing the confidence-resolver commit used for this deployment
    let resolver_version_env = env
        .var("DEPLOYER_VERSION")
        .map(|var| var.to_string())
        .unwrap_or_default();

    if req.method() == Method::Options {
        return Response::ok("")?.with_cors_headers(&allowed_origin_env);
    }

    let state = &RESOLVER_STATE;
    let router = Router::new();

    let response = router
        .get_async("/metrics", |_req, ctx| {
            let allowed_origin = allowed_origin_env.clone();
            async move {
                let text = match ctx.env.kv("METRICS_KV") {
                    Ok(kv) => kv.get("prometheus").text().await.unwrap_or(None),
                    Err(_) => None,
                };
                let body = text.unwrap_or_default();
                let headers = Headers::new();
                headers.set("Content-Type", "text/plain; version=0.0.4; charset=utf-8")?;
                Response::ok(body)?.with_headers(headers).with_cors_headers(&allowed_origin)
            }
        })
        // GET endpoint to expose the current deployment state etag and resolver version
        .get_async("/v1/state:etag", |_req, _ctx| {
            let allowed_origin = allowed_origin_env.clone();
            let etag_value = state_etag_env.clone();
            let version_value = resolver_version_env.clone();
            async move {
                let body = json!({
                    "etag": etag_value,
                    "version": version_value,
                });
                Response::from_json(&body)?.with_cors_headers(&allowed_origin)
            }
        })
        // Router treats ":name" as parameters, which is incompatible without URLs
        // so we use "*path" to match the whole path and do the matching in the handler
        .post_async("/v1/*path", |mut req, ctx| {
            let allowed_origin = allowed_origin_env.clone();
            async move {
                let path = ctx.param("path").unwrap();
                match path.as_str() {
                    "flags:resolve" => {
                        let body_bytes: Vec<u8> = req.bytes().await?;
                        let mut resolver_request: ResolveFlagsRequest =
                            match from_slice(&body_bytes) {
                                Ok(req) => req,
                                Err(e) => {
                                    return Response::error(
                                        format!("Invalid request payload: {}", e),
                                        400,
                                    )?
                                    .with_cors_headers(&allowed_origin);
                                }
                            };
                        // Default apply to true for Cloudflare resolver
                        resolver_request.apply = true;
                        let evaluation_context = resolver_request
                            .evaluation_context
                            .clone()
                            .unwrap_or_default();
                        let start = js_sys::Date::now();
                        match state.get_resolver::<H>(
                            &resolver_request.client_secret,
                            evaluation_context,
                            &Bytes::from(STANDARD.decode(ENCRYPTION_KEY_BASE64).unwrap()),
                        ) {
                            Ok(resolver) => {
                                let process_request =
                                    ResolveProcessRequest::without_materializations(
                                        resolver_request,
                                    );
                                match resolver.resolve_flags(process_request) {
                                    Ok(process_response) => {
                                        let elapsed_us = ((js_sys::Date::now() - start) * 1000.0) as u32;
                                        match process_response.into_resolved() {
                                            Some((response, _writes)) => {
                                                let reasons: Vec<ResolveReason> = response
                                                    .resolved_flags
                                                    .iter()
                                                    .map(|f| f.reason())
                                                    .collect();
                                                PENDING_METRICS.with(|m| {
                                                    m.borrow_mut().push(ResolveMetrics { elapsed_us, reasons });
                                                });
                                                Response::from_json(&response)?
                                                    .with_cors_headers(&allowed_origin)
                                            }
                                            None => {
                                                PENDING_METRICS.with(|m| {
                                                    m.borrow_mut().push(ResolveMetrics {
                                                        elapsed_us,
                                                        reasons: vec![ResolveReason::Error],
                                                    });
                                                });
                                                Response::error(
                                                    "Unexpected suspended response",
                                                    500,
                                                )?
                                                .with_cors_headers(&allowed_origin)
                                            }
                                        }
                                    }
                                    Err(msg) => {
                                        let elapsed_us = ((js_sys::Date::now() - start) * 1000.0) as u32;
                                        PENDING_METRICS.with(|m| {
                                            m.borrow_mut().push(ResolveMetrics {
                                                elapsed_us,
                                                reasons: vec![ResolveReason::Error],
                                            });
                                        });
                                        Response::error(msg, 500)?
                                            .with_cors_headers(&allowed_origin)
                                    }
                                }
                            }
                            Err(msg) => {
                                let elapsed_us = ((js_sys::Date::now() - start) * 1000.0) as u32;
                                PENDING_METRICS.with(|m| {
                                    m.borrow_mut().push(ResolveMetrics {
                                        elapsed_us,
                                        reasons: vec![ResolveReason::Error],
                                    });
                                });
                                Response::error(msg, 500)?.with_cors_headers(&allowed_origin)
                            }
                        }
                    }
                    "flags:apply" => {
                        let body_bytes: Vec<u8> = req.bytes().await?;
                        let apply_flag_req: ApplyFlagsRequest = match from_slice(&body_bytes) {
                            Ok(req) => req,
                            Err(e) => {
                                return Response::error(
                                    format!("Invalid request payload: {}", e),
                                    400,
                                )?
                                .with_cors_headers(&allowed_origin);
                            }
                        };

                        match state.get_resolver::<H>(
                            &apply_flag_req.client_secret,
                            Struct::default(),
                            &Bytes::from(STANDARD.decode(ENCRYPTION_KEY_BASE64).unwrap()),
                        ) {
                            Ok(resolver) => match resolver.apply_flags(&apply_flag_req) {
                                Ok(()) => Response::from_json(&ApplyFlagsResponse::default()),
                                Err(msg) => {
                                    Response::error(msg, 500)?.with_cors_headers(&allowed_origin)
                                }
                            },
                            Err(msg) => {
                                Response::error(msg, 500)?.with_cors_headers(&allowed_origin)
                            }
                        }
                    }
                    _ => Response::error("Not found", 404)?.with_cors_headers(&allowed_origin),
                }
            }
        })
        .run(req, env)
        .await;

    // Use ctx.waitUntil to run logging and telemetry after response is returned
    ctx.wait_until(async move {
        // Record pending resolve metrics into the telemetry counters
        PENDING_METRICS.with(|m| {
            for metrics in m.borrow_mut().drain(..) {
                TELEMETRY.record_latency_us(metrics.elapsed_us);
                for reason in metrics.reasons {
                    TELEMETRY.mark_resolve(reason);
                }
            }
        });

        let aggregated: confidence_resolver::proto::confidence::flags::resolver::v1::WriteFlagLogsRequest
            = checkpoint();
        if let Ok(converted) = serde_json::to_string(&aggregated) {
            if let Some(queue) = FLAGS_LOGS_QUEUE.get() {
                let _ = queue.send(converted).await;
            }
        }
    });

    response
}

#[event(queue)]
pub async fn consume_flag_logs_queue(
    message_batch: MessageBatch<String>,
    env: Env,
    _ctx: Context,
) -> Result<()> {
    set_client_secret(&env);

    if let Ok(messages) = message_batch.messages() {
        let logs: Vec<WriteFlagLogsRequest> = messages
            .iter()
            .map(|m| m.body().clone())
            .map(|s| serde_json::from_str::<confidence_resolver::proto::confidence::flags::resolver::v1::WriteFlagLogsRequest>(s.as_str()).unwrap())
            .map(|v| WriteFlagLogsRequest {
                telemetry_data: v.telemetry_data,
                flag_resolve_info: v.flag_resolve_info,
                flag_assigned: v.flag_assigned,
                client_resolve_info: v.client_resolve_info,
            })
            .collect();

        // Accumulate telemetry deltas into KV-backed cumulative snapshot for /metrics
        if let Ok(kv) = env.kv("METRICS_KV") {
            let _ = update_prometheus_kv(&kv, &logs).await;
        }

        let req = flag_logger::aggregate_batch(logs);
        send_flags_logs(CONFIDENCE_CLIENT_SECRET.get().unwrap().as_str(), req).await?;
    }

    Ok(())
}

fn checkpoint() -> WriteFlagLogsRequest {
    let mut req = RESOLVE_LOGGER.checkpoint();
    req.telemetry_data = Some(TELEMETRY.delta_snapshot(&LAST_FLUSHED));
    ASSIGN_LOGGER.checkpoint_fill(&mut req);
    req
}

/// Accumulate telemetry deltas from all isolates into a cumulative
/// `TelemetrySnapshot` stored in KV, then write its Prometheus text
/// representation for the /metrics endpoint.
///
/// Note: concurrent queue consumer invocations can race on KV read-modify-write.
/// Acceptable for metrics — at worst one batch's deltas are lost, not cumulative state.
async fn update_prometheus_kv(kv: &kv::KvStore, logs: &[WriteFlagLogsRequest]) {
    let mut cumulative = match kv.get("snapshot").text().await {
        Ok(Some(text)) => serde_json::from_str::<TelemetrySnapshot>(&text).unwrap_or_default(),
        _ => TelemetrySnapshot::default(),
    };

    for log in logs {
        if let Some(td) = &log.telemetry_data {
            cumulative.accumulate_delta(td);
        }
    }

    let prom_text = cumulative.to_prometheus(
        "cf-resolver",
        &confidence_resolver::telemetry::PrometheusConfig::default(),
    );

    if let Ok(builder) = kv.put("snapshot", serde_json::to_string(&cumulative).unwrap_or_default()) {
        let _ = builder.execute().await;
    }
    if let Ok(builder) = kv.put("prometheus", prom_text) {
        let _ = builder.execute().await;
    }
}

async fn send_flags_logs(client_secret: &str, message: WriteFlagLogsRequest) -> Result<Response> {
    let resolve_url = "https://resolver.confidence.dev/v1/clientFlagLogs:write";
    let mut init = RequestInit::new();
    let headers = Headers::new();
    headers.set("Content-Type", "application/json")?;
    headers.set("Authorization", &format!("ClientSecret {}", client_secret))?;
    init.with_headers(headers);
    init.with_method(Method::Post);
    let json = serde_json::to_string(&message)?;
    init.with_body(Some(json.into()));
    let request = Request::new_with_init(resolve_url, &init)?;
    let response = Fetch::Request(request).send().await;
    response
}

impl ResponseExt for Response {
    fn with_cors_headers(mut self, allowed_origin: &str) -> Result<Self>
    where
        Self: Sized,
    {
        let headers = self.headers_mut();

        headers.set("Access-Control-Allow-Origin", allowed_origin)?;
        headers.set("Access-Control-Allow-Methods", "POST, GET, OPTIONS")?;
        headers.set("Access-Control-Allow-Headers", "*")?;

        Ok(self)
    }
}
