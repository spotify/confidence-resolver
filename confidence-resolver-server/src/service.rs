use crate::{
    api::{
        self, resolve_process_response, ApplyFlagsRequest, ApplyFlagsResponse, ResolveFlagsRequest,
        ResolveFlagsResponse, ResolveProcessRequest, Sdk,
    },
    backend::Backend,
    reporting::Reporting,
};
use aes_gcm::{
    aead::{Aead, AeadCore, OsRng},
    Aes256Gcm, KeyInit, Nonce,
};
use arc_swap::ArcSwapOption;
use bytes::Bytes;
use confidence_resolver::proto::google::Struct;
use confidence_resolver::telemetry::{PrometheusConfig, Telemetry};
use confidence_resolver::{Client, FlagToApply, Host, ResolvedValue, ResolverState};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Instant;

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("Resolver state not set")]
    Unavailable,
    #[error("Not authenticated")]
    Unauthenticated,
    #[error("{0}")]
    Invalid(String),
    #[error("Materialization service unavailable")]
    Materializations,
}

struct Snapshot {
    state: ResolverState,
    log_key: Option<String>,
    sampler: Arc<crate::sampling::Sampler>,
}
impl Snapshot {
    // Stable across replicas/reloads, but distinct for every account and credential.
    // Call only after snapshot() has authenticated the requesting credential.
    fn token_key(&self, secret: &str) -> Bytes {
        let client = &self.state.secrets[secret];
        let mut hash = Sha256::new();
        for part in [
            "confidence-resolver-server/token/v1",
            &client.account.name,
            &client.client_credential_name,
            secret,
        ] {
            hash.update((part.len() as u64).to_be_bytes());
            hash.update(part.as_bytes());
        }
        Bytes::copy_from_slice(&hash.finalize())
    }
}
struct RequestContext {
    sdk: Option<Sdk>,
    sampler: Arc<crate::sampling::Sampler>,
    send_apply_logs: bool,
    logs: Mutex<api::WriteFlagLogsRequest>,
}
tokio::task_local! { static REQUEST_CONTEXT: Arc<RequestContext>; }

/// A single native account snapshot and connection pool shared by both transports.
pub struct ResolverService {
    state: ArcSwapOption<Snapshot>,
    backend: Arc<Backend>,
    pub reporting: Reporting,
    telemetry: Telemetry,
    send_apply_logs: bool,
    resolve_requests: AtomicU64,
    apply_requests: AtomicU64,
    failures: AtomicU64,
    materialization_write_failures: AtomicU64,
}

