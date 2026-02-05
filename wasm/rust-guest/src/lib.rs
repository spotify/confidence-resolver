use std::sync::Arc;
use std::sync::LazyLock;

use arc_swap::ArcSwapOption;
use bytes::Bytes;
use confidence_resolver::assign_logger::AssignLogger;
use prost::Message;

use confidence_resolver::proto::confidence::flags::resolver::v1::{
    LogMessage, ResolveWithStickyRequest, WriteFlagLogsRequest,
};
use confidence_resolver::resolve_logger::ResolveLogger;
use wasm_msg::wasm_msg_guest;
use wasm_msg::wasm_msg_host;
use wasm_msg::WasmResult;

// Include the generated protobuf code
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/rust_guest.rs"));
}
use crate::proto::SetResolverStateRequest;
use confidence_resolver::{
    proto::{
        confidence::flags::admin::v1::ResolverState as ResolverStatePb,
        confidence::flags::resolver::v1::{
            ApplyFlagsRequest, ResolveFlagsRequest, ResolveFlagsResponse,
            ResolveWithStickyResponse, Sdk,
        },
        google::{Struct, Timestamp},
    },
    Client, FlagToApply, Host, ResolveReason, ResolvedValue, ResolverState,
};
use proto::Void;

impl
    From<confidence_resolver::proto::confidence::flags::resolver::v1::events::FallthroughAssignment>
    for proto::FallthroughAssignment
{
    fn from(
        val: confidence_resolver::proto::confidence::flags::resolver::v1::events::FallthroughAssignment,
    ) -> Self {
        proto::FallthroughAssignment {
            rule: val.rule,
            assignment_id: val.assignment_id,
            targeting_key: val.targeting_key,
            targeting_key_selector: val.targeting_key_selector,
        }
    }
}

const LOG_TARGET_BYTES: usize = 4 * 1024 * 1024; // 4 mb
const VOID: Void = Void {};
const ENCRYPTION_KEY: Bytes = Bytes::from_static(&[0; 16]);

// TODO simplify by assuming single threaded?
static RESOLVER_STATE: ArcSwapOption<ResolverState> = ArcSwapOption::const_empty();
static RESOLVE_LOGGER: LazyLock<ResolveLogger<WasmHost>> = LazyLock::new(ResolveLogger::new);
static ASSIGN_LOGGER: LazyLock<AssignLogger> = LazyLock::new(AssignLogger::new);

impl<'a> From<&ResolvedValue<'a>> for proto::ResolvedValue {
    fn from(val: &ResolvedValue<'a>) -> Self {
        proto::ResolvedValue {
            flag: Some(proto::Flag {
                name: val.flag.name.clone(),
            }),
            reason: convert_reason(val.reason),
            assignment_match: val
                .assignment_match
                .as_ref()
                .map(|am| proto::AssignmentMatch {
                    matched_rule: Some(proto::MatchedRule {
                        name: am.rule.clone().name,
                    }),
                    targeting_key: am.targeting_key.clone(),
                    segment: am.segment.name.clone(),
                    variant: am.variant.map(|v| proto::Variant {
                        name: v.clone().name,
                        value: v.value.clone(),
                    }),
                    assignment_id: am.assignment_id.to_string(),
                }),
            fallthrough_rules: val
                .fallthrough_rules
                .iter()
                .map(|fr| proto::FallthroughRule {
                    name: fr.rule.clone().name,
                    assignment_id: fr.clone().assignment_id,
                    targeting_key: fr.clone().targeting_key,
                    targeting_key_selector: fr.rule.clone().targeting_key_selector,
                })
                .collect(),
        }
    }
}

fn convert_reason(reason: ResolveReason) -> i32 {
    match reason {
        ResolveReason::Match => i32::from(proto::ResolveReason::Match),
        ResolveReason::NoSegmentMatch => i32::from(proto::ResolveReason::NoSegmentMatch),
        ResolveReason::FlagArchived => i32::from(proto::ResolveReason::FlagArchived),
        ResolveReason::TargetingKeyError => i32::from(proto::ResolveReason::TargetingKeyError),
    }
}

struct WasmHost;

impl Host for WasmHost {
    #[inline(never)]
    fn log(message: &str) {
        log_message(LogMessage {
            message: message.to_string(),
        })
        .unwrap();
    }

    fn current_time() -> Timestamp {
        current_time(Void {}).unwrap()
    }

