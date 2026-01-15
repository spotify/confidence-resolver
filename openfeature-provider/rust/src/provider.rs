//! OpenFeature provider implementation for Confidence.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use open_feature::provider::{
    FeatureProvider, ProviderMetadata, ProviderStatus, ResolutionDetails,
};
use open_feature::{
    EvaluationContext, EvaluationContextFieldValue, EvaluationError, EvaluationErrorCode,
    EvaluationReason, EvaluationResult, StructValue, Value,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use confidence_resolver::proto::confidence::flags::resolver::v1::{
    resolve_with_sticky_response::ResolveResult as ProtoResolveResult, ReadOperationsRequest,
    ReadResult, ResolveFlagsRequest, ResolveReason, ResolveWithStickyRequest, Sdk, SdkId,
};
use confidence_resolver::proto::google::{value, Struct, Value as ProtoValue};

use crate::error::{Error, Result};
use crate::host::{NativeHost, ASSIGN_LOGGER, RESOLVE_LOGGER};
use crate::logger::LogManager;
use crate::materialization::{
    read_ops_from_proto, read_results_to_proto, write_ops_from_proto, MaterializationStore,
};
use crate::state::{SharedState, StateFetcher};
use crate::VERSION;

/// Default interval for polling state updates (30 seconds).
const DEFAULT_STATE_POLL_INTERVAL_MS: u64 = 30_000;

/// Default interval for flushing all logs (10 seconds).
const DEFAULT_FLUSH_INTERVAL_MS: u64 = 10_000;

/// Default interval for flushing assign logs (100 ms).
const DEFAULT_ASSIGN_FLUSH_INTERVAL_MS: u64 = 100;

/// Encryption key for resolve tokens (null encryption for local provider).
const ENCRYPTION_KEY: Bytes = Bytes::from_static(&[0; 16]);

/// Materialization store configuration.
pub enum MaterializationStoreConfig {
    /// Use the Confidence remote materialization store.
    ConfidenceRemote,
    /// Use a custom materialization store.
    Custom(Arc<dyn MaterializationStore>),
}

/// Configuration options for the Confidence provider.
pub struct ProviderOptions {
    /// The client secret for authentication.
    pub client_secret: String,
    /// Timeout for initialization in milliseconds.
    pub initialize_timeout_ms: Option<u64>,
    /// Interval for polling state updates in milliseconds.
    pub state_poll_interval_ms: Option<u64>,
    /// Interval for flushing logs in milliseconds.
    pub flush_interval_ms: Option<u64>,
    /// Interval for flushing assign logs in milliseconds.
    pub assign_flush_interval_ms: Option<u64>,
    /// Materialization store for sticky resolution.
    /// If not set, sticky resolution is disabled.
    pub materialization_store: Option<MaterializationStoreConfig>,
}

impl ProviderOptions {
    /// Create new options with the required client secret.
    pub fn new(client_secret: impl Into<String>) -> Self {
        Self {
            client_secret: client_secret.into(),
            initialize_timeout_ms: None,
            state_poll_interval_ms: None,
            flush_interval_ms: None,
            assign_flush_interval_ms: None,
            materialization_store: None,
        }
    }

    /// Set the initialization timeout.
    pub fn with_initialize_timeout(mut self, timeout_ms: u64) -> Self {
        self.initialize_timeout_ms = Some(timeout_ms);
        self
    }

    /// Set the state poll interval.
    pub fn with_state_poll_interval(mut self, interval_ms: u64) -> Self {
        self.state_poll_interval_ms = Some(interval_ms);
        self
    }

    /// Enable sticky resolution with the Confidence remote materialization store.
    pub fn with_confidence_materialization_store(mut self) -> Self {
        self.materialization_store = Some(MaterializationStoreConfig::ConfidenceRemote);
        self
    }

    /// Enable sticky resolution with a custom materialization store.
    pub fn with_materialization_store(mut self, store: Arc<dyn MaterializationStore>) -> Self {
        self.materialization_store = Some(MaterializationStoreConfig::Custom(store));
        self
    }
}

/// OpenFeature provider for Confidence using native Rust resolver.
pub struct ConfidenceProvider {
    metadata: ProviderMetadata,
    client_secret: String,
    state: Arc<SharedState>,
    state_fetcher: Arc<StateFetcher>,
    log_manager: Arc<LogManager>,
    materialization_store: Option<Arc<dyn MaterializationStore>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    background_tasks: Vec<JoinHandle<()>>,
    status: ProviderStatus,
    state_poll_interval_ms: u64,
    flush_interval_ms: u64,
    assign_flush_interval_ms: u64,
}

impl ConfidenceProvider {
    /// Create a new Confidence provider.
    pub fn new(options: ProviderOptions) -> Result<Self> {
        let state_fetcher = Arc::new(StateFetcher::new(options.client_secret.clone())?);
        let log_manager = Arc::new(LogManager::new(options.client_secret.clone())?);

        // Create materialization store if configured
        let materialization_store: Option<Arc<dyn MaterializationStore>> =
            match options.materialization_store {
                Some(MaterializationStoreConfig::ConfidenceRemote) => {
                    use crate::materialization::ConfidenceRemoteMaterializationStore;
                    Some(Arc::new(ConfidenceRemoteMaterializationStore::new(
                        options.client_secret.clone(),
                    )?))
                }
                Some(MaterializationStoreConfig::Custom(store)) => Some(store),
                None => None,
            };

        Ok(Self {
            metadata: ProviderMetadata::new("confidence-local-resolver"),
            client_secret: options.client_secret,
            state: Arc::new(SharedState::new()),
            state_fetcher,
            log_manager,
            materialization_store,
            shutdown_tx: None,
            background_tasks: Vec::new(),
            status: ProviderStatus::NotReady,
            state_poll_interval_ms: options
                .state_poll_interval_ms
                .unwrap_or(DEFAULT_STATE_POLL_INTERVAL_MS),
            flush_interval_ms: options
                .flush_interval_ms
                .unwrap_or(DEFAULT_FLUSH_INTERVAL_MS),
            assign_flush_interval_ms: options
                .assign_flush_interval_ms
                .unwrap_or(DEFAULT_ASSIGN_FLUSH_INTERVAL_MS),
        })
    }

    /// Initialize the provider by fetching initial state and starting background tasks.
    pub async fn init(&mut self) -> Result<()> {
        // Fetch initial state
        let result = self.state_fetcher.fetch().await?;
        if let Some((state, account_id)) = result {
            self.state.update(state, account_id).await;
        } else {
            return Err(Error::StateFetch("No state returned from CDN".to_string()));
        }

        // Start background tasks
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        self.shutdown_tx = Some(shutdown_tx);

        self.start_background_tasks(shutdown_rx);
        self.status = ProviderStatus::Ready;

        Ok(())
    }

    /// Shutdown the provider and flush remaining logs.
    pub async fn shutdown(&mut self) {
        // Signal shutdown
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        // Wait for background tasks to complete (with timeout)
        for task in self.background_tasks.drain(..) {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), task).await;
        }

        // Final flush
        let _ = self
            .log_manager
            .flush_all(&RESOLVE_LOGGER, &ASSIGN_LOGGER)
            .await;
    }

    fn start_background_tasks(&mut self, shutdown_rx: oneshot::Receiver<()>) {
        let state = Arc::clone(&self.state);
        let state_fetcher = Arc::clone(&self.state_fetcher);
        let log_manager = Arc::clone(&self.log_manager);

        let state_poll_interval = self.state_poll_interval_ms;
        let flush_interval = self.flush_interval_ms;
        let assign_flush_interval = self.assign_flush_interval_ms;

        // Spawn combined background task
        let task = tokio::spawn(async move {
            let mut shutdown_rx = shutdown_rx;
            let mut state_interval =
                tokio::time::interval(std::time::Duration::from_millis(state_poll_interval));
            let mut flush_interval =
                tokio::time::interval(std::time::Duration::from_millis(flush_interval));
            let mut assign_interval =
                tokio::time::interval(std::time::Duration::from_millis(assign_flush_interval));

            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        break;
                    }
                    _ = state_interval.tick() => {
                        // Fetch latest state
                        match state_fetcher.fetch().await {
                            Ok(Some((new_state, account_id))) => {
                                state.update(new_state, account_id).await;
                            }
                            Ok(None) => {
                                // State unchanged (304)
                            }
                            Err(e) => {
                                tracing::error!("Failed to fetch state: {}", e);
                            }
                        }
                    }
                    _ = flush_interval.tick() => {
                        if let Err(e) = log_manager.flush_all(&RESOLVE_LOGGER, &ASSIGN_LOGGER).await {
                            tracing::error!("Failed to flush logs: {}", e);
                        }
                    }
                    _ = assign_interval.tick() => {
                        if let Err(e) = log_manager.flush_assign(&ASSIGN_LOGGER).await {
                            tracing::error!("Failed to flush assign logs: {}", e);
                        }
                    }
                }
            }
        });

        self.background_tasks.push(task);
    }

    /// Resolve a flag with the given key and evaluation context.
    async fn resolve_flag(
        &self,
        flag_key: &str,
        context: &EvaluationContext,
    ) -> EvaluationResult<ResolveResult> {
        // Get state
        let state = self.state.get().ok_or_else(|| {
            EvaluationError::builder()
                .code(EvaluationErrorCode::ProviderNotReady)
                .message("Provider not initialized")
                .build()
        })?;

        // Parse flag path
        let (flag_name, path) = parse_flag_path(flag_key);

        // Convert evaluation context to protobuf
        let proto_context = convert_evaluation_context(context);

        // Create resolve request
        let request = ResolveFlagsRequest {
            flags: vec![format!("flags/{}", flag_name)],
            evaluation_context: Some(proto_context.clone()),
            apply: true,
            client_secret: self.client_secret.clone(),
            sdk: Some(Sdk {
                sdk: Some(
                    confidence_resolver::proto::confidence::flags::resolver::v1::sdk::Sdk::Id(
                        SdkId::RustProvider as i32,
                    ),
                ),
                version: VERSION.to_string(),
            }),
        };

        // Determine if sticky processing is enabled (based on materialization store)
        let not_process_sticky = self.materialization_store.is_none();

        // Create sticky request
        let mut sticky_request = ResolveWithStickyRequest {
            resolve_request: Some(request),
            materializations: vec![],
            fail_fast_on_sticky: false,
            not_process_sticky,
        };

        // Get resolver
        let resolver = state
            .get_resolver::<NativeHost>(&self.client_secret, proto_context, &ENCRYPTION_KEY)
            .map_err(|e| {
                EvaluationError::builder()
                    .code(EvaluationErrorCode::General(format!(
                        "Failed to get resolver: {}",
                        e
                    )))
                    .build()
            })?;

        // Resolve with sticky loop
        let success = loop {
            let response = resolver
                .resolve_flags_sticky(&sticky_request)
                .map_err(|e| {
                    EvaluationError::builder()
                        .code(EvaluationErrorCode::General(format!(
                            "Failed to resolve: {}",
                            e
                        )))
                        .build()
                })?;

            match response.resolve_result {
                Some(ProtoResolveResult::Success(s)) => break s,
                Some(ProtoResolveResult::ReadOpsRequest(read_ops_request)) => {
                    // Fetch materializations from store
                    let materializations = self
                        .read_materializations(&read_ops_request)
                        .await
                        .map_err(|e| {
                            EvaluationError::builder()
                                .code(EvaluationErrorCode::General(format!(
                                    "Failed to read materializations: {}",
                                    e
                                )))
                                .build()
                        })?;

                    // Re-resolve with materializations
                    sticky_request.materializations = materializations;
                }
                None => {
                    return Err(EvaluationError::builder()
                        .code(EvaluationErrorCode::General(
                            "No resolve result".to_string(),
                        ))
                        .build());
                }
            }
        };

        // Handle materialization updates (writes)
        if !success.materialization_updates.is_empty() {
            self.write_materializations(&success.materialization_updates)
                .await;
        }

        let flags_response = success.response.ok_or_else(|| {
            EvaluationError::builder()
                .code(EvaluationErrorCode::General(
                    "No response in success".to_string(),
                ))
                .build()
        })?;

        // Get resolved flag
        let resolved_flag = flags_response.resolved_flags.first().ok_or_else(|| {
            EvaluationError::builder()
                .code(EvaluationErrorCode::FlagNotFound)
                .message(format!("Flag '{}' not found", flag_name))
                .build()
        })?;

        // Check variant
        if resolved_flag.variant.is_empty() {
            return Ok(ResolveResult {
                value: None,
                variant: None,
                reason: map_resolve_reason(resolved_flag.reason),
            });
        }

        // Extract value
        let mut value = resolved_flag.value.clone();

        // Navigate path if specified
        if let Some(ref path_str) = path {
            value = navigate_path(value, path_str);
        }

        Ok(ResolveResult {
            value,
            variant: Some(resolved_flag.variant.clone()),
            reason: EvaluationReason::TargetingMatch,
        })
    }

    /// Read materializations from the store.
    async fn read_materializations(
        &self,
        read_ops_request: &ReadOperationsRequest,
    ) -> Result<Vec<ReadResult>> {
        let store = self.materialization_store.as_ref().ok_or_else(|| {
            Error::Materialization("No materialization store configured".to_string())
        })?;

        let read_ops = read_ops_from_proto(read_ops_request);
        let results = store.read_materializations(read_ops).await?;
        Ok(read_results_to_proto(results))
    }

    /// Write materializations to the store (fire and forget).
    async fn write_materializations(
        &self,
        materialization_updates: &[confidence_resolver::proto::confidence::flags::resolver::v1::VariantData],
    ) {
        if let Some(ref store) = self.materialization_store {
            let write_ops = write_ops_from_proto(materialization_updates);
            if let Err(e) = store.write_materializations(write_ops).await {
                tracing::warn!("Failed to write materializations: {}", e);
            }
        }
    }
}