impl ResolverService {
    pub fn new(backend: Arc<Backend>, send_apply_logs: bool, capacity: usize) -> Self {
        Self {
            state: ArcSwapOption::empty(),
            backend,
            reporting: Reporting::new(capacity),
            telemetry: Telemetry::new(),
            send_apply_logs,
            resolve_requests: AtomicU64::new(0),
            apply_requests: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            materialization_write_failures: AtomicU64::new(0),
        }
    }
    pub fn replace_state(
        &self,
        state: ResolverState,
        log_key: Option<String>,
        fields: Vec<crate::sampling::FieldOverride>,
    ) {
        self.state.store(Some(Arc::new(Snapshot {
            state,
            log_key,
            sampler: Arc::new(crate::sampling::Sampler::new(fields)),
        })));
        if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            self.telemetry.set_last_state_update(now.as_millis() as u64);
        }
    }
    pub fn ready(&self) -> bool {
        self.state.load().is_some()
    }
    pub fn request_counts(&self) -> (u64, u64) {
        (
            self.resolve_requests.load(Ordering::Relaxed),
            self.apply_requests.load(Ordering::Relaxed),
        )
    }
    fn snapshot(&self, secret: &str) -> Result<Arc<Snapshot>, ServiceError> {
        if secret.is_empty() {
            return Err(ServiceError::Invalid("client_secret is required".into()));
        }
        let state = self.state.load_full().ok_or(ServiceError::Unavailable)?;
        // The core lookup error includes other keys' prefixes; never return it to callers.
        if !state.state.secrets.contains_key(secret) {
            return Err(ServiceError::Unauthenticated);
        }
        Ok(state)
    }
    fn context(
        &self,
        sdk: Option<Sdk>,
        sampler: Arc<crate::sampling::Sampler>,
    ) -> Arc<RequestContext> {
        Arc::new(RequestContext {
            sdk,
            sampler,
            send_apply_logs: self.send_apply_logs,
            logs: Mutex::new(api::WriteFlagLogsRequest::default()),
        })
    }
    fn collect_logs(&self, context: &RequestContext, secret: String) {
        let logs = std::mem::take(&mut *context.logs.lock().unwrap_or_else(|p| p.into_inner()));
        let mut samples = crate::sampling::Samples::new();
        if self.send_apply_logs {
            for client in &logs.client_resolve_info {
                samples.extend(context.sampler.snapshot(&client.client_credential));
            }
        }
        self.reporting.enqueue(secret, logs, samples);
    }

    pub async fn resolve(
        &self,
        request: ResolveFlagsRequest,
    ) -> Result<ResolveFlagsResponse, ServiceError> {
        self.resolve_requests.fetch_add(1, Ordering::Relaxed);
        let started = Instant::now();
        let result = self.resolve_inner(request).await;
        self.telemetry
            .record_latency_us(started.elapsed().as_micros().min(u32::MAX as u128) as u32);
        match &result {
            Ok(response) => {
                for flag in &response.resolved_flags {
                    self.telemetry.mark_resolve(flag.reason());
                }
            }
            Err(_) => {
                self.failures.fetch_add(1, Ordering::Relaxed);
            }
        }
        result
    }
    async fn resolve_inner(
        &self,
        request: ResolveFlagsRequest,
    ) -> Result<ResolveFlagsResponse, ServiceError> {
        let snapshot = self.snapshot(&request.client_secret)?;
        let resolver = snapshot
            .state
            .get_resolver::<ServerHost>(
                &request.client_secret,
                request.evaluation_context.clone().unwrap_or_default(),
                &snapshot.token_key(&request.client_secret),
            )
            .map_err(|_| ServiceError::Unauthenticated)?;
        let secret = request.client_secret.clone();
        let context = self.context(request.sdk.clone(), snapshot.sampler.clone());
        let result = REQUEST_CONTEXT
            .scope(context.clone(), async {
                let mut process = ResolveProcessRequest::deferred_materializations(request);
                // Bound malformed/non-progressing continuations, keeping this same snapshot throughout.
                for _ in 0..16 {
                    let response = resolver
                        .resolve_flags(process)
                        .map_err(ServiceError::Invalid)?;
                    match response.result {
                        Some(resolve_process_response::Result::Resolved(resolved)) => {
                            if !resolved.materializations_to_write.is_empty()
                                && self
                                    .backend
                                    .write_materializations(
                                        &secret,
                                        resolved.materializations_to_write,
                                    )
                                    .await
                                    .is_err()
                            {
                                self.materialization_write_failures
                                    .fetch_add(1, Ordering::Relaxed);
                                tracing::warn!("Materialization write failed");
                            }
                            return resolved.response.ok_or_else(|| {
                                ServiceError::Invalid("Empty resolve response".into())
                            });
                        }
                        Some(resolve_process_response::Result::Suspended(suspended)) => {
                            let records = self
                                .backend
                                .read_materializations(&secret, suspended.materializations_to_read)
                                .await
                                .map_err(|_| ServiceError::Materializations)?;
                            process = ResolveProcessRequest::resume(records, suspended.state);
                        }
                        None => return Err(ServiceError::Invalid("Empty resolve response".into())),
                    }
                }
                Err(ServiceError::Invalid(
                    "Materialization iteration limit exceeded".into(),
                ))
            })
            .await;
        self.collect_logs(&context, snapshot.log_key.clone().unwrap_or(secret));
        result
    }

    pub async fn apply(
        &self,
        request: ApplyFlagsRequest,
    ) -> Result<ApplyFlagsResponse, ServiceError> {
        self.apply_requests.fetch_add(1, Ordering::Relaxed);
        let result = self.apply_inner(request).await;
        if result.is_err() {
            self.failures.fetch_add(1, Ordering::Relaxed);
        }
        result
    }
    async fn apply_inner(
        &self,
        request: ApplyFlagsRequest,
    ) -> Result<ApplyFlagsResponse, ServiceError> {
        let snapshot = self.snapshot(&request.client_secret)?;
        if request.flags.is_empty() || request.resolve_token.is_empty() {
            return Err(ServiceError::Invalid(
                "flags and resolve_token are required".into(),
            ));
        }
        let mut flags = std::collections::HashSet::new();
        if request
            .flags
            .iter()
            .any(|flag| flag.flag.is_empty() || !flags.insert(&flag.flag))
        {
            return Err(ServiceError::Invalid(
                "flags must contain unique, nonempty names".into(),
            ));
        }
        let context = self.context(request.sdk.clone(), snapshot.sampler.clone());
        let result = REQUEST_CONTEXT
            .scope(context.clone(), async {
                snapshot
                    .state
                    .get_resolver::<ServerHost>(
                        &request.client_secret,
                        Struct::default(),
                        &snapshot.token_key(&request.client_secret),
                    )
                    .map_err(|_| ServiceError::Unauthenticated)?
                    .apply_flags(&request)
                    .map_err(ServiceError::Invalid)?;
                Ok(ApplyFlagsResponse {})
            })
            .await;
        self.collect_logs(
            &context,
            snapshot.log_key.clone().unwrap_or(request.client_secret),
        );
        result
    }

    pub fn metrics(&self) -> String {
        let mut output = self
            .telemetry
            .snapshot()
            .to_prometheus("0", &PrometheusConfig::default());
        let metrics = [
            ("ready", "gauge", u64::from(self.ready())),
            (
                "resolve_requests_total",
                "counter",
                self.resolve_requests.load(Ordering::Relaxed),
            ),
            (
                "apply_requests_total",
                "counter",
                self.apply_requests.load(Ordering::Relaxed),
            ),
            (
                "failures_total",
                "counter",
                self.failures.load(Ordering::Relaxed),
            ),
            (
                "materialization_write_failures_total",
                "counter",
                self.materialization_write_failures.load(Ordering::Relaxed),
            ),
            (
                "log_buffer_bytes",
                "gauge",
                self.reporting.pending_bytes() as u64,
            ),
            (
                "log_requests_dropped_total",
                "counter",
                self.reporting.dropped.load(Ordering::Relaxed),
            ),
            (
                "log_delivery_failures_total",
                "counter",
                self.reporting.failures.load(Ordering::Relaxed),
            ),
        ];
        for (name, kind, value) in metrics {
            output.push_str(&format!(
                "# TYPE confidence_server_{name} {kind}\nconfidence_server_{name} {value}\n"
            ));
        }
        output
    }
}

