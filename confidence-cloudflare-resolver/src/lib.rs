mod materialization;

use confidence_resolver::{
    apply_dedup::{ApplyDedup, ApplyDedupSnapshot},
    assign_logger, flag_logger,
    proto::{confidence, google::Struct},
    resolve_logger,
    telemetry::{self, TelemetrySnapshot},
    AccountResolver, FlagToApply, Host, LogDestination, ResolvedValue, ResolverState,
};
use worker::*;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::Bytes;
use prost::Message;
use serde_json::from_slice;
use serde_json::json;
use std::cell::{Cell, RefCell};
use wasm_bindgen::JsCast;

use confidence::flags::resolver::v1::{ApplyFlagsRequest, ApplyFlagsResponse, ResolveFlagsRequest};
use confidence_resolver::proto::confidence::flags::resolver::v1::{
    resolve_process_response, MaterializationRecord, ResolveProcessRequest, ResolveReason,
    ResolveFlagsResponse,
};

use confidence_resolver::Client;
use once_cell::sync::Lazy;

#[derive(Clone, PartialEq, Message)]
pub struct ClientResolverState {
    #[prost(bytes = "bytes", tag = "1")]
    pub state: Bytes,
    #[prost(string, tag = "2")]
    pub account_id: String,
    #[prost(int32, repeated, tag = "4")]
    pub log_destinations: Vec<i32>,
}

/// The CDN response containing both the state and account_id
const CDN_STATE_BYTES: &[u8] = include_bytes!("../../data/resolver_state_current.pb");

use confidence::flags::resolver::v1::Sdk;
use confidence_resolver::proto::confidence::flags::resolver::v1::WriteFlagLogsRequest;
use std::sync::OnceLock;

thread_local! {
    // Side channel for the `Host` logging callbacks, which are static methods
    // with no way to reach their caller. Only ever `Some` inside `with_log`.
    static FLAG_LOG: RefCell<Option<WriteFlagLogsRequest>> = const { RefCell::new(None) };
    static APPLY_DEDUP: RefCell<ApplyDedup> = RefCell::new(ApplyDedup::new(120, 100_000));
    static APPLY_DEDUP_ENABLED: Cell<bool> = const { Cell::new(false) };
    static LAST_DEDUP_SNAPSHOT: RefCell<ApplyDedupSnapshot> =
        RefCell::new(ApplyDedupSnapshot::default());
}

fn dedup_telemetry_delta() -> Option<confidence::flags::resolver::v1::telemetry_data::ApplyDedupTelemetry> {
    if !APPLY_DEDUP_ENABLED.with(|c| c.get()) {
        return None;
    }
    let current = APPLY_DEDUP.with(|d| d.borrow().telemetry_snapshot());
    // The baseline advances only for a delta we actually return. Advancing it
    // unconditionally and then discarding the delta would permanently lose
    // those counts — notably sweeps, which keep running after the map empties
    // and so yield deltas with no applies and a zero map_size. This is also why
    // an idle worker's gauges no longer stick at their last non-zero reading.
    LAST_DEDUP_SNAPSHOT.with(|s| {
        let mut last = s.borrow_mut();
        let delta = current.delta_to_report(&last)?;
        *last = current;
        Some(delta)
    })
}

/// Queues one request's flag log and sweeps the apply-dedup map. Called via
/// `Context::wait_until`, so both run after the response has been returned.
async fn queue_flag_log(log: WriteFlagLogsRequest) {
    if APPLY_DEDUP_ENABLED.with(|c| c.get()) {
        APPLY_DEDUP.with(|d| d.borrow_mut().sweep((js_sys::Date::now() / 1000.0) as i64));
    }
    match serde_json::to_string(&log) {
        Ok(json) => {
            if let Some(queue) = FLAGS_LOGS_QUEUE.get() {
                if let Err(e) = queue.send(json).await {
                    console_log!("flag log queue send failed: {:?}", e);
                }
            }
        }
        Err(e) => console_log!("flag log serialize failed: {:?}", e),
    }
}

/// Runs `f` with `log` installed as the destination for the `Host` logging
/// callbacks, then moves whatever they wrote back into `log`. Call it once per
/// entry into the resolver; repeated calls accumulate into the same `log`.
fn with_log<T>(log: &mut WriteFlagLogsRequest, f: impl FnOnce() -> T) -> T {
    FLAG_LOG.with(|slot| {
        let local = RefCell::new(Some(std::mem::take(log)));
        slot.swap(&local);
        let result = f();
        slot.swap(&local);
        *log = local.into_inner().unwrap_or_default();
        result
    })
}

/// Seeds the resolver's RNG once per isolate with host entropy. Without this
/// every isolate produces the same resolve-id sequence, so ids collide across
/// isolates and downstream consumers that key on resolve id misbehave.
fn seed_resolver_rng() {
    static SEEDED: OnceLock<()> = OnceLock::new();
    SEEDED.get_or_init(|| {
        let seed = getrandom::u64().unwrap_or_else(|e| {
            console_log!("host entropy unavailable, using weak seed: {:?}", e);
            let hi = (js_sys::Math::random() * (u32::MAX as f64)) as u64;
            let lo = (js_sys::Math::random() * (u32::MAX as f64)) as u64;
            (hi << 32) ^ lo ^ (js_sys::Date::now() as u64)
        });
        confidence_resolver::seed_rng(seed);
    });
}

/// Prometheus exposition format content type (version 0.0.4).
const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

static FLAGS_LOGS_QUEUE: OnceLock<Queue> = OnceLock::new();

static EVENTS_QUEUE: OnceLock<Queue> = OnceLock::new();

static CONFIDENCE_CLIENT_SECRET: OnceLock<String> = OnceLock::new();

static RESOLVE_TOKEN_KEY: OnceLock<Bytes> = OnceLock::new();

/// Parsed CDN state request containing both state and account_id
static CDN_STATE_REQUEST: Lazy<ClientResolverState> = Lazy::new(|| {
    ClientResolverState::decode(Bytes::from_static(CDN_STATE_BYTES))
        .expect("Failed to decode ClientResolverState from CDN state")
});

