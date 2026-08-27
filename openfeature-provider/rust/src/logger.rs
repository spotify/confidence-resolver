//! Log management for sending flag logs to the Confidence API.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use prost::Message;
use rand::Rng;
use reqwest::StatusCode;
use reqwest_middleware::ClientWithMiddleware;
use tokio::sync::RwLock;

use confidence_resolver::assign_logger::AssignLogger;
use confidence_resolver::proto::confidence::flags::resolver::v1::{
    telemetry_data::ProviderInitRate, Sdk, WriteFlagLogsRequest,
};
use confidence_resolver::resolve_logger::ResolveLogger;

use crate::error::Result;
use crate::host::{NativeHost, LAST_FLUSHED, TELEMETRY};
use crate::state::LogDestination;

const EDGE_URL: &str = "https://resolver.confidence.dev/v1/clientFlagLogs:write";
const CLOUDFLARE_URL: &str =
    "https://epx-flags-logs.experimentation-platform.workers.dev/v1/flagLogs:ingest";

/// Target size for log batches (4 MB).
const LOG_TARGET_BYTES: usize = 4 * 1024 * 1024;
const INIT_PENDING: u8 = 0;
const INIT_SENDING: u8 = 1;
const INIT_SENT: u8 = 2;

struct InitTelemetryState(AtomicU8);

impl InitTelemetryState {
    fn new() -> Self {
        Self(AtomicU8::new(INIT_PENDING))
    }

