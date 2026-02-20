use confidence_resolver::{
    assign_logger::AssignLogger,
    flag_logger,
    proto::{confidence, google::Struct},
    FlagToApply, Host, ResolvedValue, ResolverState,
};
use worker::*;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::Bytes;
use prost::Message;
use serde_json::from_slice;
use serde_json::json;

use confidence::flags::resolver::v1::{ApplyFlagsRequest, ApplyFlagsResponse, ResolveFlagsRequest};
use confidence_resolver::proto::confidence::flags::resolver::v1::ResolveProcessRequest;

static RESOLVE_LOGGER: LazyLock<ResolveLogger<H>> = LazyLock::new(ResolveLogger::new);
static ASSIGN_LOGGER: LazyLock<AssignLogger> = LazyLock::new(AssignLogger::new);

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
        sdk: &Option<Sdk>,
    ) {
        RESOLVE_LOGGER.log_resolve(
            resolve_id,
            evaluation_context,
            client.client_credential_name.as_str(),
            values,
            client,
            sdk,
        );
    }

    fn log_assign(
        resolve_id: &str,
        evaluation_context: &Struct,
        assigned_flags: &[FlagToApply],
        client: &Client,
        sdk: &Option<Sdk>,
    ) {
        ASSIGN_LOGGER.log_assigns(resolve_id, evaluation_context, assigned_flags, client, sdk);
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
                                                Response::from_json(&response)?
                                                    .with_cors_headers(&allowed_origin)
                                            }
                                            None => Response::error(
                                                "Unexpected suspended response",
                                                500,
                                            )?
                                            .with_cors_headers(&allowed_origin),
                                        }
                                    }
                                    Err(msg) => Response::error(msg, 500)?
                                        .with_cors_headers(&allowed_origin),
                                }
                            }
                            Err(msg) => {
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

    // Use ctx.waitUntil to run logging after response is returned
    ctx.wait_until(async move {
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
        let req = flag_logger::aggregate_batch(logs);
        send_flags_logs(CONFIDENCE_CLIENT_SECRET.get().unwrap().as_str(), req).await?;
    }

    Ok(())
}

fn checkpoint() -> WriteFlagLogsRequest {
    let mut req = RESOLVE_LOGGER.checkpoint();
    ASSIGN_LOGGER.checkpoint_fill(&mut req);
    req
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
