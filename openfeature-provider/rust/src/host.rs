//! Native Rust implementation of the Host trait for the confidence resolver.

use std::sync::LazyLock;

use arc_swap::ArcSwap;
use confidence_resolver::assign_logger::AssignLogger;
use confidence_resolver::proto::confidence::flags::resolver::v1::Sdk;
use confidence_resolver::proto::google::Struct;
use confidence_resolver::resolve_logger::ResolveLogger;
use confidence_resolver::telemetry::{Telemetry, TelemetrySnapshot};
use confidence_resolver::{Client, FlagToApply, Host, ResolvedValue};

/// Global resolve logger instance.
pub static RESOLVE_LOGGER: LazyLock<ResolveLogger<NativeHost>> = LazyLock::new(ResolveLogger::new);

/// Global assign logger instance.
pub static ASSIGN_LOGGER: LazyLock<AssignLogger> = LazyLock::new(AssignLogger::new);

/// Global telemetry instance for recording resolve rates and latencies.
pub static TELEMETRY: LazyLock<Telemetry> = LazyLock::new(Telemetry::new);

/// Snapshot of the last flushed telemetry, used for delta computation.
pub static LAST_FLUSHED: LazyLock<ArcSwap<TelemetrySnapshot>> =
    LazyLock::new(|| ArcSwap::from_pointee(TelemetrySnapshot::default()));

/// Native Rust host implementation for the confidence resolver.
///
/// This implements the `Host` trait using standard library functions
/// for time, randomness, and encryption. Logging is delegated to
/// the global `ResolveLogger` and `AssignLogger` instances.
pub struct NativeHost;

impl Host for NativeHost {
    fn log(message: &str) {
        tracing::debug!("{}", message);
    }

    fn log_resolve(
        resolve_id: &str,
        evaluation_context: &Struct,
        values: &[ResolvedValue<'_>],
        client: &Client,
    ) {
        RESOLVE_LOGGER.log_resolve(
            resolve_id,
            evaluation_context,
            &client.client_credential_name,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use bytes::Bytes;
    use confidence_resolver::proto::confidence::flags::resolver::v1::{
        resolve_process_response, ResolveFlagsRequest, ResolveProcessRequest,
    };
    use confidence_resolver::proto::google::{value, Struct, Value as ProtoValue};

    use crate::test_utils::{create_state_with_flag, TEST_CLIENT_SECRET};

    #[test]
    fn disable_exposure_collection_does_not_enqueue_assigns_but_logs_resolves() {
        let (state, _) = create_state_with_flag();
        let mut fields = HashMap::new();
        fields.insert(
            "targeting_key".to_string(),
            ProtoValue {
                kind: Some(value::Kind::StringValue("user-1".to_string())),
            },
        );
        let context = Struct { fields };
        let encryption_key = Bytes::from_static(&[0; 16]);
        let resolver = state
            .get_resolver::<NativeHost>(TEST_CLIENT_SECRET, context, &encryption_key)
            .expect("resolver")
            .with_disable_exposure_collection(true);

        let _ = ASSIGN_LOGGER.checkpoint();
        let _ = RESOLVE_LOGGER.checkpoint();

        let request = ResolveFlagsRequest {
            flags: vec!["flags/test-flag".to_string()],
            evaluation_context: Some(Struct::default()),
            apply: true,
            client_secret: TEST_CLIENT_SECRET.to_string(),
            sdk: None,
        };
        let process_response = resolver
            .resolve_flags(ResolveProcessRequest::without_materializations(request))
            .expect("resolve");
        let response = match process_response.result {
            Some(resolve_process_response::Result::Resolved(resolved)) => {
                resolved.response.expect("resolve response")
            }
            other => panic!("expected resolved response, got {other:?}"),
        };

        assert!(
            response.resolve_token.is_empty(),
            "disable_exposure_collection must not emit a deferred-apply resolve token"
        );

        let assigns = ASSIGN_LOGGER.checkpoint();
        let resolves = RESOLVE_LOGGER.checkpoint();
        assert!(
            assigns
                .flag_assigned
                .iter()
                .flat_map(|assigned| &assigned.flags)
                .all(|flag| flag.flag != "flags/test-flag"),
            "disable_exposure_collection must not enqueue assigns for test-flag"
        );
        assert!(
            !resolves.client_resolve_info.is_empty() || !resolves.flag_resolve_info.is_empty(),
            "disable_exposure_collection must still log resolves"
        );
    }
}