    fn log_resolve(
        resolve_id: &str,
        evaluation_context: &Struct,
        values: &[ResolvedValue<'_>],
        client: &Client,
        _sdk: &Option<Sdk>,
    ) {
        RESOLVE_LOGGER.log_resolve(
            resolve_id,
            evaluation_context,
            &client.client_credential_name,
            values,
            client,
            _sdk,
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

    fn track_flag_not_found(count: u32) {
        use confidence_resolver::proto::confidence::flags::resolver::v1::OpenFeatureErrorCode;
        for _ in 0..count {
            RESOLVE_LOGGER.log_error(OpenFeatureErrorCode::FlagNotFound);
        }
    }

    fn encrypt_resolve_token(token_data: &[u8], _encryption_key: &[u8]) -> Result<Vec<u8>, String> {
        Ok(token_data.to_vec())
    }

    fn decrypt_resolve_token(token_data: &[u8], _encryption_key: &[u8]) -> Result<Vec<u8>, String> {
        Ok(token_data.to_vec())
    }
}

/// Safely gets an owned handle to the current resolver state.
fn get_resolver_state() -> Result<Arc<ResolverState>, String> {
    let guard = RESOLVER_STATE.load();
    // Dereference the guard to get at the Option, then clone the Arc inside.
    // .cloned() on an Option<&Arc<T>> gives an Option<Arc<T>>.
    guard
        .as_ref()
        .cloned()
        .ok_or_else(|| "Resolver state not set".to_string())
}

wasm_msg_guest! {
    // Initialize the current thread with entropy for the RNG.
    // Should be called once per thread in multi-threaded WASM environments.
    fn init_thread(request: proto::InitThreadRequest) -> WasmResult<Void> {
        confidence_resolver::seed_rng(request.rng_seed);
        Ok(VOID)
    }

    fn set_resolver_state(request: SetResolverStateRequest) -> WasmResult<Void> {
        let state_pb = ResolverStatePb::decode(request.state.as_slice())
            .map_err(|e| format!("Failed to decode resolver state: {}", e))?;
        let new_state = ResolverState::from_proto(state_pb, request.account_id.as_str())?;
        RESOLVER_STATE.store(Some(Arc::new(new_state)));
        Ok(VOID)
    }

    fn resolve_with_sticky(request: ResolveWithStickyRequest) -> WasmResult<ResolveWithStickyResponse> {
        let resolver_state = get_resolver_state()?;
        let resolve_request = &request.resolve_request.clone().unwrap();
        let evaluation_context = resolve_request.evaluation_context.clone().unwrap();
        let resolver = resolver_state.get_resolver::<WasmHost>(resolve_request.client_secret.as_str(), evaluation_context, &ENCRYPTION_KEY)?;
        resolver.resolve_flags_sticky(&request)
    }

    fn resolve(request: ResolveFlagsRequest) -> WasmResult<ResolveFlagsResponse> {
        let resolver_state = get_resolver_state()?;
        let evaluation_context = request.evaluation_context.as_ref().cloned().unwrap_or_default();
        let resolver = resolver_state.get_resolver::<WasmHost>(&request.client_secret, evaluation_context, &ENCRYPTION_KEY)?;
        resolver.resolve_flags(&request)
    }

    // deprecated
    fn flush_logs(_request:Void) -> WasmResult<WriteFlagLogsRequest> {
        let mut req = RESOLVE_LOGGER.checkpoint();
        ASSIGN_LOGGER.checkpoint_fill(&mut req);
        Ok(req)
    }

    fn bounded_flush_logs(_request:Void) -> WasmResult<WriteFlagLogsRequest> {
        let mut req = RESOLVE_LOGGER.checkpoint();
        ASSIGN_LOGGER.checkpoint_fill_with_limit(&mut req, LOG_TARGET_BYTES, false);
        Ok(req)
    }

    fn bounded_flush_assign(_request:Void) -> WasmResult<WriteFlagLogsRequest> {
        Ok(ASSIGN_LOGGER.checkpoint_with_limit(LOG_TARGET_BYTES, true))
    }

    fn apply_flags(request: ApplyFlagsRequest) -> WasmResult<Void> {
        let resolver_state = get_resolver_state()?;
        // Use empty evaluation context - the real one is extracted from the resolve token
        let evaluation_context = Struct::default();
        let resolver = match resolver_state.get_resolver::<WasmHost>(&request.client_secret, evaluation_context, &ENCRYPTION_KEY) {
            Ok(r) => r,
            Err(_) => {
                // State may have changed and client_secret is no longer valid.
                // This is not a fatal error - just skip the apply silently.
                // The flag was already resolved successfully, we just can't log the apply event.
                return Ok(VOID);
            }
        };
        // Ignore apply errors - best effort logging
        let _ = resolver.apply_flags(&request);
        Ok(VOID)
    }
}

// Declare the add function as a host function
wasm_msg_host! {
    fn log_message(message: LogMessage) -> WasmResult<Void>;
    fn current_time(request: Void) -> WasmResult<Timestamp>;
}