static LOG_DESTINATIONS: Lazy<Vec<LogDestination>> = Lazy::new(|| {
    let raw = &CDN_STATE_REQUEST.log_destinations;
    let parsed: Vec<LogDestination> = raw.iter().map(|&v| LogDestination::from(v)).collect();
    if parsed.is_empty() {
        vec![LogDestination::Edge]
    } else {
        parsed
    }
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
        assigned_flags: &[FlagToApply<'_>],
        client: &Client,
        sdk: &Option<Sdk>,
    ) {
        if !assigned_flags.is_empty() && APPLY_DEDUP_ENABLED.with(|c| c.get()) {
            let now_seconds = (js_sys::Date::now() / 1000.0) as i64;
            let result = APPLY_DEDUP.with(|dedup| {
                dedup.borrow_mut().filter_duplicates(assigned_flags, now_seconds)
            });
            if result.is_empty() {
                return;
            }
            if result.kept_count() < assigned_flags.len() {
                let filtered = result.collect(assigned_flags);
                FLAG_LOG.with(|f| {
                    if let Some(req) = f.borrow_mut().as_mut() {
                        req.flag_assigned
                            .push(assign_logger::build_flag_assigned(
                                resolve_id, &filtered, client, sdk,
                            ));
                    }
                });
                return;
            }
        }
        FLAG_LOG.with(|f| {
            if let Some(req) = f.borrow_mut().as_mut() {
                req.flag_assigned
                    .push(assign_logger::build_flag_assigned(
                        resolve_id, assigned_flags, client, sdk,
                    ));
            }
        });
    }
}

fn set_client_secret(env: &Env) {
    if let Ok(var) = env.var("CONFIDENCE_CLIENT_SECRET") {
        let _ = CONFIDENCE_CLIENT_SECRET.set(var.to_string());
    } else {
        console_log!("no confidence client secret provided");
    }
}

fn init_resolve_token_key(env: &Env) {
    let _ = RESOLVE_TOKEN_KEY.get_or_init(|| {
        let s = env
            .secret("RESOLVE_TOKEN_ENCRYPTION_KEY")
            .map(|s| s.to_string())
            .or_else(|_| env.var("RESOLVE_TOKEN_ENCRYPTION_KEY").map(|v| v.to_string()))
            .expect("RESOLVE_TOKEN_ENCRYPTION_KEY is not configured");
        Bytes::from(
            STANDARD
                .decode(s.trim())
                .expect("RESOLVE_TOKEN_ENCRYPTION_KEY is not valid base64"),
        )
    });
}

fn resolve_token_key() -> Bytes {
    RESOLVE_TOKEN_KEY
        .get()
        .expect("RESOLVE_TOKEN_ENCRYPTION_KEY not initialized")
        .clone()
}

fn sdk_info() -> Sdk {
    Sdk {
        sdk: Some(confidence::flags::resolver::v1::sdk::Sdk::Id(
            confidence::flags::resolver::v1::SdkId::CloudflareResolver as i32,
        )),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Resolve flags with sticky assignment support via the suspend/resume cycle.
///
/// If the resolver suspends (needs materialization data), reads from KV and resumes.
/// Returns the resolved response and any materialization writes to persist, and
/// accumulates whatever the resolver logged into `log`.
async fn resolve_with_sticky(
    resolver: &AccountResolver<'_, H>,
    request: ResolveProcessRequest,
    kv: Option<&kv::KvStore>,
    log: &mut WriteFlagLogsRequest,
) -> std::result::Result<(ResolveFlagsResponse, Vec<MaterializationRecord>), String> {
    let response = with_log(log, || resolver.resolve_flags(request))?;

    match response.result {
        Some(resolve_process_response::Result::Resolved(r)) => Ok((
            r.response.ok_or("Empty resolve response")?,
            r.materializations_to_write,
        )),
        Some(resolve_process_response::Result::Suspended(s)) => {
            let kv = kv.ok_or("Materializations required but KV not available")?;
            let records =
                materialization::read_materializations(kv, &s.materializations_to_read).await;
            let resume = ResolveProcessRequest::resume(records, s.state);
            let resumed = with_log(log, || resolver.resolve_flags(resume))?;
            resumed
                .into_resolved()
                .ok_or_else(|| "Still suspended after resume".to_string())
        }
        None => Err("Empty process response".to_string()),
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

    match env.queue("events_queue") {
        Ok(queue) => {
            let _ = EVENTS_QUEUE.set(queue);
        }
        Err(_e) => {
            console_log!("events_queue binding is missing; event tracking disabled");
        }
    }

    set_client_secret(&env);
    init_resolve_token_key(&env);
    seed_resolver_rng();

    let allowed_origin_env = env
        .var("ALLOWED_ORIGIN")
        .map(|var| var.to_string())
        .unwrap_or("*".to_string()); // Fallback to "*" if the variable is not set

    // When true (the default), every resolve is treated as apply=true so
    // assignments are logged at resolve time. Deployments that want the
    // deferred-apply flow (SDKs resolving with apply=false and calling
    // flags:apply later) opt out by setting FORCE_APPLY = "false".
    let force_apply = env
        .var("FORCE_APPLY")
        .map(|var| !var.to_string().trim().eq_ignore_ascii_case("false"))
        .unwrap_or(true);

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

    let enable_apply_dedup = env
        .var("ENABLE_APPLY_DEDUP")
        .map(|var| var.to_string().trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    APPLY_DEDUP_ENABLED.with(|c| c.set(enable_apply_dedup));

    if req.method() == Method::Options {
        return Response::ok("")?.with_cors_headers(&allowed_origin_env);
    }

    let mat_ttl: Option<u64> = env
        .var("MATERIALIZATION_TTL_SECONDS")
        .ok()
        .and_then(|v| v.to_string().parse().ok());

    let state = &RESOLVER_STATE;
    let event_ctx = &ctx;
    let router = Router::new();

    router
        .get_async("/metrics", |req, ctx| {
            let allowed_origin = allowed_origin_env.clone();
            async move {
                // Require client secret — metrics are not public.
                if let Some(expected) = CONFIDENCE_CLIENT_SECRET.get() {
                    let authorized = req.headers().get("Authorization").ok().flatten()
                        .map(|v| v.strip_prefix("ClientSecret ").unwrap_or("") == expected.as_str())
                        .unwrap_or(false);
                    if !authorized {
                        return Response::error("Unauthorized", 401)?
                            .with_cors_headers(&allowed_origin);
                    }
                }
                // Rendered on read by summing the per-pipeline snapshot keys,
                // rather than served from a pre-rendered key that either
                // consumer could clobber.
                let body = match ctx.env.kv("CONFIDENCE_METRICS_KV") {
                    Ok(kv) => render_metrics(&kv).await,
                    Err(_) => String::new(),
                };
                let headers = Headers::new();
                headers.set("Content-Type", PROMETHEUS_CONTENT_TYPE)?;
                headers.set("Cache-Control", "no-store")?;
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
            // `event_ctx` is borrowed from `main`'s scope, like `state` above,
            // so each handler schedules its own post-response work directly
            // rather than parking it somewhere shared for `main` to pick up.
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
                        if force_apply {
                            resolver_request.apply = true;
                        }

                        let encryption_key = resolve_token_key();
                        let evaluation_context = resolver_request
                            .evaluation_context
                            .clone()
                            .unwrap_or_default();

                        // Start timer before resolve. CF Workers freeze timers
                        // during sync CPU, but scheduler.wait(0) unfreezes them.
                        let t0 = js_sys::Date::now();

                        let mat_kv = ctx.env.kv("CONFIDENCE_MATERIALIZATIONS_KV").ok();

                        let mut log = WriteFlagLogsRequest::default();
                        let (reasons, resp) = match state.get_resolver::<H>(
                            &resolver_request.client_secret,
                            evaluation_context,
                            &encryption_key,
                        ) {
                            Ok(resolver) => {
                                let process_request = if mat_kv.is_some() {
                                    ResolveProcessRequest::deferred_materializations(
                                        resolver_request,
                                    )
                                } else {
                                    ResolveProcessRequest::without_materializations(
                                        resolver_request,
                                    )
                                };
                                match resolve_with_sticky(
                                    &resolver, process_request, mat_kv.as_ref(), &mut log,
                                ).await {
                                    Ok((response, writes)) => {
                                        // Write sticky assignments to KV
                                        // without blocking the response.
                                        if !writes.is_empty() {
                                            if let Some(kv) = mat_kv.clone() {
                                                event_ctx.wait_until(async move {
                                                    materialization::write_materializations(
                                                        &kv, &writes, mat_ttl,
                                                    )
                                                    .await;
                                                });
                                            }
                                        }
                                        let reasons: Vec<ResolveReason> = response
                                            .resolved_flags
                                            .iter()
                                            .map(|f| f.reason())
                                            .collect();
                                        (reasons, Response::from_json(&response)?
                                            .with_cors_headers(&allowed_origin))
                                    }
                                    Err(msg) => {
                                        (vec![ResolveReason::Error],
                                        Response::error(msg, 500)?
                                            .with_cors_headers(&allowed_origin))
                                    }
                                }
                            }
                            Err(msg) => {
                                (vec![ResolveReason::Error],
                                Response::error(msg, 500)?.with_cors_headers(&allowed_origin))
                            }
                        };

                        let elapsed_us = {
                            let scheduler = js_sys::Reflect::get(
                                &js_sys::global(), &wasm_bindgen::JsValue::from_str("scheduler")
                            ).unwrap_or(wasm_bindgen::JsValue::UNDEFINED);
                            if !scheduler.is_undefined() {
                                let wait = js_sys::Reflect::get(
                                    &scheduler, &wasm_bindgen::JsValue::from_str("wait")
                                ).unwrap_or(wasm_bindgen::JsValue::UNDEFINED);
                                if let Ok(func) = wait.dyn_into::<js_sys::Function>() {
                                    if let Ok(ret) = func.call1(&scheduler, &wasm_bindgen::JsValue::from(0)) {
                                        if let Ok(promise) = ret.dyn_into::<js_sys::Promise>() {
                                            let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
                                        }
                                    }
                                }
                                Some(((js_sys::Date::now() - t0) * 1000.0).max(0.0) as u32)
                            } else {
                                None
                            }
                        };

                        let mut td = telemetry::build_request_telemetry(elapsed_us, &reasons);
                        td.sdk = Some(sdk_info());
                        td.apply_dedup = dedup_telemetry_delta();
                        log.telemetry_data = Some(td);
                        event_ctx.wait_until(queue_flag_log(log));

                        resp
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

                        // SDKs that resolved with apply=true send a background
                        // apply with an empty token — nothing to do.
                        if apply_flag_req.resolve_token.is_empty() {
                            return Response::from_json(&ApplyFlagsResponse::default())?
                                .with_cors_headers(&allowed_origin);
                        }

                        let encryption_key = resolve_token_key();
                        let mut log = WriteFlagLogsRequest::default();
                        let resp = match state.get_resolver::<H>(
                            &apply_flag_req.client_secret,
                            Struct::default(),
                            &encryption_key,
                        ) {
                            Ok(resolver) => {
                                match with_log(&mut log, || resolver.apply_flags(&apply_flag_req)) {
                                    Ok(()) => Response::from_json(&ApplyFlagsResponse::default()),
                                    Err(msg) => Response::error(msg, 500)?
                                        .with_cors_headers(&allowed_origin),
                                }
                            }
                            Err(msg) => {
                                Response::error(msg, 500)?.with_cors_headers(&allowed_origin)
                            }
                        };
                        if let Some(dedup_delta) = dedup_telemetry_delta() {
                            let td = log.telemetry_data.get_or_insert_with(Default::default);
                            td.apply_dedup = Some(dedup_delta);
                        }
                        if log != WriteFlagLogsRequest::default() {
                            event_ctx.wait_until(queue_flag_log(log));
                        }
                        resp
                    }
                    "telemetry:upload" => {
                        Response::ok("")?.with_cors_headers(&allowed_origin)
                    }
                    "events:publish" => {
                        // Read every header we need up front so the immutable
                        // borrow of `req` ends before `req.bytes()` takes it
                        // mutably.
                        let (is_protobuf, header_secret) = {
                            let h = req.headers();
                            (
                                h.get("Content-Type")
                                    .ok()
                                    .flatten()
                                    .unwrap_or_default()
                                    .contains("protobuf"),
                                h.get("Authorization").ok().flatten().and_then(|v| {
                                    v.strip_prefix("ClientSecret ").map(|s| s.to_string())
                                }),
                            )
                        };

                        let expected = CONFIDENCE_CLIENT_SECRET.get();

                        // Fast path: a caller that sends the secret as an
                        // `Authorization: ClientSecret ...` header (the
                        // convention /metrics already uses) is authorized
                        // without the body being read at all.
                        let authorized_by_header = match (&header_secret, expected) {
                            (Some(got), Some(exp)) => {
                                if got != exp {
                                    return Response::error("Unauthorized", 401)?
                                        .with_cors_headers(&allowed_origin);
                                }
                                true
                            }
                            _ => false,
                        };

                        let body_bytes: Vec<u8> = req.bytes().await?;

                        // Otherwise the secret is a body field, so it can't be
                        // checked without touching the body — but it can be
                        // checked without decoding the events. `probe_client_secret`
                        // reads only that one field, so an unauthorized caller
                        // never pays for event materialization.
                        if !authorized_by_header {
                            if let Some(exp) = expected {
                                let probed =
                                    match probe_client_secret(&body_bytes, is_protobuf) {
                                        Ok(s) => s,
                                        Err(msg) => {
                                            return Response::error(
                                                format!("Invalid request payload: {}", msg),
                                                400,
                                            )?
                                            .with_cors_headers(&allowed_origin);
                                        }
                                    };
                                if probed != *exp {
                                    return Response::error("Unauthorized", 401)?
                                        .with_cors_headers(&allowed_origin);
                                }
                            }
                        }

                        let queue = match EVENTS_QUEUE.get() {
                            Some(q) => q,
                            None => {
                                return Response::error(
                                    "Event tracking not available",
                                    503,
                                )?
                                .with_cors_headers(&allowed_origin);
                            }
                        };

                        // Authorized: now decode the events themselves. The
                        // inbound secret is not carried forward — the queue
                        // consumer re-signs the batch with the worker's own
                        // configured secret.
                        let events = match parse_events(&body_bytes, is_protobuf) {
                            Ok(e) => e,
                            Err(msg) => {
                                return Response::error(
                                    format!("Invalid request payload: {}", msg),
                                    400,
                                )?
                                .with_cors_headers(&allowed_origin);
                            }
                        };

                        if !events.is_empty() {
                            let json = serde_json::to_string(&events)
                                .map_err(|e| {
                                    worker::Error::RustError(format!(
                                        "event serialize failed: {}",
                                        e
                                    ))
                                })?;
                            queue.send(json).await.map_err(|e| {
                                console_log!("event queue send failed: {:?}", e);
                                e
                            })?;
                        }

                        Response::from_json(&json!({"errors": []}))?
                            .with_cors_headers(&allowed_origin)
                    }
                    _ => Response::error("Not found", 404)?.with_cors_headers(&allowed_origin),
                }
            }
        })
        .run(req, env)
        .await
}

#[event(queue)]
pub async fn consume_queue(
    message_batch: MessageBatch<String>,
    env: Env,
    _ctx: Context,
) -> Result<()> {
    set_client_secret(&env);
    seed_resolver_rng();

    let queue_name = message_batch.queue();
    if queue_name.ends_with("events-queue") {
        return consume_events_queue(message_batch, env).await;
    }

    consume_flag_logs(message_batch, env).await
}

async fn consume_flag_logs(
    message_batch: MessageBatch<String>,
    env: Env,
) -> Result<()> {
    if let Ok(messages) = message_batch.messages() {
        // A message that fails to parse is skipped instead of panicking the
        // whole batch (a panic would retry and eventually drop all of it).
        let logs: Vec<WriteFlagLogsRequest> = messages
            .iter()
            .map(|m| m.body().clone())
            .filter_map(
                |s| match serde_json::from_str::<WriteFlagLogsRequest>(s.as_str()) {
                    Ok(log) => Some(log),
                    Err(e) => {
                        console_log!("flag log message parse failed, skipping: {:?}", e);
                        None
                    }
                },
            )
            .collect();

        let req = flag_logger::aggregate_batch(logs);

        let client_secret = CONFIDENCE_CLIENT_SECRET.get().unwrap().as_str();
        let account_id = CDN_STATE_REQUEST.account_id.as_str();
        let destinations = &*LOG_DESTINATIONS;

        let (primary, fallback) = if destinations.len() >= 2 {
            (destinations[0], Some(destinations[1]))
        } else {
            (destinations[0], None)
        };

        let delivered = if let Err(reason) =
            deliver_flag_logs(client_secret, account_id, &req, primary).await
        {
            console_log!(
                "flag log delivery to {:?} failed ({}), trying fallback",
                primary,
                reason
            );
            match fallback {
                Some(fb) => match deliver_flag_logs(client_secret, account_id, &req, fb).await {
                    Ok(()) => true,
                    Err(fb_reason) => {
                        console_log!(
                            "fallback flag log delivery to {:?} also failed: {}",
                            fb,
                            fb_reason
                        );
                        false
                    }
                },
                None => false,
            }
        } else {
            true
        };

        if let Ok(kv) = env.kv("CONFIDENCE_METRICS_KV") {
            update_kv_snapshot(
                &kv,
                SnapshotPipeline::FlagLogs,
                request_telemetry_to_accumulate(req.telemetry_data.as_ref(), delivered),
                Some(delivered),
                None,
            )
            .await;
        }

        if !delivered {
            return Err(worker::Error::RustError(
                "flag log delivery failed on all destinations".to_string(),
            ));
        }
    }

    Ok(())
}

/// Attempt delivery to one destination. Any transport error or non-2xx/3xx
/// response counts as a failure.
async fn deliver_flag_logs(
    client_secret: &str,
    account_id: &str,
    req: &WriteFlagLogsRequest,
    dest: LogDestination,
) -> std::result::Result<(), String> {
    let url = log_destination_url(&dest);
    let acct = match dest {
        LogDestination::Edge => None,
        _ => Some(account_id),
    };
    match send_flags_logs(client_secret, req, url, acct).await {
        Ok(resp) if resp.status_code() < 400 => Ok(()),
        Ok(resp) => Err(format!("HTTP {}", resp.status_code())),
        Err(e) => Err(format!("{:?}", e)),
    }
}

/// KV key accumulated exclusively by the flag-log queue consumer.
const SNAPSHOT_KEY_FLAG_LOGS: &str = "snapshot:flaglogs";
/// KV key accumulated exclusively by the events queue consumer.
const SNAPSHOT_KEY_EVENTS: &str = "snapshot:events";
/// Pre-split key, written by earlier deployments. Read-only from now on: its
/// COUNTERS are folded into `/metrics` so already-accumulated totals are not
/// lost, but it is never written again, so it stays frozen at its final
/// pre-upgrade value. Its gauges are therefore ignored — see [`GaugeSource`].
const SNAPSHOT_KEY_LEGACY: &str = "snapshot";

/// Whether a source snapshot may supply gauge readings.
///
/// Counters always accumulate, from every source. Gauges are point-in-time
/// readings, so only a live pipeline may supply one: the legacy key is frozen
/// at its final pre-upgrade value, and letting it win a "latest reading"
/// contest would pin `memory_bytes` and `map_size` to stale values forever —
/// including preventing `map_size` from ever being observed reaching 0, which
/// would defeat the sweep reporting it is meant to expose.
///
/// This is expressed as a parameter rather than relying on merge ORDER so the
/// rule cannot be silently undone by reordering the keys in `render_metrics`.
#[derive(Copy, Clone, PartialEq, Eq)]
enum GaugeSource {
    /// A live pipeline's own key: its gauges are current readings.
    Live,
    /// A frozen key: fold in the counters, ignore the gauges.
    Frozen,
}

/// Which pipeline is reporting, and therefore which KV key it owns.
///
/// The two queue consumers accumulate into separate keys so they cannot clobber
/// one another: a shared key means both can read the same version and the last
/// writer wins. `/metrics` merges the keys at read time instead.
#[derive(Copy, Clone)]
enum SnapshotPipeline {
    FlagLogs,
    Events,
}

impl SnapshotPipeline {
    fn key(self) -> &'static str {
        match self {
            SnapshotPipeline::FlagLogs => SNAPSHOT_KEY_FLAG_LOGS,
            SnapshotPipeline::Events => SNAPSHOT_KEY_EVENTS,
        }
    }
}

/// A request's own telemetry deltas are accumulated only when delivery
/// succeeded.
///
/// A failed batch makes the consumer return `Err`, which tells Cloudflare
/// Queues to redeliver it. Folding the deltas in on a failed attempt would
/// therefore count them again on every retry, multiplying apply-dedup, latency,
/// resolve rates and provider-init by the attempt count. The flush
/// success/failure counter is still recorded per attempt — that one is meant to
/// count attempts.
fn request_telemetry_to_accumulate(
    telemetry: Option<&confidence_resolver::proto::confidence::flags::resolver::v1::TelemetryData>,
    delivered: bool,
) -> Option<&confidence_resolver::proto::confidence::flags::resolver::v1::TelemetryData> {
    if delivered {
        telemetry
    } else {
        None
    }
}

/// Sums two cumulative snapshots.
///
/// Lives here rather than on `TelemetrySnapshot` because only the Cloudflare
/// worker splits its accumulation across keys.
///
/// MAINTENANCE: every field of `TelemetrySnapshot` must be handled here. A new
/// field that is not added will silently read as zero on `/metrics` for one of
/// the two pipelines.
fn merge_snapshots(
    mut acc: TelemetrySnapshot,
    other: &TelemetrySnapshot,
    gauges: GaugeSource,
) -> TelemetrySnapshot {
    // Latency histogram: sums add, buckets add index-wise (the wider of the two wins).
    acc.latency.sum = acc.latency.sum.wrapping_add(other.latency.sum);
    acc.latency.count = acc.latency.count.wrapping_add(other.latency.count);
    if acc.latency.buckets.len() < other.latency.buckets.len() {
        acc.latency.buckets.resize(other.latency.buckets.len(), 0);
    }
    for (slot, add) in acc.latency.buckets.iter_mut().zip(&other.latency.buckets) {
        *slot = slot.wrapping_add(*add);
    }

    // Resolve rates are indexed by reason.
    if acc.resolve_rates.len() < other.resolve_rates.len() {
        acc.resolve_rates.resize(other.resolve_rates.len(), 0);
    }
    for (slot, add) in acc.resolve_rates.iter_mut().zip(&other.resolve_rates) {
        *slot = slot.wrapping_add(*add);
    }

    // Gauge: keep whichever LIVE pipeline reported a value. A frozen source
    // must not supply it, or its stale reading would win permanently.
    if gauges == GaugeSource::Live && other.memory_bytes > 0 {
        acc.memory_bytes = other.memory_bytes;
    }

    acc.flush.succeeded = acc.flush.succeeded.wrapping_add(other.flush.succeeded);
    acc.flush.failed = acc.flush.failed.wrapping_add(other.flush.failed);

    acc.events.published = acc.events.published.wrapping_add(other.events.published);
    acc.events.batches_succeeded = acc
        .events
        .batches_succeeded
        .wrapping_add(other.events.batches_succeeded);
    acc.events.batches_failed = acc
        .events
        .batches_failed
        .wrapping_add(other.events.batches_failed);
    acc.events.events_rejected = acc
        .events
        .events_rejected
        .wrapping_add(other.events.events_rejected);

    match (&mut acc.apply_dedup, &other.apply_dedup) {
        (Some(a), Some(b)) => {
            // Counters add; map_size/map_capacity are point-in-time gauges, so
            // only a live source may update them.
            a.applies_total = a.applies_total.wrapping_add(b.applies_total);
            a.applies_deduped = a.applies_deduped.wrapping_add(b.applies_deduped);
            a.apply_dedup_overflow = a.apply_dedup_overflow.wrapping_add(b.apply_dedup_overflow);
            a.sweeps = a.sweeps.wrapping_add(b.sweeps);
            if gauges == GaugeSource::Live {
                a.map_size = b.map_size;
                a.map_capacity = b.map_capacity;
            }
        }
        (None, Some(b)) => {
            // Adopting wholesale would also adopt the gauges, so a frozen
            // source contributes its counters with the gauges zeroed.
            let mut adopted = b.clone();
            if gauges == GaugeSource::Frozen {
                adopted.map_size = 0;
                adopted.map_capacity = 0;
            }
            acc.apply_dedup = Some(adopted);
        }
        _ => {}
    }

    // Provider init counts are keyed by label set, so entries are matched on
    // their labels rather than by position. Kept sorted by labels to match
    // `accumulate_delta`/`merge_telemetry`, which the Prometheus output and the
    // serialized snapshot both rely on for determinism.
    for add in &other.provider_init_rate {
        match acc
            .provider_init_rate
            .iter_mut()
            .find(|existing| existing.labels == add.labels)
        {
            Some(existing) => existing.count = existing.count.wrapping_add(add.count),
            None => acc.provider_init_rate.push(add.clone()),
        }
    }
    acc.provider_init_rate
        .sort_by(|a, b| a.labels.cmp(&b.labels));

    acc
}

/// Reads and sums every snapshot key, rendering the Prometheus exposition.
async fn render_metrics(kv: &kv::KvStore) -> String {
    let mut total = TelemetrySnapshot::default();
    for (key, gauges) in [
        (SNAPSHOT_KEY_FLAG_LOGS, GaugeSource::Live),
        (SNAPSHOT_KEY_EVENTS, GaugeSource::Live),
        (SNAPSHOT_KEY_LEGACY, GaugeSource::Frozen),
    ] {
        if let Ok(Some(text)) = kv.get(key).text().await {
            if let Ok(part) = serde_json::from_str::<TelemetrySnapshot>(&text) {
                total = merge_snapshots(total, &part, gauges);
            }
        }
    }
    total.to_prometheus(
        "cf-resolver",
        &confidence_resolver::telemetry::PrometheusConfig::default(),
    )
}

/// Read-modify-write of one pipeline's cumulative telemetry snapshot.
///
/// Each pipeline owns its own KV key, so the flag-log and events consumers can
/// no longer overwrite each other. `/metrics` sums the keys at read time.
///
/// Concurrency that REMAINS: two concurrent invocations of the *same* consumer
/// still race on this read-modify-write, so one batch's deltas can be lost.
/// Accepted for metrics — the loss is bounded to a single batch and does not
/// corrupt cumulative state. KV offers no compare-and-swap to close this.
///
/// `event_result` is `(published, rejected, succeeded)`, where `published` is
/// already net of the events the service refused.
async fn update_kv_snapshot(
    kv: &kv::KvStore,
    pipeline: SnapshotPipeline,
    telemetry_delta: Option<&confidence_resolver::proto::confidence::flags::resolver::v1::TelemetryData>,
    flush_result: Option<bool>,
    event_result: Option<(u64, u64, bool)>,
) {
    let key = pipeline.key();
    let mut cumulative = match kv.get(key).text().await {
        Ok(Some(text)) => serde_json::from_str::<TelemetrySnapshot>(&text).unwrap_or_default(),
        _ => TelemetrySnapshot::default(),
    };

    if let Some(td) = telemetry_delta {
        cumulative.accumulate_delta(td);
    }

    match flush_result {
        Some(true) => {
            cumulative.flush.succeeded = cumulative.flush.succeeded.wrapping_add(1);
        }
        Some(false) => {
            cumulative.flush.failed = cumulative.flush.failed.wrapping_add(1);
        }
        None => {}
    }

    if let Some((event_count, rejected, succeeded)) = event_result {
        if succeeded {
            cumulative.events.published = cumulative.events.published.wrapping_add(event_count);
            cumulative.events.events_rejected =
                cumulative.events.events_rejected.wrapping_add(rejected);
            cumulative.events.batches_succeeded =
                cumulative.events.batches_succeeded.wrapping_add(1);
        } else {
            cumulative.events.batches_failed = cumulative.events.batches_failed.wrapping_add(1);
        }
    }

    if let Ok(builder) = kv.put(key, serde_json::to_string(&cumulative).unwrap_or_default()) {
        let _ = builder.execute().await;
    }
}

fn log_destination_url(dest: &LogDestination) -> &'static str {
    match dest {
        LogDestination::Edge => "https://resolver.confidence.dev/v1/clientFlagLogs:write",
        LogDestination::Cloudflare => "https://epx-flags-logs.experimentation-platform.workers.dev/v1/flagLogs:ingest",
    }
}

/// Request wrapper expected by the Cloudflare ingest worker. The `batch`
/// field holds an already-encoded `WriteFlagLogsRequest` — a length-delimited
/// bytes field is wire-identical to a nested message field, so this avoids
/// re-encoding the batch into an owned message.
#[derive(Clone, PartialEq, Message)]
struct IngestFlagLogsRequest {
    #[prost(string, tag = "1")]
    account_id: String,
    #[prost(bytes = "vec", tag = "2")]
    batch: Vec<u8>,
}

/// Send a flag log batch to `destination_url`.
///
/// The Edge endpoint accepts the `WriteFlagLogsRequest` batch directly as
/// JSON. The Cloudflare ingest worker instead expects an
/// `IngestFlagLogsRequest` protobuf that adds the `account_id`, which the
/// ingestor uses to partition storage per account.
async fn send_flags_logs(
    client_secret: &str,
    message: &WriteFlagLogsRequest,
    destination_url: &str,
    account_id: Option<&str>,
) -> Result<Response> {
    let mut init = RequestInit::new();
    let headers = Headers::new();
    headers.set("Authorization", &format!("ClientSecret {}", client_secret))?;
    init.with_method(Method::Post);

    if let Some(account) = account_id {
        let body = IngestFlagLogsRequest {
            account_id: account.to_string(),
            batch: message.encode_to_vec(),
        }
        .encode_to_vec();

        headers.set("Content-Type", "application/protobuf")?;
        init.with_headers(headers);
        init.with_body(Some(body.into()));
    } else {
        // Edge: send as JSON
        headers.set("Content-Type", "application/json")?;
        init.with_headers(headers);
        let json = serde_json::to_string(message)?;
        init.with_body(Some(json.into()));
    }

    let request = Request::new_with_init(destination_url, &init)?;
    Fetch::Request(request).send().await
}

const EVENTS_URL: &str = "https://events.confidence.dev/v1/events:publish";

/// Minimal prost type for decoding the protobuf `PublishEventsRequest`.
/// Fields we don't need (send_time, sdk) are skipped by prost.
#[derive(Clone, PartialEq, Message)]
struct ProtoPublishEventsRequest {
    #[prost(string, tag = "1")]
    client_secret: String,
    #[prost(message, repeated, tag = "2")]
    events: Vec<ProtoEvent>,
}

/// Decodes only `client_secret` (field 1). prost walks past the repeated
/// `events` field without constructing any `Event`, so authorizing a request
/// this way costs a buffer scan rather than a full decode.
#[derive(Clone, PartialEq, Message)]
struct ProtoClientSecretProbe {
    #[prost(string, tag = "1")]
    client_secret: String,
}

/// JSON counterpart to `ProtoClientSecretProbe`. serde skips the `events`
/// array without building a `Value` tree for it.
#[derive(serde::Deserialize)]
struct JsonClientSecretProbe {
    #[serde(rename = "clientSecret", alias = "client_secret", default)]
    client_secret: String,
}

#[derive(Clone, PartialEq, Message, serde::Serialize)]
struct ProtoEvent {
    #[prost(string, tag = "1")]
    #[serde(rename = "eventDefinition")]
    event_definition: String,
    #[prost(message, optional, tag = "2")]
    payload: Option<pbjson_types::Struct>,
    #[prost(message, optional, tag = "3")]
    #[serde(rename = "eventTime")]
    event_time: Option<pbjson_types::Timestamp>,
}

/// Extracts just the `client_secret` so the request can be authorized before
/// the events are decoded. See `ProtoClientSecretProbe`.
fn probe_client_secret(
    body: &[u8],
    is_protobuf: bool,
) -> std::result::Result<String, String> {
    if is_protobuf {
        ProtoClientSecretProbe::decode(body)
            .map(|p| p.client_secret)
            .map_err(|e| format!("Invalid protobuf: {}", e))
    } else {
        serde_json::from_slice::<JsonClientSecretProbe>(body)
            .map(|p| p.client_secret)
            .map_err(|e| format!("Invalid JSON: {}", e))
    }
}

/// Decodes the events, normalized to the proto3-JSON shape the queue consumer
/// forwards. Only called once the request is authorized.
fn parse_events(
    body: &[u8],
    is_protobuf: bool,
) -> std::result::Result<Vec<serde_json::Value>, String> {
    if is_protobuf {
        let req = ProtoPublishEventsRequest::decode(body)
            .map_err(|e| format!("Invalid protobuf: {}", e))?;
        Ok(req
            .events
            .iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect())
    } else {
        let req: serde_json::Value =
            serde_json::from_slice(body).map_err(|e| format!("Invalid JSON: {}", e))?;
        Ok(req
            .get("events")
            .and_then(|e| e.as_array())
            .cloned()
            .unwrap_or_default())
    }
}

fn aggregate_events(messages: &[String]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .filter_map(|s| serde_json::from_str::<Vec<serde_json::Value>>(s).ok())
        .flatten()
        .collect()
}

fn build_publish_events_request(
    client_secret: &str,
    events: Vec<serde_json::Value>,
    send_time: &str,
) -> serde_json::Value {
    json!({
        "clientSecret": client_secret,
        "events": events,
        "sendTime": send_time,
        "sdk": {
            "id": "SDK_ID_CLOUDFLARE_RESOLVER",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

async fn consume_events_queue(
    message_batch: MessageBatch<String>,
    env: Env,
) -> Result<()> {
    let messages = message_batch.messages()?;
    let raw: Vec<String> = messages.iter().map(|m| m.body().clone()).collect();
    let all_events = aggregate_events(&raw);

    if all_events.is_empty() {
        return Ok(());
    }

    let event_count = all_events.len() as u64;

    let client_secret = CONFIDENCE_CLIENT_SECRET
        .get()
        .ok_or_else(|| worker::Error::RustError("client secret not configured".into()))?;

    let now = js_sys::Date::new_0().to_iso_string();
    let publish_request = build_publish_events_request(
        client_secret.as_str(),
        all_events,
        &now.as_string().unwrap_or_default(),
    );

    let (delivered, rejected) = match send_events(&publish_request).await {
        Ok(mut resp) if resp.status_code() < 400 => {
            // The events API answers 2xx even when it refuses individual
            // events; those come back in an "errors" array.
            let rejected = resp
                .text()
                .await
                .ok()
                .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
                .and_then(|v| {
                    v.get("errors")
                        .and_then(|e| e.as_array())
                        .map(|a| a.len() as u64)
                })
                .unwrap_or(0);
            (true, rejected)
        }
        Ok(resp) => {
            console_log!("events delivery failed: HTTP {}", resp.status_code());
            (false, 0)
        }
        Err(e) => {
            console_log!("events delivery error: {:?}", e);
            (false, 0)
        }
    };

    if let Ok(kv) = env.kv("CONFIDENCE_METRICS_KV") {
        update_kv_snapshot(
            &kv,
            SnapshotPipeline::Events,
            None,
            None,
            Some((event_count.saturating_sub(rejected), rejected, delivered)),
        )
        .await;
    }

    if !delivered {
        return Err(worker::Error::RustError(
            "events delivery failed".to_string(),
        ));
    }

    Ok(())
}


async fn send_events(body: &serde_json::Value) -> Result<Response> {
    let mut init = RequestInit::new();
    let headers = Headers::new();
    headers.set("Content-Type", "application/json")?;
    init.with_method(Method::Post);
    init.with_headers(headers);
    init.with_body(Some(serde_json::to_string(body)?.into()));

    let request = Request::new_with_init(EVENTS_URL, &init)?;
    Fetch::Request(request).send().await
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

#[cfg(test)]
mod snapshot_merge_tests {
    use super::*;
    use confidence_resolver::apply_dedup::ApplyDedupSnapshot;

    /// Each pipeline owns a key, so `/metrics` must sum them rather than pick one.
    #[test]
    fn merges_counters_from_both_pipelines() {
        let flag_logs = TelemetrySnapshot {
            flush: confidence_resolver::telemetry::FlushSnapshot {
                succeeded: 7,
                failed: 2,
            },
            ..Default::default()
        };
        let mut events = TelemetrySnapshot::default();
        events.events.published = 50;
        events.events.batches_succeeded = 3;
        events.events.batches_failed = 1;
        events.events.events_rejected = 4;

        let merged = merge_snapshots(
            merge_snapshots(TelemetrySnapshot::default(), &flag_logs, GaugeSource::Live),
            &events,
            GaugeSource::Live,
        );

        assert_eq!(merged.flush.succeeded, 7, "flush from the flag-log key was lost");
        assert_eq!(merged.flush.failed, 2);
        assert_eq!(merged.events.published, 50, "events from the events key were lost");
        assert_eq!(merged.events.batches_succeeded, 3);
        assert_eq!(merged.events.batches_failed, 1);
        assert_eq!(merged.events.events_rejected, 4);
    }

    /// A failed batch is redelivered by Queues, so its telemetry must not be
    /// accumulated on the failing attempt — otherwise a fail-then-succeed
    /// sequence counts those deltas once per attempt.
    #[test]
    fn request_telemetry_is_accumulated_only_on_successful_delivery() {
        use confidence_resolver::proto::confidence::flags::resolver::v1::TelemetryData;

        let td = TelemetryData {
            memory_bytes: 4096,
            ..Default::default()
        };

        assert!(
            request_telemetry_to_accumulate(Some(&td), true).is_some(),
            "a delivered batch must have its telemetry accumulated"
        );
        assert!(
            request_telemetry_to_accumulate(Some(&td), false).is_none(),
            "a failed batch is retried, so accumulating now double-counts it"
        );
        // No telemetry on the request is simply nothing to accumulate.
        assert!(request_telemetry_to_accumulate(None, true).is_none());
    }

    /// provider_init_rate is keyed by label set, so a shared label set must
    /// accumulate across pipelines while distinct ones stay separate. Without
    /// a provider_init_rate arm in merge_snapshots this reads as zero for
    /// whichever pipeline is merged second.
    #[test]
    fn provider_init_rate_accumulates_per_label_set_across_pipelines() {
        use confidence_resolver::telemetry::ProviderInitSnapshot;
        use std::collections::BTreeMap;

        let labels = |k: &str, v: &str| {
            let mut m = BTreeMap::new();
            m.insert(k.to_string(), v.to_string());
            m
        };

        let flag_logs = TelemetrySnapshot {
            provider_init_rate: vec![
                ProviderInitSnapshot {
                    labels: labels("encryption", "true"),
                    count: 2,
                },
                ProviderInitSnapshot {
                    labels: labels("encryption", "false"),
                    count: 1,
                },
            ],
            ..Default::default()
        };
        let events = TelemetrySnapshot {
            provider_init_rate: vec![ProviderInitSnapshot {
                labels: labels("encryption", "true"),
                count: 5,
            }],
            ..Default::default()
        };

        let merged = merge_snapshots(
            merge_snapshots(TelemetrySnapshot::default(), &flag_logs, GaugeSource::Live),
            &events,
            GaugeSource::Live,
        );

        assert_eq!(
            merged.provider_init_rate.len(),
            2,
            "distinct label sets must not be collapsed"
        );
        // Sorted by labels, so "false" precedes "true".
        assert_eq!(
            merged.provider_init_rate[0].labels,
            labels("encryption", "false")
        );
        assert_eq!(merged.provider_init_rate[0].count, 1);
        assert_eq!(
            merged.provider_init_rate[1].labels,
            labels("encryption", "true")
        );
        assert_eq!(
            merged.provider_init_rate[1].count, 7,
            "provider_init_rate was dropped or not accumulated across pipeline keys"
        );
    }

    /// Counters accumulate across keys; map_size/map_capacity are gauges.
    #[test]
    fn apply_dedup_counters_add_but_gauges_do_not() {
        let mut a = TelemetrySnapshot::default();
        a.apply_dedup = Some(ApplyDedupSnapshot {
            applies_total: 10,
            applies_deduped: 4,
            apply_dedup_overflow: 1,
            sweeps: 2,
            map_size: 100,
            map_capacity: 500,
        });
        let mut b = TelemetrySnapshot::default();
        b.apply_dedup = Some(ApplyDedupSnapshot {
            applies_total: 5,
            applies_deduped: 3,
            apply_dedup_overflow: 2,
            sweeps: 1,
            map_size: 120,
            map_capacity: 500,
        });

        let merged = merge_snapshots(a, &b, GaugeSource::Live);
        let ad = merged.apply_dedup.expect("apply_dedup dropped by merge");

        assert_eq!(ad.applies_total, 15);
        assert_eq!(ad.applies_deduped, 7);
        assert_eq!(ad.apply_dedup_overflow, 3);
        assert_eq!(ad.sweeps, 3);
        assert_eq!(ad.map_size, 120, "gauge must not be summed");
        assert_eq!(ad.map_capacity, 500, "gauge must not be summed");
    }

    /// A missing accumulator side must adopt the other's apply_dedup.
    #[test]
    fn adopts_apply_dedup_when_accumulator_has_none() {
        let mut b = TelemetrySnapshot::default();
        b.apply_dedup = Some(ApplyDedupSnapshot {
            applies_total: 9,
            ..Default::default()
        });
        let merged = merge_snapshots(TelemetrySnapshot::default(), &b, GaugeSource::Live);
        assert_eq!(merged.apply_dedup.expect("not adopted").applies_total, 9);
    }

    /// Histograms and rate vectors of differing width must not panic or truncate.
    #[test]
    fn histogram_and_rates_merge_across_differing_widths() {
        let a = TelemetrySnapshot {
            latency: confidence_resolver::telemetry::HistogramSnapshot {
                sum: 100,
                count: 2,
                buckets: vec![1, 2],
            },
            resolve_rates: vec![5],
            ..Default::default()
        };
        let b = TelemetrySnapshot {
            latency: confidence_resolver::telemetry::HistogramSnapshot {
                sum: 50,
                count: 1,
                buckets: vec![3, 4, 5],
            },
            resolve_rates: vec![1, 7],
            ..Default::default()
        };

        let merged = merge_snapshots(a, &b, GaugeSource::Live);
        assert_eq!(merged.latency.sum, 150);
        assert_eq!(merged.latency.count, 3);
        assert_eq!(merged.latency.buckets, vec![4, 6, 5]);
        assert_eq!(merged.resolve_rates, vec![6, 7]);
    }

    /// memory_bytes is a gauge: a reporting pipeline wins, a silent one does not zero it.
    #[test]
    fn memory_gauge_prefers_a_reported_value() {
        let a = TelemetrySnapshot {
            memory_bytes: 4096,
            ..Default::default()
        };
        let merged = merge_snapshots(a, &TelemetrySnapshot::default(), GaugeSource::Live);
        assert_eq!(merged.memory_bytes, 4096, "silent pipeline zeroed the gauge");
    }

    /// The legacy key is frozen at its final pre-upgrade value, so it must
    /// contribute counters but never gauges — otherwise its stale readings win
    /// every subsequent scrape and `map_size` can never be seen reaching 0.
    #[test]
    fn frozen_legacy_contributes_counters_but_never_gauges() {
        // Live pipeline: current readings, mid-flight counters.
        let live = TelemetrySnapshot {
            memory_bytes: 200_000_000,
            flush: confidence_resolver::telemetry::FlushSnapshot {
                succeeded: 3,
                failed: 1,
            },
            apply_dedup: Some(ApplyDedupSnapshot {
                applies_total: 7,
                applies_deduped: 2,
                apply_dedup_overflow: 0,
                sweeps: 4,
                map_size: 100,
                map_capacity: 500,
            }),
            ..Default::default()
        };
        // Legacy key: frozen at pre-upgrade values, deliberately much larger.
        let legacy = TelemetrySnapshot {
            memory_bytes: 500_000_000,
            flush: confidence_resolver::telemetry::FlushSnapshot {
                succeeded: 10,
                failed: 5,
            },
            apply_dedup: Some(ApplyDedupSnapshot {
                applies_total: 50,
                applies_deduped: 20,
                apply_dedup_overflow: 3,
                sweeps: 8,
                map_size: 9000,
                map_capacity: 9000,
            }),
            ..Default::default()
        };

        let merged = merge_snapshots(
            merge_snapshots(TelemetrySnapshot::default(), &live, GaugeSource::Live),
            &legacy,
            GaugeSource::Frozen,
        );

        // Gauges: the live reading must survive the frozen source.
        assert_eq!(
            merged.memory_bytes, 200_000_000,
            "frozen legacy memory_bytes clobbered the live reading"
        );
        let ad = merged.apply_dedup.expect("apply_dedup dropped by merge");
        assert_eq!(
            ad.map_size, 100,
            "frozen legacy map_size clobbered the live reading"
        );
        assert_eq!(
            ad.map_capacity, 500,
            "frozen legacy map_capacity clobbered the live reading"
        );

        // Counters: still summed across both sources.
        assert_eq!(merged.flush.succeeded, 13, "legacy flush counters were lost");
        assert_eq!(merged.flush.failed, 6, "legacy flush counters were lost");
        assert_eq!(ad.applies_total, 57, "legacy apply counters were lost");
        assert_eq!(ad.applies_deduped, 22, "legacy apply counters were lost");
        assert_eq!(ad.apply_dedup_overflow, 3, "legacy apply counters were lost");
        assert_eq!(ad.sweeps, 12, "legacy apply counters were lost");
    }

    /// A frozen source must not smuggle gauges in via the adopt branch, which
    /// fires when no live pipeline has reported apply_dedup yet.
    #[test]
    fn frozen_legacy_adopted_wholesale_still_drops_gauges() {
        let legacy = TelemetrySnapshot {
            apply_dedup: Some(ApplyDedupSnapshot {
                applies_total: 50,
                applies_deduped: 20,
                apply_dedup_overflow: 3,
                sweeps: 8,
                map_size: 9000,
                map_capacity: 9000,
            }),
            ..Default::default()
        };

        let merged = merge_snapshots(TelemetrySnapshot::default(), &legacy, GaugeSource::Frozen);
        let ad = merged.apply_dedup.expect("apply_dedup dropped by merge");

        assert_eq!(ad.applies_total, 50, "legacy counters lost on adopt");
        assert_eq!(ad.sweeps, 8, "legacy counters lost on adopt");
        assert_eq!(ad.map_size, 0, "frozen gauge adopted wholesale");
        assert_eq!(ad.map_capacity, 0, "frozen gauge adopted wholesale");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proto_body(secret: &str, events: Vec<ProtoEvent>) -> Vec<u8> {
        ProtoPublishEventsRequest {
            client_secret: secret.to_string(),
            events,
        }
        .encode_to_vec()
    }

    fn proto_event(name: &str) -> ProtoEvent {
        ProtoEvent {
            event_definition: name.to_string(),
            payload: Some(pbjson_types::Struct {
                fields: [(
                    "page".to_string(),
                    pbjson_types::Value {
                        kind: Some(pbjson_types::value::Kind::StringValue("/home".to_string())),
                    },
                )]
                .into_iter()
                .collect(),
            }),
            event_time: Some(pbjson_types::Timestamp {
                seconds: 1704067200,
                nanos: 0,
            }),
        }
    }

    #[test]
    fn parse_json_valid_request() {
        let body = br#"{"clientSecret":"s","events":[{"eventDefinition":"eventDefinitions/test","payload":{"key":"val"},"eventTime":"2024-01-01T00:00:00Z"}]}"#;
        let events = parse_events(body, false).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["eventDefinition"], "eventDefinitions/test");
    }

    #[test]
    fn parse_json_empty_events() {
        assert!(parse_events(br#"{"events":[]}"#, false).unwrap().is_empty());
    }

    #[test]
    fn parse_json_missing_events() {
        assert!(parse_events(br#"{"clientSecret":"s"}"#, false)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn parse_json_invalid() {
        assert!(parse_events(b"not json", false).is_err());
    }

    #[test]
    fn parse_protobuf_valid_request() {
        let bytes = proto_body("my-secret", vec![proto_event("eventDefinitions/click")]);
        let events = parse_events(&bytes, true).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["eventDefinition"], "eventDefinitions/click");
        assert_eq!(events[0]["payload"]["page"], "/home");
        assert!(events[0]["eventTime"].is_string());
    }

    #[test]
    fn parse_protobuf_empty_events() {
        let bytes = proto_body("s", vec![]);
        assert!(parse_events(&bytes, true).unwrap().is_empty());
    }

    #[test]
    fn parse_protobuf_invalid() {
        assert!(parse_events(b"\xff\xff", true).is_err());
    }

    // --- client secret probe: authorization before the events are decoded ---

    #[test]
    fn probe_json_camel_case() {
        let body = br#"{"clientSecret":"s","events":[{"eventDefinition":"x"}]}"#;
        assert_eq!(probe_client_secret(body, false).unwrap(), "s");
    }

    #[test]
    fn probe_json_snake_case() {
        let body = br#"{"client_secret":"abc","events":[]}"#;
        assert_eq!(probe_client_secret(body, false).unwrap(), "abc");
    }

    #[test]
    fn probe_json_missing_secret_is_empty() {
        let body = br#"{"events":[]}"#;
        assert_eq!(probe_client_secret(body, false).unwrap(), "");
    }

    #[test]
    fn probe_json_invalid() {
        assert!(probe_client_secret(b"not json", false).is_err());
    }

    #[test]
    fn probe_protobuf_reads_secret_past_events() {
        // Many events after the secret field: the probe must still return the
        // secret without depending on the events decoding.
        let events: Vec<ProtoEvent> = (0..50)
            .map(|i| proto_event(&format!("eventDefinitions/e{}", i)))
            .collect();
        let bytes = proto_body("my-secret", events);
        assert_eq!(probe_client_secret(&bytes, true).unwrap(), "my-secret");
    }

    #[test]
    fn probe_protobuf_empty_secret() {
        let bytes = proto_body("", vec![proto_event("eventDefinitions/x")]);
        assert_eq!(probe_client_secret(&bytes, true).unwrap(), "");
    }

    #[test]
    fn probe_protobuf_invalid() {
        assert!(probe_client_secret(b"\xff\xff", true).is_err());
    }

    #[test]
    fn probe_agrees_with_full_parse_on_both_encodings() {
        let bytes = proto_body("secret-123", vec![proto_event("eventDefinitions/a")]);
        assert_eq!(probe_client_secret(&bytes, true).unwrap(), "secret-123");
        assert_eq!(parse_events(&bytes, true).unwrap().len(), 1);

        let json = br#"{"clientSecret":"secret-123","events":[{"eventDefinition":"eventDefinitions/a"}]}"#;
        assert_eq!(probe_client_secret(json, false).unwrap(), "secret-123");
        assert_eq!(parse_events(json, false).unwrap().len(), 1);
    }

    #[test]
    fn aggregate_events_multiple_messages() {
        let messages = vec![
            serde_json::to_string(&json!([
                {"eventDefinition":"eventDefinitions/a","payload":{},"eventTime":"2024-01-01T00:00:00Z"},
                {"eventDefinition":"eventDefinitions/b","payload":{},"eventTime":"2024-01-01T00:00:00Z"},
            ]))
            .unwrap(),
            serde_json::to_string(&json!([
                {"eventDefinition":"eventDefinitions/c","payload":{},"eventTime":"2024-01-01T00:00:00Z"},
            ]))
            .unwrap(),
        ];
        let events = aggregate_events(&messages);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["eventDefinition"], "eventDefinitions/a");
        assert_eq!(events[2]["eventDefinition"], "eventDefinitions/c");
    }

    #[test]
    fn aggregate_events_skips_malformed() {
        let messages = vec![
            serde_json::to_string(&json!([{"eventDefinition":"eventDefinitions/a"}])).unwrap(),
            "not valid json".to_string(),
            serde_json::to_string(&json!([{"eventDefinition":"eventDefinitions/b"}])).unwrap(),
        ];
        let events = aggregate_events(&messages);
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn aggregate_events_empty() {
        assert!(aggregate_events(&[]).is_empty());
    }

    #[test]
    fn build_publish_request_structure() {
        let events = vec![json!({"eventDefinition":"eventDefinitions/test"})];
        let req = build_publish_events_request("my-secret", events, "2024-01-01T00:00:00Z");
        assert_eq!(req["clientSecret"], "my-secret");
        assert_eq!(req["sendTime"], "2024-01-01T00:00:00Z");
        assert_eq!(req["sdk"]["id"], "SDK_ID_CLOUDFLARE_RESOLVER");
        assert!(!req["sdk"]["version"].as_str().unwrap().is_empty());
        assert_eq!(req["events"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn build_publish_request_preserves_events() {
        let event = json!({
            "eventDefinition": "eventDefinitions/purchase",
            "payload": {"amount": 42.5, "currency": "USD"},
            "eventTime": "2024-06-15T10:30:00Z"
        });
        let req = build_publish_events_request("secret", vec![event.clone()], "2024-06-15T10:30:05Z");
        assert_eq!(req["events"][0], event);
    }

    #[test]
    fn protobuf_round_trip_preserves_nested_payload() {
        let proto_req = ProtoPublishEventsRequest {
            client_secret: "s".to_string(),
            events: vec![ProtoEvent {
                event_definition: "eventDefinitions/e".to_string(),
                payload: Some(pbjson_types::Struct {
                    fields: [
                        ("count".to_string(), pbjson_types::Value {
                            kind: Some(pbjson_types::value::Kind::NumberValue(42.0)),
                        }),
                        ("tags".to_string(), pbjson_types::Value {
                            kind: Some(pbjson_types::value::Kind::ListValue(
                                pbjson_types::ListValue {
                                    values: vec![
                                        pbjson_types::Value {
                                            kind: Some(pbjson_types::value::Kind::StringValue(
                                                "a".to_string(),
                                            )),
                                        },
                                        pbjson_types::Value {
                                            kind: Some(pbjson_types::value::Kind::StringValue(
                                                "b".to_string(),
                                            )),
                                        },
                                    ],
                                },
                            )),
                        }),
                    ]
                    .into_iter()
                    .collect(),
                }),
                event_time: None,
            }],
        };
        let bytes = proto_req.encode_to_vec();
        let events = parse_events(&bytes, true).unwrap();
        assert_eq!(events[0]["payload"]["count"], 42.0);
        assert_eq!(events[0]["payload"]["tags"][0], "a");
        assert_eq!(events[0]["payload"]["tags"][1], "b");
    }
}
