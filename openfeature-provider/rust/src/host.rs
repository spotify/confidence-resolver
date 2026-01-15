//! Native Rust implementation of the Host trait for the confidence resolver.

use std::sync::LazyLock;

use confidence_resolver::assign_logger::AssignLogger;
use confidence_resolver::proto::confidence::flags::resolver::v1::Sdk;
use confidence_resolver::proto::google::Struct;
use confidence_resolver::resolve_logger::ResolveLogger;
use confidence_resolver::{Client, FlagToApply, Host, ResolvedValue};

/// Global resolve logger instance.
pub static RESOLVE_LOGGER: LazyLock<ResolveLogger<NativeHost>> = LazyLock::new(ResolveLogger::new);

/// Global assign logger instance.
pub static ASSIGN_LOGGER: LazyLock<AssignLogger> = LazyLock::new(AssignLogger::new);

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
        sdk: &Option<Sdk>,
    ) {
        RESOLVE_LOGGER.log_resolve(
            resolve_id,
            evaluation_context,
            &client.client_credential_name,
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