    fn claim(&self) -> bool {
        self.0
            .compare_exchange(
                INIT_PENDING,
                INIT_SENDING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn complete(&self, success: bool) {
        self.0.store(
            if success { INIT_SENT } else { INIT_PENDING },
            Ordering::Release,
        );
    }
}

/// Maximum number of send attempts before giving up.
const MAX_ATTEMPTS: u32 = 3;

/// Initial delay between retry attempts.
const RETRY_BASE_DELAY: Duration = Duration::from_millis(500);

/// Multiplier applied to the delay after each failed attempt.
const RETRY_BACKOFF_MULTIPLIER: u32 = 2;

/// Jitter factor applied to retry delays (±10%).
const RETRY_JITTER: f64 = 0.1;

fn is_retryable_status(status: StatusCode) -> bool {
    status.is_server_error()
        || status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
}

fn apply_jitter(delay: Duration) -> Duration {
    let mut rng = rand::rng();
    let factor = 1.0 + rng.random_range(-RETRY_JITTER..RETRY_JITTER);
    delay.mul_f64(factor)
}

fn parse_retry_after(header: Option<&str>) -> Option<Duration> {
    let value = header?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    None
}

/// Log sender that sends flag logs to the Confidence API.
pub struct LogSender {
    client: ClientWithMiddleware,
    client_secret: String,
    account_id: Arc<RwLock<Option<String>>>,
    destinations: Arc<RwLock<Vec<LogDestination>>>,
}

impl LogSender {
    pub fn new(
        client: ClientWithMiddleware,
        client_secret: String,
        account_id: Arc<RwLock<Option<String>>>,
        destinations: Arc<RwLock<Vec<LogDestination>>>,
    ) -> Self {
        Self {
            client,
            client_secret,
            account_id,
            destinations,
        }
    }

    pub async fn send(&self, logs: &[u8]) -> Result<()> {
        if logs.is_empty() {
            return Ok(());
        }

        let destinations = {
            let d = self.destinations.read().await.clone();
            if d.is_empty() {
                vec![LogDestination::Edge]
            } else {
                d
            }
        };
        let account_id = self.account_id.read().await.clone();

        let (primary, fallback) = if destinations.len() >= 2 {
            (destinations[0], Some(destinations[1]))
        } else {
            (destinations[0], None)
        };

        let result = self
            .send_to_destination(primary, logs, account_id.as_deref())
            .await;

        if let Some(fb) = fallback {
            if result.is_err() {
                tracing::warn!(
                    "primary flag log destination {:?} failed, trying fallback {:?}",
                    primary,
                    fb
                );
                let _ = self
                    .send_to_destination(fb, logs, account_id.as_deref())
                    .await;
            }
        }

        Ok(())
    }

    async fn send_to_destination(
        &self,
        dest: LogDestination,
        logs: &[u8],
        account_id: Option<&str>,
    ) -> std::result::Result<(), ()> {
        let (url, body, content_type) = match dest {
            LogDestination::Edge => (EDGE_URL, logs.to_vec(), "application/x-protobuf"),
            LogDestination::Cloudflare => {
                let acct = account_id.unwrap_or_default();
                let body = encode_ingest_request(acct, logs);
                (CLOUDFLARE_URL, body, "application/protobuf")
            }
        };

        self.send_with_retry(url, body, content_type, dest).await
    }

    /// Send flag logs to a single destination, retrying on transient failures
    /// with exponential backoff and jitter. Respects the server's `Retry-After`
    /// header when present. Returns `Err(())` if the destination could not be
    /// reached after all attempts so callers can fall back to another destination.
    async fn send_with_retry(
        &self,
        url: &str,
        body: Vec<u8>,
        content_type: &str,
        dest: LogDestination,
    ) -> std::result::Result<(), ()> {
        let mut delay = RETRY_BASE_DELAY;

        for attempt in 1..=MAX_ATTEMPTS {
            let result = self
                .client
                .post(url)
                .header("Content-Type", content_type)
                .header(
                    "Authorization",
                    format!("ClientSecret {}", self.client_secret),
                )
                .body(body.clone())
                .send()
                .await;

            match result {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response) if is_retryable_status(response.status()) => {
                    let status = response.status();
                    if attempt < MAX_ATTEMPTS {
                        let server_delay = parse_retry_after(
                            response
                                .headers()
                                .get("retry-after")
                                .and_then(|v| v.to_str().ok()),
                        );
                        let sleep_dur = server_delay.unwrap_or_else(|| apply_jitter(delay));
                        tracing::debug!(
                            "Flag log send attempt {}/{} to {:?} failed with {}, retrying in {:?}",
                            attempt,
                            MAX_ATTEMPTS,
                            dest,
                            status,
                            sleep_dur
                        );
                        tokio::time::sleep(sleep_dur).await;
                        delay *= RETRY_BACKOFF_MULTIPLIER;
                    } else {
                        let resp_body = response.text().await.unwrap_or_default();
                        tracing::warn!(
                            "Failed to send flag logs to {:?} after {} attempts: {} - {}",
                            dest,
                            MAX_ATTEMPTS,
                            status,
                            resp_body
                        );
                        return Err(());
                    }
                }
                Ok(response) => {
                    let status = response.status();
                    let resp_body = response.text().await.unwrap_or_default();
                    tracing::error!(
                        "Failed to send flag logs to {:?}: {} - {}",
                        dest,
                        status,
                        resp_body
                    );
                    return Err(());
                }
                Err(e) => {
                    if attempt < MAX_ATTEMPTS {
                        let sleep_dur = apply_jitter(delay);
                        tracing::debug!(
                            "Flag log send attempt {}/{} to {:?} failed with {}, retrying in {:?}",
                            attempt,
                            MAX_ATTEMPTS,
                            dest,
                            e,
                            sleep_dur
                        );
                        tokio::time::sleep(sleep_dur).await;
                        delay *= RETRY_BACKOFF_MULTIPLIER;
                    } else {
                        tracing::warn!(
                            "Failed to send flag logs to {:?} after {} attempts: {}",
                            dest,
                            MAX_ATTEMPTS,
                            e
                        );
                        return Err(());
                    }
                }
            }
        }

        Err(())
    }
}

/// Request wrapper expected by the Cloudflare ingest worker. The `batch`
/// field holds an already-encoded `WriteFlagLogsRequest` — a length-delimited
/// bytes field is wire-identical to a nested message field.
#[derive(Clone, PartialEq, Message)]
struct IngestFlagLogsRequest {
    #[prost(string, tag = "1")]
    account_id: String,
    #[prost(bytes = "vec", tag = "2")]
    batch: Vec<u8>,
}

fn encode_ingest_request(account_id: &str, batch: &[u8]) -> Vec<u8> {
    IngestFlagLogsRequest {
        account_id: account_id.to_string(),
        batch: batch.to_vec(),
    }
    .encode_to_vec()
}

/// Log manager that coordinates flushing logs from the loggers.
pub struct LogManager {
    sender: LogSender,
    sdk: Sdk,
    init_labels: BTreeMap<String, String>,
    init_state: InitTelemetryState,
}

impl LogManager {
    /// Create a new log manager with the given client, client secret, and SDK identity.
    pub fn new(
        client: ClientWithMiddleware,
        client_secret: String,
        sdk: Sdk,
        account_id: Arc<RwLock<Option<String>>>,
        destinations: Arc<RwLock<Vec<LogDestination>>>,
        init_labels: BTreeMap<String, String>,
    ) -> Self {
        Self {
            sender: LogSender::new(client, client_secret, account_id, destinations),
            sdk,
            init_labels,
            init_state: InitTelemetryState::new(),
        }
    }

