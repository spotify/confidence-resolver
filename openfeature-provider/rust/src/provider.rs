//! OpenFeature provider implementation for Confidence.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use open_feature::provider::{FeatureProvider, ProviderMetadata, ProviderStatus, ResolutionDetails};
use open_feature::{
    EvaluationContext, EvaluationContextFieldValue, EvaluationError, EvaluationErrorCode,
    EvaluationReason, EvaluationResult, StructValue, Value,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use confidence_resolver::proto::confidence::flags::resolver::v1::{
    ResolveFlagsRequest, ResolveReason, ResolveWithStickyRequest, Sdk, SdkId,
};
use confidence_resolver::proto::google::{value, Struct, Value as ProtoValue};

use crate::error::{Error, Result};
use crate::host::{NativeHost, ASSIGN_LOGGER, RESOLVE_LOGGER};
use crate::logger::LogManager;
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

/// Configuration options for the Confidence provider.
#[derive(Clone)]
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
}

/// OpenFeature provider for Confidence using native Rust resolver.
pub struct ConfidenceProvider {
    metadata: ProviderMetadata,
    options: ProviderOptions,
    state: Arc<SharedState>,
    state_fetcher: Arc<StateFetcher>,
    log_manager: Arc<LogManager>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    background_tasks: Vec<JoinHandle<()>>,
    status: ProviderStatus,
}

impl ConfidenceProvider {
    /// Create a new Confidence provider.
    pub fn new(options: ProviderOptions) -> Result<Self> {
        let state_fetcher = Arc::new(StateFetcher::new(options.client_secret.clone())?);
        let log_manager = Arc::new(LogManager::new(options.client_secret.clone())?);

        Ok(Self {
            metadata: ProviderMetadata::new("confidence-local-resolver"),
            options,
            state: Arc::new(SharedState::new()),
            state_fetcher,
            log_manager,
            shutdown_tx: None,
            background_tasks: Vec::new(),
            status: ProviderStatus::NotReady,
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

        let state_poll_interval = self
            .options
            .state_poll_interval_ms
            .unwrap_or(DEFAULT_STATE_POLL_INTERVAL_MS);
        let flush_interval = self
            .options
            .flush_interval_ms
            .unwrap_or(DEFAULT_FLUSH_INTERVAL_MS);
        let assign_flush_interval = self
            .options
            .assign_flush_interval_ms
            .unwrap_or(DEFAULT_ASSIGN_FLUSH_INTERVAL_MS);

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
    fn resolve_flag(
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
            client_secret: self.options.client_secret.clone(),
            sdk: Some(Sdk {
                sdk: Some(
                    confidence_resolver::proto::confidence::flags::resolver::v1::sdk::Sdk::Id(
                        SdkId::RustProvider as i32,
                    ),
                ),
                version: VERSION.to_string(),
            }),
            ..Default::default()
        };

        // Create sticky request (without materializations for now)
        let sticky_request = ResolveWithStickyRequest {
            resolve_request: Some(request),
            materializations: vec![],
            fail_fast_on_sticky: false,
            not_process_sticky: true,
        };

        // Get resolver
        let resolver = state
            .get_resolver::<NativeHost>(
                &self.options.client_secret,
                proto_context,
                &ENCRYPTION_KEY,
            )
            .map_err(|e| {
                EvaluationError::builder()
                    .code(EvaluationErrorCode::General(format!(
                        "Failed to get resolver: {}",
                        e
                    )))
                    .build()
            })?;

        // Resolve
        let response = resolver.resolve_flags_sticky(&sticky_request).map_err(|e| {
            EvaluationError::builder()
                .code(EvaluationErrorCode::General(format!(
                    "Failed to resolve: {}",
                    e
                )))
                .build()
        })?;

        // Extract result
        use confidence_resolver::proto::confidence::flags::resolver::v1::resolve_with_sticky_response::ResolveResult as ProtoResolveResult;
        let success = match response.resolve_result {
            Some(ProtoResolveResult::Success(s)) => s,
            Some(ProtoResolveResult::ReadOpsRequest(_)) => {
                return Err(EvaluationError::builder()
                    .code(EvaluationErrorCode::General(
                        "Missing materializations".to_string(),
                    ))
                    .build());
            }
            None => {
                return Err(EvaluationError::builder()
                    .code(EvaluationErrorCode::General("No resolve result".to_string()))
                    .build());
            }
        };

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
        let result = self.resolve_flag(flag_key, evaluation_context)?;

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
        let result = self.resolve_flag(flag_key, evaluation_context)?;

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
        let result = self.resolve_flag(flag_key, evaluation_context)?;

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
        let result = self.resolve_flag(flag_key, evaluation_context)?;

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
        let result = self.resolve_flag(flag_key, evaluation_context)?;

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

    // Add targeting key as visitor_id (Confidence uses visitor_id for targeting)
    if let Some(ref key) = ctx.targeting_key {
        fields.insert(
            "visitor_id".to_string(),
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
