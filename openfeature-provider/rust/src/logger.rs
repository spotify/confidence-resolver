//! Log management for sending flag logs to the Confidence API.

use std::time::Duration;

use prost::Message;
use reqwest::StatusCode;
use reqwest_middleware::ClientWithMiddleware;

use confidence_resolver::assign_logger::AssignLogger;
use confidence_resolver::proto::confidence::flags::resolver::v1::{Sdk, WriteFlagLogsRequest};
use confidence_resolver::resolve_logger::ResolveLogger;

use crate::error::Result;
use crate::host::{NativeHost, LAST_FLUSHED, TELEMETRY};

/// API endpoint for flag logs.
const FLAG_LOGS_URL: &str = "https://resolver.confidence.dev/v1/clientFlagLogs:write";

/// Target size for log batches (4 MB).
const LOG_TARGET_BYTES: usize = 4 * 1024 * 1024;

/// Maximum number of send attempts before giving up.
const MAX_ATTEMPTS: u32 = 3;

/// Initial delay between retry attempts.
const RETRY_BASE_DELAY: Duration = Duration::from_millis(500);

/// Multiplier applied to the delay after each failed attempt.
const RETRY_BACKOFF_MULTIPLIER: u32 = 2;

fn is_retryable_status(status: StatusCode) -> bool {
    status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT || status == StatusCode::TOO_MANY_REQUESTS
}

/// Log sender that sends flag logs to the Confidence API.
pub struct LogSender {
    client: ClientWithMiddleware,
    client_secret: String,
    url: String,
}

impl LogSender {
    /// Create a new log sender with the given client and client secret.
    pub fn new(client: ClientWithMiddleware, client_secret: String) -> Self {
        Self {
            client,
            client_secret,
            url: FLAG_LOGS_URL.to_string(),
        }
    }

    /// Send encoded flag logs to the API, retrying on transient failures.
    pub async fn send(&self, logs: &[u8]) -> Result<()> {
        if logs.is_empty() {
            return Ok(());
        }

        let mut delay = RETRY_BASE_DELAY;

        for attempt in 1..=MAX_ATTEMPTS {
            let result = self
                .client
                .post(&self.url)
                .header("Content-Type", "application/x-protobuf")
                .header(
                    "Authorization",
                    format!("ClientSecret {}", self.client_secret),
                )
                .body(logs.to_vec())
                .send()
                .await;

            match result {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response) if is_retryable_status(response.status()) => {
                    let status = response.status();
                    if attempt < MAX_ATTEMPTS {
                        tracing::debug!(
                            "Flag log send attempt {}/{} failed with {}, retrying in {:?}",
                            attempt,
                            MAX_ATTEMPTS,
                            status,
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        delay *= RETRY_BACKOFF_MULTIPLIER;
                    } else {
                        let body = response.text().await.unwrap_or_default();
                        tracing::warn!(
                            "Failed to send flag logs after {} attempts: {} - {}",
                            MAX_ATTEMPTS,
                            status,
                            body
                        );
                    }
                }
                Ok(response) => {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    tracing::error!("Failed to send flag logs: {} - {}", status, body);
                    return Ok(());
                }
                Err(e) => {
                    if attempt < MAX_ATTEMPTS {
                        tracing::debug!(
                            "Flag log send attempt {}/{} failed with {}, retrying in {:?}",
                            attempt,
                            MAX_ATTEMPTS,
                            e,
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        delay *= RETRY_BACKOFF_MULTIPLIER;
                    } else {
                        tracing::warn!(
                            "Failed to send flag logs after {} attempts: {}",
                            MAX_ATTEMPTS,
                            e
                        );
                    }
                }
            }
        }

        Ok(())
    }
}

/// Log manager that coordinates flushing logs from the loggers.
pub struct LogManager {
    sender: LogSender,
    sdk: Sdk,
}

impl LogManager {
    /// Create a new log manager with the given client, client secret, and SDK identity.
    pub fn new(client: ClientWithMiddleware, client_secret: String, sdk: Sdk) -> Self {
        Self {
            sender: LogSender::new(client, client_secret),
            sdk,
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
        request.telemetry_data = Some(td);

        let encoded = request.encode_to_vec();
        if !encoded.is_empty() && has_logs(&request) {
            self.sender.send(&encoded).await?;
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

    fn test_sender(url: &str) -> LogSender {
        let client = ClientBuilder::new(Client::new()).build();
        LogSender {
            client,
            client_secret: "test-secret".to_string(),
            url: url.to_string(),
        }
    }

    #[tokio::test]
    async fn send_empty_logs_is_noop() {
        let server = MockServer::start().await;
        let sender = test_sender(&format!("{}/v1/clientFlagLogs:write", server.uri()));
        sender.send(&[]).await.unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn send_succeeds_on_first_attempt() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/clientFlagLogs:write"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let sender = test_sender(&format!("{}/v1/clientFlagLogs:write", server.uri()));
        sender.send(b"test-payload").await.unwrap();
    }

    #[tokio::test]
    async fn retries_on_503_up_to_max_attempts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/clientFlagLogs:write"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&server)
            .await;

        let sender = test_sender(&format!("{}/v1/clientFlagLogs:write", server.uri()));
        sender.send(b"test-payload").await.unwrap();
    }

    #[tokio::test]
    async fn retries_on_429() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/clientFlagLogs:write"))
            .respond_with(ResponseTemplate::new(429))
            .expect(3)
            .mount(&server)
            .await;

        let sender = test_sender(&format!("{}/v1/clientFlagLogs:write", server.uri()));
        sender.send(b"test-payload").await.unwrap();
    }

    #[tokio::test]
    async fn retries_on_408() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/clientFlagLogs:write"))
            .respond_with(ResponseTemplate::new(408))
            .expect(3)
            .mount(&server)
            .await;

        let sender = test_sender(&format!("{}/v1/clientFlagLogs:write", server.uri()));
        sender.send(b"test-payload").await.unwrap();
    }

    #[tokio::test]
    async fn does_not_retry_on_400() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/clientFlagLogs:write"))
            .respond_with(ResponseTemplate::new(400))
            .expect(1)
            .mount(&server)
            .await;

        let sender = test_sender(&format!("{}/v1/clientFlagLogs:write", server.uri()));
        sender.send(b"test-payload").await.unwrap();
    }

    #[tokio::test]
    async fn does_not_retry_on_403() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/clientFlagLogs:write"))
            .respond_with(ResponseTemplate::new(403))
            .expect(1)
            .mount(&server)
            .await;

        let sender = test_sender(&format!("{}/v1/clientFlagLogs:write", server.uri()));
        sender.send(b"test-payload").await.unwrap();
    }

    #[test]
    fn retryable_status_codes() {
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
}
