use confidence_resolver::{
    assign_logger::AssignLogger,
    flag_logger,
    proto::{confidence, google::Struct},
    resolve_logger::ResolveLogger,
    telemetry::{self, TelemetrySnapshot},
    FlagToApply, Host, ResolvedValue, ResolverState,
};
use worker::*;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::Bytes;
use prost::Message;
use serde_json::from_slice;
use serde_json::json;
use wasm_bindgen::JsCast;

use confidence::flags::resolver::v1::{ApplyFlagsRequest, ApplyFlagsResponse, ResolveFlagsRequest};
use confidence_resolver::proto::confidence::flags::resolver::v1::{
    ResolveProcessRequest, ResolveReason,
};

use confidence_resolver::Client;
use once_cell::sync::Lazy;

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
use confidence_resolver::proto::confidence::flags::resolver::v1::{
    TelemetryData, WriteFlagLogsRequest,
};
use std::sync::{LazyLock, Mutex, OnceLock};

/// Prometheus exposition format content type (version 0.0.4).
const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

static FLAGS_LOGS_QUEUE: OnceLock<Queue> = OnceLock::new();

static CONFIDENCE_CLIENT_SECRET: OnceLock<String> = OnceLock::new();

static RESOLVE_LOGGER: LazyLock<ResolveLogger<H>> = LazyLock::new(ResolveLogger::new);
static ASSIGN_LOGGER: LazyLock<AssignLogger> = LazyLock::new(AssignLogger::new);
static TELEMETRY_LOG: Mutex<Option<TelemetryData>> = Mutex::new(None);

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

fn sdk_info() -> Sdk {
    Sdk {
        sdk: Some(confidence::flags::resolver::v1::sdk::Sdk::Id(
            confidence::flags::resolver::v1::SdkId::CloudflareResolver as i32,
        )),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

fn accumulate_telemetry(td: TelemetryData) {
    if let Ok(mut guard) = TELEMETRY_LOG.lock() {
        match guard.as_mut() {
            Some(acc) => {
                match (&mut acc.resolve_latency, td.resolve_latency) {
                    (Some(a), Some(d)) => {
                        a.sum = a.sum.wrapping_add(d.sum);
                        a.count = a.count.wrapping_add(d.count);
                        a.buckets.extend(d.buckets);
                    }
                    (None, some) => acc.resolve_latency = some,
                    _ => {}
                }
                for dr in td.resolve_rate {
                    if let Some(ar) = acc.resolve_rate.iter_mut().find(|r| r.reason == dr.reason) {
                        ar.count = ar.count.wrapping_add(dr.count);
                    } else {
                        acc.resolve_rate.push(dr);
                    }
                }
                if !td.resolver_version.is_empty() {
                    acc.resolver_version = td.resolver_version;
                }
            }
            None => *guard = Some(td),
        }
    }
}

fn checkpoint() -> WriteFlagLogsRequest {
    let mut req = RESOLVE_LOGGER.checkpoint();
    ASSIGN_LOGGER.checkpoint_fill(&mut req);
    if let Ok(mut guard) = TELEMETRY_LOG.lock() {
        if let Some(mut td) = guard.take() {
            td.sdk = Some(sdk_info());
            req.telemetry_data = Some(td);
        }
    }
    req
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
        .unwrap_or("*".to_string());

    let state_etag_env = env
        .var("RESOLVER_STATE_ETAG")
        .map(|var| var.to_string())
        .unwrap_or_default();

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
        .get_async("/metrics", |req, ctx| {
            let allowed_origin = allowed_origin_env.clone();
            async move {
                if let Some(expected) = CONFIDENCE_CLIENT_SECRET.get() {
                    let authorized = req.headers().get("Authorization").ok().flatten()
                        .map(|v| v.strip_prefix("ClientSecret ").unwrap_or("") == expected.as_str())
                        .unwrap_or(false);
                    if !authorized {
                        return Response::error("Unauthorized", 401)?
                            .with_cors_headers(&allowed_origin);
                    }
                }
                let text = match ctx.env.kv("CONFIDENCE_METRICS_KV") {
                    Ok(kv) => kv.get("prometheus").text().await.unwrap_or(None),
                    Err(_) => None,
                };
                let body = text.unwrap_or_default();
                let headers = Headers::new();
                headers.set("Content-Type", PROMETHEUS_CONTENT_TYPE)?;
                headers.set("Cache-Control", "no-store")?;
                Response::ok(body)?.with_headers(headers).with_cors_headers(&allowed_origin)
            }
        })
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
                        resolver_request.apply = true;
                        let evaluation_context = resolver_request
                            .evaluation_context
                            .clone()
                            .unwrap_or_default();

                        let t0 = js_sys::Date::now();

                        let (reasons, resp) = match state.get_resolver::<H>(
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
                                                (reasons, Response::from_json(&response)?
                                                    .with_cors_headers(&allowed_origin))
                                            }
                                            None => {
                                                (vec![ResolveReason::Error],
                                                Response::error(
                                                    "Unexpected suspended response",
                                                    500,
                                                )?
                                                .with_cors_headers(&allowed_origin))
                                            }
                                        }
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

                        accumulate_telemetry(
                            telemetry::build_request_telemetry(elapsed_us, &reasons),
                        );

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

                        // This resolver forces apply=true at resolve time and
                        // returns no resolve token. Some SDKs still send a background
                        // apply with an empty token — nothing to do.
                        if apply_flag_req.resolve_token.is_empty() {
                            return Response::from_json(&ApplyFlagsResponse::default())?
                                .with_cors_headers(&allowed_origin);
                        }

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
                    "telemetry:upload" => {
                        Response::ok("")?.with_cors_headers(&allowed_origin)
                    }
                    _ => Response::error("Not found", 404)?.with_cors_headers(&allowed_origin),
                }
            }
        })
        .run(req, env)
        .await;

    ctx.wait_until(async move {
        let req = checkpoint();
        if let Ok(json) = serde_json::to_string(&req) {
            if let Some(queue) = FLAGS_LOGS_QUEUE.get() {
                let _ = queue.send(json).await;
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
            .map(|s| serde_json::from_str::<WriteFlagLogsRequest>(s.as_str()).unwrap())
            .collect();

        let req = flag_logger::aggregate_batch(logs);

        if let Ok(kv) = env.kv("CONFIDENCE_METRICS_KV") {
            update_prometheus_kv(&kv, &req).await;
        }

        send_flags_logs(CONFIDENCE_CLIENT_SECRET.get().unwrap().as_str(), req).await?;
    }

    Ok(())
}

async fn update_prometheus_kv(kv: &kv::KvStore, req: &WriteFlagLogsRequest) {
    let mut cumulative = match kv.get("snapshot").text().await {
        Ok(Some(text)) => serde_json::from_str::<TelemetrySnapshot>(&text).unwrap_or_default(),
        _ => TelemetrySnapshot::default(),
    };

    if let Some(td) = &req.telemetry_data {
        cumulative.accumulate_delta(td);
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