struct ServerHost;
impl Host for ServerHost {
    fn log_resolve(_: &str, context: &Struct, values: &[ResolvedValue<'_>], client: &Client) {
        REQUEST_CONTEXT.with(|request| {
            if request.send_apply_logs {
                for value in values {
                    let assigned: api::resolve_token_v1::AssignedFlag = value.into();
                    request.sampler.observe(
                        &client.client_credential_name,
                        &client.client_name,
                        &assigned.targeting_key,
                        context,
                    );
                }
            }
            let (flags, client) = confidence_resolver::resolve_logger::build_resolve_log(
                context,
                &client.client_credential_name,
                values,
            );
            let mut logs = request.logs.lock().unwrap_or_else(|p| p.into_inner());
            logs.client_resolve_info.push(client);
            logs.flag_resolve_info.extend(flags);
        });
    }
    fn log_assign(resolve_id: &str, flags: &[FlagToApply<'_>], client: &Client, _: &Option<Sdk>) {
        REQUEST_CONTEXT.with(|request| {
            if request.send_apply_logs {
                request
                    .logs
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .flag_assigned
                    .push(confidence_resolver::assign_logger::build_flag_assigned(
                        resolve_id,
                        flags,
                        client,
                        &request.sdk,
                    ));
            }
        });
    }
    fn encrypt_resolve_token(data: &[u8], key: &[u8]) -> Result<Vec<u8>, String> {
        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| "Invalid token key")?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let encrypted = cipher
            .encrypt(&nonce, data)
            .map_err(|_| "Token encryption failed")?;
        Ok([nonce.as_slice(), &encrypted].concat())
    }
    fn decrypt_resolve_token(data: &[u8], key: &[u8]) -> Result<Vec<u8>, String> {
        if data.len() < 28 {
            return Err("Invalid resolve token".into());
        }
        Aes256Gcm::new_from_slice(key)
            .map_err(|_| "Invalid token key")?
            .decrypt(Nonce::from_slice(&data[..12]), &data[12..])
            .map_err(|_| "Invalid resolve token".into())
    }
}