struct ResolveResult {
    value: Option<Struct>,
    variant: Option<String>,
    reason: EvaluationReason,
}

#[async_trait]
impl FeatureProvider for ConfidenceProvider {
    async fn initialize(&mut self, _context: &EvaluationContext) {
        if let Err(e) = self.init().await {
            tracing::error!("Failed to initialize provider: {}", e);
            self.status = ProviderStatus::Error;
        }
    }

    fn status(&self) -> ProviderStatus {
        match self.status {
            ProviderStatus::NotReady => ProviderStatus::NotReady,
            ProviderStatus::Ready => ProviderStatus::Ready,
            ProviderStatus::Error => ProviderStatus::Error,
            ProviderStatus::STALE => ProviderStatus::STALE,
        }
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn resolve_bool_value(
        &self,
        flag_key: &str,
        evaluation_context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<bool>> {
        let result = self.resolve_flag(flag_key, evaluation_context).await?;

        // If no variant assigned (no match), return default with the reason
        if result.variant.is_none() {
            return Ok(ResolutionDetails {
                value: false, // default value
                variant: None,
                reason: Some(result.reason),
                flag_metadata: None,
            });
        }

        let value = extract_bool_value(&result.value).ok_or_else(|| {
            EvaluationError::builder()
                .code(EvaluationErrorCode::TypeMismatch)
                .message("Value is not a boolean")
                .build()
        })?;

        Ok(ResolutionDetails {
            value,
            variant: result.variant,
            reason: Some(result.reason),
            flag_metadata: None,
        })
    }

    async fn resolve_int_value(
        &self,
        flag_key: &str,
        evaluation_context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<i64>> {
        let result = self.resolve_flag(flag_key, evaluation_context).await?;

        let value = extract_number_value(&result.value)
            .map(|v| v as i64)
            .ok_or_else(|| {
                EvaluationError::builder()
                    .code(EvaluationErrorCode::TypeMismatch)
                    .message("Value is not a number")
                    .build()
            })?;

        Ok(ResolutionDetails {
            value,
            variant: result.variant,
            reason: Some(result.reason),
            flag_metadata: None,
        })
    }

    async fn resolve_float_value(
        &self,
        flag_key: &str,
        evaluation_context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<f64>> {
        let result = self.resolve_flag(flag_key, evaluation_context).await?;

        let value = extract_number_value(&result.value).ok_or_else(|| {
            EvaluationError::builder()
                .code(EvaluationErrorCode::TypeMismatch)
                .message("Value is not a number")
                .build()
        })?;

        Ok(ResolutionDetails {
            value,
            variant: result.variant,
            reason: Some(result.reason),
            flag_metadata: None,
        })
    }

    async fn resolve_string_value(
        &self,
        flag_key: &str,
        evaluation_context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<String>> {
        let result = self.resolve_flag(flag_key, evaluation_context).await?;

        let value = extract_string_value(&result.value).ok_or_else(|| {
            EvaluationError::builder()
                .code(EvaluationErrorCode::TypeMismatch)
                .message("Value is not a string")
                .build()
        })?;

        Ok(ResolutionDetails {
            value,
            variant: result.variant,
            reason: Some(result.reason),
            flag_metadata: None,
        })
    }

    async fn resolve_struct_value(
        &self,
        flag_key: &str,
        evaluation_context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<StructValue>> {
        let result = self.resolve_flag(flag_key, evaluation_context).await?;

        let value = result
            .value
            .map(|s| proto_struct_to_openfeature(&s))
            .unwrap_or_default();

        Ok(ResolutionDetails {
            value,
            variant: result.variant,
            reason: Some(result.reason),
            flag_metadata: None,
        })
    }
}

// Helper functions

fn parse_flag_path(flag_key: &str) -> (&str, Option<String>) {
    match flag_key.split_once('.') {
        Some((name, path)) => (name, Some(path.to_string())),
        None => (flag_key, None),
    }
}

fn convert_evaluation_context(ctx: &EvaluationContext) -> Struct {
    let mut fields = HashMap::new();

    // Add targeting key as targeting_key (matching the JS provider behavior)
    if let Some(ref key) = ctx.targeting_key {
        fields.insert(
            "targeting_key".to_string(),
            ProtoValue {
                kind: Some(value::Kind::StringValue(key.clone())),
            },
        );
    }

    // Add custom fields
    for (key, value) in &ctx.custom_fields {
        if let Some(proto_value) = context_field_to_proto(value) {
            fields.insert(key.clone(), proto_value);
        }
    }

    Struct { fields }
}

fn context_field_to_proto(value: &EvaluationContextFieldValue) -> Option<ProtoValue> {
    match value {
        EvaluationContextFieldValue::Bool(b) => Some(ProtoValue {
            kind: Some(value::Kind::BoolValue(*b)),
        }),
        EvaluationContextFieldValue::Int(i) => Some(ProtoValue {
            kind: Some(value::Kind::NumberValue(*i as f64)),
        }),
        EvaluationContextFieldValue::Float(f) => Some(ProtoValue {
            kind: Some(value::Kind::NumberValue(*f)),
        }),
        EvaluationContextFieldValue::String(s) => Some(ProtoValue {
            kind: Some(value::Kind::StringValue(s.clone())),
        }),
        EvaluationContextFieldValue::DateTime(_) => {
            // DateTime not directly supported in protobuf Value
            None
        }
        EvaluationContextFieldValue::Struct(_) => {
            // Struct type not easily convertible
            None
        }
    }
}

fn proto_struct_to_openfeature(s: &Struct) -> StructValue {
    let mut fields = HashMap::new();
    for (key, value) in &s.fields {
        if let Some(of_value) = proto_value_to_openfeature(value) {
            fields.insert(key.clone(), of_value);
        }
    }
    StructValue { fields }
}

fn proto_value_to_openfeature(value: &ProtoValue) -> Option<Value> {
    match &value.kind {
        Some(value::Kind::BoolValue(b)) => Some(Value::Bool(*b)),
        Some(value::Kind::NumberValue(n)) => Some(Value::Float(*n)),
        Some(value::Kind::StringValue(s)) => Some(Value::String(s.clone())),
        Some(value::Kind::ListValue(list)) => {
            let values: Vec<Value> = list
                .values
                .iter()
                .filter_map(proto_value_to_openfeature)
                .collect();
            Some(Value::Array(values))
        }
        Some(value::Kind::StructValue(s)) => Some(Value::Struct(proto_struct_to_openfeature(s))),
        Some(value::Kind::NullValue(_)) | None => None,
    }
}

fn navigate_path(value: Option<Struct>, path: &str) -> Option<Struct> {
    let mut current = value?;

    for part in path.split('.') {
        let field = current.fields.get(part)?;
        match &field.kind {
            Some(value::Kind::StructValue(s)) => {
                current = s.clone();
            }
            _ => {
                // If we're at the last part, wrap the value in a struct
                let mut fields = HashMap::new();
                fields.insert(part.to_string(), field.clone());
                return Some(Struct { fields });
            }
        }
    }

    Some(current)
}

fn extract_bool_value(value: &Option<Struct>) -> Option<bool> {
    let s = value.as_ref()?;
    let (_, v) = s.fields.iter().next()?;
    match &v.kind {
        Some(value::Kind::BoolValue(b)) => Some(*b),
        _ => None,
    }
}

fn extract_number_value(value: &Option<Struct>) -> Option<f64> {
    let s = value.as_ref()?;
    let (_, v) = s.fields.iter().next()?;
    match &v.kind {
        Some(value::Kind::NumberValue(n)) => Some(*n),
        _ => None,
    }
}

fn extract_string_value(value: &Option<Struct>) -> Option<String> {
    let s = value.as_ref()?;
    let (_, v) = s.fields.iter().next()?;
    match &v.kind {
        Some(value::Kind::StringValue(s)) => Some(s.clone()),
        _ => None,
    }
}

fn map_resolve_reason(reason: i32) -> EvaluationReason {
    match ResolveReason::try_from(reason) {
        Ok(ResolveReason::Match) => EvaluationReason::TargetingMatch,
        Ok(ResolveReason::NoSegmentMatch) => EvaluationReason::Default,
        Ok(ResolveReason::FlagArchived) => EvaluationReason::Disabled,
        Ok(ResolveReason::TargetingKeyError) => EvaluationReason::Error,
        Ok(ResolveReason::Error) => EvaluationReason::Error,
        _ => EvaluationReason::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use confidence_resolver::proto::google::{value, Struct, Value as ProtoValue};
    use open_feature::{EvaluationContext, EvaluationContextFieldValue, EvaluationReason, Value};

    // ==================== parse_flag_path tests ====================
    // Similar to Go's TestParseFlagPath

    #[test]
    fn test_parse_flag_path_simple() {
        let (flag, path) = parse_flag_path("my-flag");
        assert_eq!(flag, "my-flag");
        assert_eq!(path, None);
    }

    #[test]
    fn test_parse_flag_path_with_single_level() {
        let (flag, path) = parse_flag_path("my-flag.value");
        assert_eq!(flag, "my-flag");
        assert_eq!(path, Some("value".to_string()));
    }

    #[test]
    fn test_parse_flag_path_with_nested_path() {
        let (flag, path) = parse_flag_path("my-flag.nested.value");
        assert_eq!(flag, "my-flag");
        assert_eq!(path, Some("nested.value".to_string()));
    }

    // ==================== convert_evaluation_context tests ====================
    // Similar to Go's TestFlattenedContextToProto and TestProcessTargetingKey

    #[test]
    fn test_convert_evaluation_context_with_targeting_key() {
        let ctx = EvaluationContext::default().with_targeting_key("user-123");
        let result = convert_evaluation_context(&ctx);

        assert!(result.fields.contains_key("targeting_key"));
        if let Some(value::Kind::StringValue(s)) = &result.fields.get("targeting_key").unwrap().kind
        {
            assert_eq!(s, "user-123");
        } else {
            panic!("Expected string value for targeting_key");
        }
    }

    #[test]
    fn test_convert_evaluation_context_without_targeting_key() {
        let ctx = EvaluationContext::default().with_custom_field("other", "value");
        let result = convert_evaluation_context(&ctx);

        assert!(!result.fields.contains_key("targeting_key"));
        assert!(result.fields.contains_key("other"));
    }

    #[test]
    fn test_convert_evaluation_context_empty() {
        let ctx = EvaluationContext::default();
        let result = convert_evaluation_context(&ctx);

        assert!(result.fields.is_empty());
    }

    #[test]
    fn test_convert_evaluation_context_with_multiple_fields() {
        let ctx = EvaluationContext::default()
            .with_targeting_key("user-123")
            .with_custom_field("string_field", "value")
            .with_custom_field("bool_field", true)
            .with_custom_field("int_field", 42i64)
            .with_custom_field("float_field", 3.14f64);

        let result = convert_evaluation_context(&ctx);

        assert_eq!(result.fields.len(), 5); // targeting_key + 4 custom fields
    }

    // ==================== context_field_to_proto tests ====================
    // Similar to Go's TestGoValueToProto

    #[test]
    fn test_context_field_to_proto_bool() {
        let result = context_field_to_proto(&EvaluationContextFieldValue::Bool(true));
        assert!(result.is_some());
        if let Some(ProtoValue {
            kind: Some(value::Kind::BoolValue(b)),
        }) = result
        {
            assert!(b);
        } else {
            panic!("Expected bool value");
        }
    }

    #[test]
    fn test_context_field_to_proto_int() {
        let result = context_field_to_proto(&EvaluationContextFieldValue::Int(42));
        assert!(result.is_some());
        if let Some(ProtoValue {
            kind: Some(value::Kind::NumberValue(n)),
        }) = result
        {
            assert!((n - 42.0).abs() < f64::EPSILON);
        } else {
            panic!("Expected number value");
        }
    }

    #[test]
    fn test_context_field_to_proto_float() {
        let result = context_field_to_proto(&EvaluationContextFieldValue::Float(3.14));
        assert!(result.is_some());
        if let Some(ProtoValue {
            kind: Some(value::Kind::NumberValue(n)),
        }) = result
        {
            assert!((n - 3.14).abs() < 0.001);
        } else {
            panic!("Expected number value");
        }
    }

    #[test]
    fn test_context_field_to_proto_string() {
        let result = context_field_to_proto(&EvaluationContextFieldValue::String("hello".into()));
        assert!(result.is_some());
        if let Some(ProtoValue {
            kind: Some(value::Kind::StringValue(s)),
        }) = result
        {
            assert_eq!(s, "hello");
        } else {
            panic!("Expected string value");
        }
    }

    // Note: DateTime test skipped as it requires `time` crate as dev-dependency
    // The EvaluationContextFieldValue::DateTime variant returns None from context_field_to_proto

    // ==================== proto_value_to_openfeature tests ====================
    // Similar to Go's TestProtoValueToGo

    #[test]
    fn test_proto_value_to_openfeature_bool() {
        let proto_value = ProtoValue {
            kind: Some(value::Kind::BoolValue(true)),
        };
        let result = proto_value_to_openfeature(&proto_value);
        assert_eq!(result, Some(Value::Bool(true)));
    }

    #[test]
    fn test_proto_value_to_openfeature_number() {
        let proto_value = ProtoValue {
            kind: Some(value::Kind::NumberValue(42.5)),
        };
        let result = proto_value_to_openfeature(&proto_value);
        assert_eq!(result, Some(Value::Float(42.5)));
    }

    #[test]
    fn test_proto_value_to_openfeature_string() {
        let proto_value = ProtoValue {
            kind: Some(value::Kind::StringValue("hello".to_string())),
        };
        let result = proto_value_to_openfeature(&proto_value);
        assert_eq!(result, Some(Value::String("hello".to_string())));
    }

    #[test]
    fn test_proto_value_to_openfeature_null() {
        let proto_value = ProtoValue {
            kind: Some(value::Kind::NullValue(0)),
        };
        let result = proto_value_to_openfeature(&proto_value);
        assert_eq!(result, None);
    }

    #[test]
    fn test_proto_value_to_openfeature_none() {
        let proto_value = ProtoValue { kind: None };
        let result = proto_value_to_openfeature(&proto_value);
        assert_eq!(result, None);
    }

    // Note: List test requires access to prost_types::ListValue which isn't directly exported
    // The proto_value_to_openfeature function handles lists correctly - tested via integration tests

    #[test]
    fn test_proto_value_to_openfeature_struct() {
        let mut fields = HashMap::new();
        fields.insert(
            "key".to_string(),
            ProtoValue {
                kind: Some(value::Kind::StringValue("value".to_string())),
            },
        );
        let proto_value = ProtoValue {
            kind: Some(value::Kind::StructValue(Struct { fields })),
        };
        let result = proto_value_to_openfeature(&proto_value);
        if let Some(Value::Struct(s)) = result {
            assert_eq!(s.fields.get("key"), Some(&Value::String("value".to_string())));
        } else {
            panic!("Expected struct value");
        }
    }

    // ==================== proto_struct_to_openfeature tests ====================
    // Similar to Go's TestProtoStructToGo

    #[test]
    fn test_proto_struct_to_openfeature_empty() {
        let proto_struct = Struct {
            fields: HashMap::new(),
        };
        let result = proto_struct_to_openfeature(&proto_struct);
        assert!(result.fields.is_empty());
    }

    #[test]
    fn test_proto_struct_to_openfeature_with_values() {
        let mut fields = HashMap::new();
        fields.insert(
            "name".to_string(),
            ProtoValue {
                kind: Some(value::Kind::StringValue("test".to_string())),
            },
        );
        fields.insert(
            "count".to_string(),
            ProtoValue {
                kind: Some(value::Kind::NumberValue(42.0)),
            },
        );
        fields.insert(
            "active".to_string(),
            ProtoValue {
                kind: Some(value::Kind::BoolValue(true)),
            },
        );
        let proto_struct = Struct { fields };

        let result = proto_struct_to_openfeature(&proto_struct);

        assert_eq!(
            result.fields.get("name"),
            Some(&Value::String("test".to_string()))
        );
        assert_eq!(result.fields.get("count"), Some(&Value::Float(42.0)));
        assert_eq!(result.fields.get("active"), Some(&Value::Bool(true)));
    }

    // ==================== navigate_path tests ====================
    // Similar to Go's TestGetValueForPath

    fn create_test_struct() -> Struct {
        let mut level3_fields = HashMap::new();
        level3_fields.insert(
            "level3".to_string(),
            ProtoValue {
                kind: Some(value::Kind::StringValue("deep-value".to_string())),
            },
        );
        let level3 = Struct {
            fields: level3_fields,
        };

        let mut level2_fields = HashMap::new();
        level2_fields.insert(
            "level2".to_string(),
            ProtoValue {
                kind: Some(value::Kind::StructValue(level3)),
            },
        );
        level2_fields.insert(
            "simple".to_string(),
            ProtoValue {
                kind: Some(value::Kind::StringValue("simple-value".to_string())),
            },
        );
        let level2 = Struct {
            fields: level2_fields,
        };

        let mut root_fields = HashMap::new();
        root_fields.insert(
            "level1".to_string(),
            ProtoValue {
                kind: Some(value::Kind::StructValue(level2)),
            },
        );
        root_fields.insert(
            "top".to_string(),
            ProtoValue {
                kind: Some(value::Kind::StringValue("top-value".to_string())),
            },
        );

        Struct {
            fields: root_fields,
        }
    }

    #[test]
    fn test_navigate_path_top_level() {
        let test_struct = create_test_struct();
        let result = navigate_path(Some(test_struct), "top");
        assert!(result.is_some());
        let s = result.unwrap();
        if let Some(ProtoValue {
            kind: Some(value::Kind::StringValue(v)),
        }) = s.fields.get("top")
        {
            assert_eq!(v, "top-value");
        } else {
            panic!("Expected string value at top");
        }
    }

    #[test]
    fn test_navigate_path_nested() {
        let test_struct = create_test_struct();
        let result = navigate_path(Some(test_struct), "level1.simple");
        assert!(result.is_some());
        let s = result.unwrap();
        if let Some(ProtoValue {
            kind: Some(value::Kind::StringValue(v)),
        }) = s.fields.get("simple")
        {
            assert_eq!(v, "simple-value");
        } else {
            panic!("Expected string value at level1.simple");
        }
    }

    #[test]
    fn test_navigate_path_deep_nested() {
        let test_struct = create_test_struct();
        let result = navigate_path(Some(test_struct), "level1.level2.level3");
        assert!(result.is_some());
        let s = result.unwrap();
        if let Some(ProtoValue {
            kind: Some(value::Kind::StringValue(v)),
        }) = s.fields.get("level3")
        {
            assert_eq!(v, "deep-value");
        } else {
            panic!("Expected string value at level1.level2.level3");
        }
    }

    #[test]
    fn test_navigate_path_nonexistent() {
        let test_struct = create_test_struct();
        let result = navigate_path(Some(test_struct), "does.not.exist");
        assert!(result.is_none());
    }

    #[test]
    fn test_navigate_path_none_input() {
        let result = navigate_path(None, "any.path");
        assert!(result.is_none());
    }

    // ==================== extract_*_value tests ====================

    #[test]
    fn test_extract_bool_value_valid() {
        let mut fields = HashMap::new();
        fields.insert(
            "enabled".to_string(),
            ProtoValue {
                kind: Some(value::Kind::BoolValue(true)),
            },
        );
        let s = Some(Struct { fields });
        let result = extract_bool_value(&s);
        assert_eq!(result, Some(true));
    }

    #[test]
    fn test_extract_bool_value_wrong_type() {
        let mut fields = HashMap::new();
        fields.insert(
            "value".to_string(),
            ProtoValue {
                kind: Some(value::Kind::StringValue("not a bool".to_string())),
            },
        );
        let s = Some(Struct { fields });
        let result = extract_bool_value(&s);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_bool_value_none() {
        let result = extract_bool_value(&None);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_number_value_valid() {
        let mut fields = HashMap::new();
        fields.insert(
            "count".to_string(),
            ProtoValue {
                kind: Some(value::Kind::NumberValue(42.5)),
            },
        );
        let s = Some(Struct { fields });
        let result = extract_number_value(&s);
        assert_eq!(result, Some(42.5));
    }

    #[test]
    fn test_extract_number_value_wrong_type() {
        let mut fields = HashMap::new();
        fields.insert(
            "value".to_string(),
            ProtoValue {
                kind: Some(value::Kind::BoolValue(true)),
            },
        );
        let s = Some(Struct { fields });
        let result = extract_number_value(&s);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_number_value_none() {
        let result = extract_number_value(&None);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_string_value_valid() {
        let mut fields = HashMap::new();
        fields.insert(
            "message".to_string(),
            ProtoValue {
                kind: Some(value::Kind::StringValue("hello".to_string())),
            },
        );
        let s = Some(Struct { fields });
        let result = extract_string_value(&s);
        assert_eq!(result, Some("hello".to_string()));
    }

    #[test]
    fn test_extract_string_value_wrong_type() {
        let mut fields = HashMap::new();
        fields.insert(
            "value".to_string(),
            ProtoValue {
                kind: Some(value::Kind::NumberValue(123.0)),
            },
        );
        let s = Some(Struct { fields });
        let result = extract_string_value(&s);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_string_value_none() {
        let result = extract_string_value(&None);
        assert_eq!(result, None);
    }

    // ==================== map_resolve_reason tests ====================

    #[test]
    fn test_map_resolve_reason_match() {
        let result = map_resolve_reason(ResolveReason::Match as i32);
        assert_eq!(result, EvaluationReason::TargetingMatch);
    }

    #[test]
    fn test_map_resolve_reason_no_segment_match() {
        let result = map_resolve_reason(ResolveReason::NoSegmentMatch as i32);
        assert_eq!(result, EvaluationReason::Default);
    }

    #[test]
    fn test_map_resolve_reason_flag_archived() {
        let result = map_resolve_reason(ResolveReason::FlagArchived as i32);
        assert_eq!(result, EvaluationReason::Disabled);
    }

    #[test]
    fn test_map_resolve_reason_targeting_key_error() {
        let result = map_resolve_reason(ResolveReason::TargetingKeyError as i32);
        assert_eq!(result, EvaluationReason::Error);
    }

    #[test]
    fn test_map_resolve_reason_error() {
        let result = map_resolve_reason(ResolveReason::Error as i32);
        assert_eq!(result, EvaluationReason::Error);
    }

    #[test]
    fn test_map_resolve_reason_unknown() {
        let result = map_resolve_reason(999); // Invalid value
        assert_eq!(result, EvaluationReason::Unknown);
    }

    // ==================== Provider tests ====================

    #[test]
    fn test_provider_options_new() {
        let options = ProviderOptions::new("test-secret");
        assert_eq!(options.client_secret, "test-secret");
        assert!(options.initialize_timeout_ms.is_none());
        assert!(options.state_poll_interval_ms.is_none());
        assert!(options.flush_interval_ms.is_none());
        assert!(options.materialization_store.is_none());
    }

    #[test]
    fn test_provider_options_with_initialize_timeout() {
        let options = ProviderOptions::new("test-secret").with_initialize_timeout(5000);
        assert_eq!(options.initialize_timeout_ms, Some(5000));
    }

    #[test]
    fn test_provider_options_with_state_poll_interval() {
        let options = ProviderOptions::new("test-secret").with_state_poll_interval(60000);
        assert_eq!(options.state_poll_interval_ms, Some(60000));
    }

    #[test]
    fn test_provider_options_with_confidence_materialization_store() {
        let options = ProviderOptions::new("test-secret").with_confidence_materialization_store();
        assert!(matches!(
            options.materialization_store,
            Some(MaterializationStoreConfig::ConfidenceRemote)
        ));
    }

    #[tokio::test]
    async fn test_provider_metadata() {
        let options = ProviderOptions::new("test-secret");
        let provider = ConfidenceProvider::new(options).expect("Failed to create provider");

        assert_eq!(provider.metadata().name, "confidence-local-resolver");
    }

    #[tokio::test]
    async fn test_provider_status_before_init() {
        let options = ProviderOptions::new("test-secret");
        let provider = ConfidenceProvider::new(options).expect("Failed to create provider");

        use open_feature::provider::FeatureProvider;
        assert!(matches!(provider.status(), ProviderStatus::NotReady));
    }
}
