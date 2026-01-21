//! Log management for sending flag logs to the Confidence API.

use prost::Message;
use reqwest_middleware::ClientWithMiddleware;

use confidence_resolver::assign_logger::AssignLogger;
use confidence_resolver::proto::confidence::flags::resolver::v1::WriteFlagLogsRequest;
use confidence_resolver::resolve_logger::ResolveLogger;

use crate::error::Result;
use crate::host::NativeHost;

/// API endpoint for flag logs.
const FLAG_LOGS_URL: &str = "https://resolver.confidence.dev/v1/clientFlagLogs:write";

/// Target size for log batches (4 MB).
const LOG_TARGET_BYTES: usize = 4 * 1024 * 1024;

/// Log sender that sends flag logs to the Confidence API.
pub struct LogSender {
    client: ClientWithMiddleware,
    client_secret: String,
}

impl LogSender {
    /// Create a new log sender with the given client and client secret.
    pub fn new(client: ClientWithMiddleware, client_secret: String) -> Self {
        Self {
            client,
            client_secret,
        }
    }

    /// Send encoded flag logs to the API.
    pub async fn send(&self, logs: &[u8]) -> Result<()> {
        if logs.is_empty() {
            return Ok(());
        }

        let response = self
            .client
            .post(FLAG_LOGS_URL)
            .header("Content-Type", "application/x-protobuf")
            .header(
                "Authorization",
                format!("ClientSecret {}", self.client_secret),
            )
            .body(logs.to_vec())
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            tracing::error!("Failed to send flag logs: {} - {}", status, body);
            // Don't return error for log sending failures to avoid disrupting flag evaluation
        }

        Ok(())
    }
}

/// Log manager that coordinates flushing logs from the loggers.
pub struct LogManager {
    sender: LogSender,
}

impl LogManager {
    /// Create a new log manager with the given client and client secret.
    pub fn new(client: ClientWithMiddleware, client_secret: String) -> Self {
        Self {
            sender: LogSender::new(client, client_secret),
        }
    }

    /// Flush all logs (both resolve and assign logs).
    pub async fn flush_all(
        &self,
        resolve_logger: &ResolveLogger<NativeHost>,
        assign_logger: &AssignLogger,
    ) -> Result<()> {
        let mut request = resolve_logger.checkpoint();
        assign_logger.checkpoint_fill_with_limit(&mut request, LOG_TARGET_BYTES, false);

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