    /// Flush all logs (both resolve and assign logs), including telemetry deltas.
    pub async fn flush_all(
        &self,
        resolve_logger: &ResolveLogger<NativeHost>,
        assign_logger: &AssignLogger,
    ) -> Result<()> {
        let mut request = resolve_logger.checkpoint();
        assign_logger.checkpoint_fill_with_limit(&mut request, LOG_TARGET_BYTES, false);

        let mut td = TELEMETRY.delta_snapshot(&LAST_FLUSHED);
        td.sdk = Some(self.sdk.clone());
        let include_init = self.init_state.claim();
        if include_init {
            td.provider_init_rate.push(ProviderInitRate {
                count: 1,
                labels: self.init_labels.clone(),
            });
        }
        request.telemetry_data = Some(td);

        let encoded = request.encode_to_vec();
        if !encoded.is_empty() && has_logs(&request) {
            if let Err(error) = self.sender.send(&encoded).await {
                if include_init {
                    self.init_state.complete(false);
                }
                return Err(error);
            }
            if include_init {
                self.init_state.complete(true);
            }
        }

        Ok(())
    }

    /// Flush assign logs only (for more frequent flushing).
    pub async fn flush_assign(&self, assign_logger: &AssignLogger) -> Result<()> {
        let request = assign_logger.checkpoint_with_limit(LOG_TARGET_BYTES, true);

        let encoded = request.encode_to_vec();
        if !encoded.is_empty() && has_logs(&request) {
            self.sender.send(&encoded).await?;
        }

        Ok(())
    }
}

/// Check if a WriteFlagLogsRequest has any logs to send.
fn has_logs(request: &WriteFlagLogsRequest) -> bool {
    !request.flag_assigned.is_empty()
        || !request.client_resolve_info.is_empty()
        || !request.flag_resolve_info.is_empty()
        || request.telemetry_data.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Client;
    use reqwest_middleware::ClientBuilder;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TEST_PATH: &str = "/v1/clientFlagLogs:write";

    fn test_sender() -> LogSender {
        let client = ClientBuilder::new(Client::new()).build();
        LogSender {
            client,
            client_secret: "test-secret".to_string(),
            account_id: Arc::new(RwLock::new(None)),
            destinations: Arc::new(RwLock::new(vec![LogDestination::Edge])),
        }
    }

    async fn send_to(sender: &LogSender, server: &MockServer) -> std::result::Result<(), ()> {
        sender
            .send_with_retry(
                &format!("{}{}", server.uri(), TEST_PATH),
                b"test-payload".to_vec(),
                "application/x-protobuf",
                LogDestination::Edge,
            )
            .await
    }

    #[test]
    fn test_encode_ingest_request_roundtrip() {
        let account_id = "test-account-123";
        let batch = vec![0x0a, 0x0b, 0x0c, 0x0d];

        let encoded = encode_ingest_request(account_id, &batch);
        let decoded = IngestFlagLogsRequest::decode(encoded.as_slice()).unwrap();

        assert_eq!(decoded.account_id, account_id);
        assert_eq!(decoded.batch, batch);
    }

    #[test]
    fn test_encode_ingest_request_empty_account_id() {
        let encoded = encode_ingest_request("", &[0x01, 0x02]);
        let decoded = IngestFlagLogsRequest::decode(encoded.as_slice()).unwrap();

        assert_eq!(decoded.account_id, "");
        assert_eq!(decoded.batch, vec![0x01, 0x02]);
    }

    #[test]
    fn test_encode_ingest_request_empty_batch() {
        let encoded = encode_ingest_request("acct", &[]);
        let decoded = IngestFlagLogsRequest::decode(encoded.as_slice()).unwrap();

        assert_eq!(decoded.account_id, "acct");
        assert!(decoded.batch.is_empty());
    }

    #[test]
    fn init_telemetry_state_retries_failure_and_stops_after_success() {
        let state = InitTelemetryState::new();

        assert!(state.claim());
        assert!(!state.claim());
        state.complete(false);
        assert!(state.claim());
        state.complete(true);
        assert!(!state.claim());
    }

    #[tokio::test]
    async fn test_is_retryable_status() {
        assert!(is_retryable_status(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(is_retryable_status(StatusCode::BAD_GATEWAY));
        assert!(is_retryable_status(StatusCode::SERVICE_UNAVAILABLE));
        assert!(is_retryable_status(StatusCode::GATEWAY_TIMEOUT));
        assert!(is_retryable_status(StatusCode::REQUEST_TIMEOUT));
        assert!(is_retryable_status(StatusCode::TOO_MANY_REQUESTS));

        assert!(!is_retryable_status(StatusCode::OK));
        assert!(!is_retryable_status(StatusCode::BAD_REQUEST));
        assert!(!is_retryable_status(StatusCode::UNAUTHORIZED));
        assert!(!is_retryable_status(StatusCode::FORBIDDEN));
        assert!(!is_retryable_status(StatusCode::NOT_FOUND));
    }

    #[test]
    fn test_apply_jitter_within_bounds() {
        let base = Duration::from_millis(1000);
        for _ in 0..100 {
            let jittered = apply_jitter(base);
            assert!(jittered >= Duration::from_millis(900));
            assert!(jittered <= Duration::from_millis(1100));
        }
    }

    #[test]
    fn test_parse_retry_after() {
        assert_eq!(parse_retry_after(Some("5")), Some(Duration::from_secs(5)));
        assert_eq!(parse_retry_after(Some(" 5 ")), Some(Duration::from_secs(5)));
        assert_eq!(parse_retry_after(Some("abc")), None);
        assert_eq!(parse_retry_after(Some("")), None);
        assert_eq!(parse_retry_after(None), None);
    }

    #[tokio::test]
    async fn send_succeeds_on_first_attempt() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(TEST_PATH))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let sender = test_sender();
        assert!(send_to(&sender, &server).await.is_ok());
    }

    #[tokio::test]
    async fn retries_on_503_up_to_max_attempts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(TEST_PATH))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&server)
            .await;

        let sender = test_sender();
        assert!(send_to(&sender, &server).await.is_err());
    }

    #[tokio::test]
    async fn retries_on_429_and_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(TEST_PATH))
            .respond_with(ResponseTemplate::new(429))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(TEST_PATH))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let sender = test_sender();
        assert!(send_to(&sender, &server).await.is_ok());
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn no_retry_on_client_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(TEST_PATH))
            .respond_with(ResponseTemplate::new(400))
            .expect(1)
            .mount(&server)
            .await;

        let sender = test_sender();
        assert!(send_to(&sender, &server).await.is_err());
    }

    #[tokio::test]
    async fn no_retry_on_403() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(TEST_PATH))
            .respond_with(ResponseTemplate::new(403))
            .expect(1)
            .mount(&server)
            .await;

        let sender = test_sender();
        assert!(send_to(&sender, &server).await.is_err());
    }

    #[tokio::test]
    async fn respects_retry_after_header() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(TEST_PATH))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "1"))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(TEST_PATH))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let sender = test_sender();
        let start = std::time::Instant::now();
        assert!(send_to(&sender, &server).await.is_ok());
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_secs(2));
    }

    #[tokio::test]
    async fn retries_on_network_error() {
        // Stop the server so requests fail at the transport layer.
        let server = MockServer::start().await;
        let uri = server.uri();
        drop(server);

        let sender = test_sender();
        let result = sender
            .send_with_retry(
                &format!("{}{}", uri, TEST_PATH),
                b"test-payload".to_vec(),
                "application/x-protobuf",
                LogDestination::Edge,
            )
            .await;
        assert!(result.is_err());
    }
}
