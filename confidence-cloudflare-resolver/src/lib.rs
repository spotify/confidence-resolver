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
use confidence_resolver::proto::confidence::flags::resolver::v1::{
    TelemetryData,
    telemetry_data::{BucketSpan, ResolveLatency},
};

static RESOLVE_LOGGER: LazyLock<ResolveLogger<H>> = LazyLock::new(ResolveLogger::new);
static ASSIGN_LOGGER: LazyLock<AssignLogger> = LazyLock::new(AssignLogger::new);
static TELEMETRY: LazyLock<Telemetry> = LazyLock::new(Telemetry::new);
static LAST_FLUSHED: LazyLock<ArcSwap<TelemetrySnapshot>> =
    LazyLock::new(|| ArcSwap::from_pointee(TelemetrySnapshot::default()));

use confidence_resolver::Client;
use once_cell::sync::Lazy;
use std::cell::RefCell;

/// Per-request resolve metrics captured in the hot path, recorded in wait_until.
/// Note: CF Workers freeze all timer APIs during sync CPU work (Spectre mitigation),
/// so we cannot measure resolve latency internally. CPU time is sourced from
/// Cloudflare's analytics API in the queue consumer instead.
struct ResolveMetrics {
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
                let text = match ctx.env.kv("CONFIDENCE_METRICS_KV") {
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
                                        match process_response.into_resolved() {
                                            Some((response, _writes)) => {
                                                let reasons: Vec<ResolveReason> = response
                                                    .resolved_flags
                                                    .iter()
                                                    .map(|f| f.reason())
                                                    .collect();
                                                PENDING_METRICS.with(|m| {
                                                    m.borrow_mut().push(ResolveMetrics { reasons });
                                                });
                                                Response::from_json(&response)?
                                                    .with_cors_headers(&allowed_origin)
                                            }
                                            None => {
                                                PENDING_METRICS.with(|m| {
                                                    m.borrow_mut().push(ResolveMetrics {
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
                                        PENDING_METRICS.with(|m| {
                                            m.borrow_mut().push(ResolveMetrics {
                                                reasons: vec![ResolveReason::Error],
                                            });
                                        });
                                        Response::error(msg, 500)?
                                            .with_cors_headers(&allowed_origin)
                                    }
                                }
                            }
                            Err(msg) => {
                                PENDING_METRICS.with(|m| {
                                    m.borrow_mut().push(ResolveMetrics {
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

    // Use ctx.waitUntil to run logging and telemetry after response is returned.
    // Note: resolve latency cannot be measured inside Workers (timer APIs are
    // frozen during sync CPU work). CPU time is sourced from Cloudflare's
    // analytics API in the queue consumer instead.
    ctx.wait_until(async move {
        PENDING_METRICS.with(|m| {
            for metrics in m.borrow_mut().drain(..) {
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
        if let Ok(kv) = env.kv("CONFIDENCE_METRICS_KV") {
            let _ = update_prometheus_kv(&kv, &logs).await;
        }

        // Fetch real CPU time from CF analytics API and append to Prometheus KV.
        // Returns a TelemetryData delta when new latency data is available.
        let cf_latency = {
            let cf_token = env.var("CLOUDFLARE_API_TOKEN").ok().map(|v| v.to_string());
            let cf_account = env.var("CLOUDFLARE_ACCOUNT_ID").ok().map(|v| v.to_string());
            let cf_script = env
                .var("CF_SCRIPT_NAME")
                .ok()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "confidence-cloudflare-resolver".to_string());
            match (cf_token, cf_account) {
                (Some(token), Some(account)) => match env.kv("CONFIDENCE_METRICS_KV") {
                    Ok(kv) => update_cpu_time_kv(&kv, &token, &account, &cf_script).await,
                    Err(_) => None,
                },
                _ => None,
            }
        };

        let mut req = flag_logger::aggregate_batch(logs);
        // Inject CF analytics latency into the telemetry sent to the backend
        if let Some(latency_td) = cf_latency {
            match &mut req.telemetry_data {
                Some(td) => {
                    td.resolve_latency = latency_td.resolve_latency;
                }
                None => {
                    req.telemetry_data = Some(latency_td);
                }
            }
        }
        send_flags_logs(CONFIDENCE_CLIENT_SECRET.get().unwrap().as_str(), req).await?;
    }

    Ok(())
}

fn checkpoint() -> WriteFlagLogsRequest {
    let mut req = RESOLVE_LOGGER.checkpoint();
    let mut td = TELEMETRY.delta_snapshot(&LAST_FLUSHED);
    td.sdk = Some(confidence::flags::resolver::v1::Sdk {
        sdk: Some(confidence::flags::resolver::v1::sdk::Sdk::Id(
            confidence::flags::resolver::v1::SdkId::CloudflareResolver as i32,
        )),
        version: env!("CARGO_PKG_VERSION").to_string(),
    });
    req.telemetry_data = Some(td);
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

/// Query Cloudflare's analytics GraphQL API for Worker CPU time and write
/// the percentiles as Prometheus gauge metrics to KV.
/// Return an ISO-8601 timestamp `seconds_ago` in the past.
fn since_iso8601(seconds_ago: u64) -> String {
    let now_ms = js_sys::Date::now() as u64;
    let then_ms = now_ms.saturating_sub(seconds_ago.saturating_mul(1000));
    let d = js_sys::Date::new_0();
    d.set_time(then_ms as f64);
    // Use Date.toISOString() for proper formatting
    d.to_iso_string().into()
}

/// Query recent CPU time from Cloudflare analytics and update the cumulative
/// `TelemetrySnapshot` in KV so the Prometheus output uses the same metric
/// names as all other providers (`confidence_resolve_latency_microseconds`).
///
/// Since CF Workers freeze timers during sync CPU work, we can't measure
/// latency internally. Instead, we use CF's own analytics (p50 × requests)
/// as an approximation for the histogram's `_sum` and `_count`.
///
/// To avoid double-counting, we store the last queried timestamp in KV
/// (`cpu_time_cursor`) and only process data points newer than that.
async fn update_cpu_time_kv(kv: &kv::KvStore, token: &str, account: &str, script: &str) -> Option<TelemetryData> {
    // Read cursor: the last datetime we processed (also serves as rate-limit)
    let cursor = match kv.get("cpu_time_cursor").text().await {
        Ok(Some(c)) if !c.is_empty() => c,
        _ => since_iso8601(300), // bootstrap: last 5 minutes
    };

    // Rate-limit: skip if cursor is less than 10 seconds old.
    // Cursor is an ISO-8601 datetime from CF analytics; parse via JS Date.
    let cursor_ms = js_sys::Date::parse(&cursor);
    let now_ms = js_sys::Date::now();
    if (now_ms - cursor_ms) < 10_000.0 {
        return None;
    }

    let gql = format!(
        "{{ viewer {{ accounts(filter: {{accountTag: \"{account}\"}}) {{ workersInvocationsAdaptive(limit: 50, filter: {{scriptName: \"{script}\", datetime_gt: \"{cursor}\"}}, orderBy: [datetime_ASC]) {{ dimensions {{ datetime }} quantiles {{ cpuTimeP25 cpuTimeP50 cpuTimeP75 cpuTimeP90 cpuTimeP99 }} sum {{ requests }} }} }} }} }}"
    );
    let query = serde_json::to_string(&json!({ "query": gql })).unwrap_or_default();

    let url = "https://api.cloudflare.com/client/v4/graphql";
    let mut init = RequestInit::new();
    let headers = Headers::new();
    let _ = headers.set("Authorization", &format!("Bearer {token}"));
    let _ = headers.set("Content-Type", "application/json");
    init.with_headers(headers);
    init.with_method(Method::Post);
    init.with_body(Some(query.into()));

    let Ok(request) = Request::new_with_init(url, &init) else { return None };
    let Ok(mut resp) = Fetch::Request(request).send().await else { return None };
    let Ok(body) = resp.text().await else { return None };
    let Ok(data) = serde_json::from_str::<serde_json::Value>(&body) else { return None };

    let Some(entries) = data
        .pointer("/data/viewer/accounts/0/workersInvocationsAdaptive")
        .and_then(|v| v.as_array())
    else { return None };

    if entries.is_empty() { return None }

    // Aggregate only NEW data points (after cursor)
    let mut total_observations: u64 = 0;
    let mut weighted_sum: u64 = 0;
    let mut bucket_adds: Vec<(u64, u64)> = Vec::new(); // (μs_value, count)
    let mut latest_datetime = String::new();

    for entry in entries {
        let reqs = entry.pointer("/sum/requests").and_then(|v| v.as_u64()).unwrap_or(0);
        if reqs == 0 { continue }
        let q = entry.pointer("/quantiles");
        let p25 = q.and_then(|q| q.get("cpuTimeP25")).and_then(|v| v.as_u64()).unwrap_or(0);
        let p50 = q.and_then(|q| q.get("cpuTimeP50")).and_then(|v| v.as_u64()).unwrap_or(0);
        let p75 = q.and_then(|q| q.get("cpuTimeP75")).and_then(|v| v.as_u64()).unwrap_or(0);
        let p90 = q.and_then(|q| q.get("cpuTimeP90")).and_then(|v| v.as_u64()).unwrap_or(0);
        let p99 = q.and_then(|q| q.get("cpuTimeP99")).and_then(|v| v.as_u64()).unwrap_or(0);

        // Distribute requests into histogram buckets using CF percentiles.
        // For single-request data points all percentiles are identical, so
        // we just place 1 observation at p50. For multi-request points we
        // spread across bands: [0-25%] p25, (25-50%] p50, (50-75%] p75,
        // (75-90%] p90, (90-99%] p99, (99-100%] p99.
        if reqs == 1 {
            bucket_adds.push((p50, 1));
            weighted_sum = weighted_sum.saturating_add(p50);
            total_observations = total_observations.saturating_add(1);
        } else {
            let bands: &[(f64, u64)] = &[
                (0.25, p25), (0.25, p50), (0.25, p75), (0.15, p90), (0.09, p99), (0.01, p99),
            ];
            let mut remaining = reqs;
            for (i, &(frac, value)) in bands.iter().enumerate() {
                let count = if i == bands.len() - 1 {
                    remaining
                } else {
                    let c = (reqs as f64 * frac) as u64;
                    c.min(remaining)
                };
                remaining = remaining.saturating_sub(count);
                if count > 0 && value > 0 {
                    bucket_adds.push((value, count));
                    weighted_sum = weighted_sum.saturating_add(value.saturating_mul(count));
                    total_observations = total_observations.saturating_add(count);
                }
            }
        }

        if let Some(dt) = entry.pointer("/dimensions/datetime").and_then(|v| v.as_str()) {
            if dt > latest_datetime.as_str() {
                latest_datetime = dt.to_string();
            }
        }
    }

    if total_observations == 0 || latest_datetime.is_empty() { return None }

    // Update cursor to latest processed timestamp
    if let Ok(builder) = kv.put("cpu_time_cursor", &latest_datetime) {
        let _ = builder.execute().await;
    }

    // Update the cumulative snapshot with histogram buckets, sum, and count
    let mut cumulative = match kv.get("snapshot").text().await {
        Ok(Some(text)) => serde_json::from_str::<TelemetrySnapshot>(&text).unwrap_or_default(),
        _ => TelemetrySnapshot::default(),
    };

    cumulative.latency.sum = cumulative.latency.sum.saturating_add(weighted_sum);
    cumulative.latency.count = cumulative.latency.count.saturating_add(total_observations);

    // Place observations into exponential histogram buckets (same as other providers)
    let ln_ratio: f64 = core::f64::consts::LN_10 / 18.0;
    for &(us_value, count) in &bucket_adds {
        let idx = if us_value == 0 {
            0
        } else {
            ((us_value as f64).ln() / ln_ratio).floor() as usize
        };
        if idx >= cumulative.latency.buckets.len() {
            cumulative.latency.buckets.resize(idx.saturating_add(1), 0);
        }
        if let Some(b) = cumulative.latency.buckets.get_mut(idx) {
            *b = b.saturating_add(count);
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

    // Build BucketSpans for the TelemetryData delta sent to the backend.
    let spans = {
        let mut flat: Vec<u32> = Vec::new();
        for &(us_value, count) in &bucket_adds {
            let idx = if us_value == 0 { 0 } else {
                ((us_value as f64).ln() / ln_ratio).floor() as usize
            };
            if idx >= flat.len() { flat.resize(idx + 1, 0); }
            if let Some(b) = flat.get_mut(idx) { *b = b.saturating_add(count as u32); }
        }
        // Compress into BucketSpans (contiguous non-zero runs)
        let mut spans: Vec<BucketSpan> = Vec::new();
        let mut current: Option<(i32, Vec<u32>)> = None;
        for (i, &v) in flat.iter().enumerate() {
            if v > 0 {
                match &mut current {
                    Some((_, counts)) => counts.push(v),
                    None => current = Some((i as i32, vec![v])),
                }
            } else if let Some((offset, counts)) = current.take() {
                spans.push(BucketSpan { offset, counts });
            }
        }
        if let Some((offset, counts)) = current {
            spans.push(BucketSpan { offset, counts });
        }
        spans
    };

    Some(TelemetryData {
        resolve_latency: Some(ResolveLatency {
            sum: weighted_sum as u32,
            count: total_observations as u32,
            buckets: spans,
            ln_ratio,
        }),
        ..Default::default()
    })
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
