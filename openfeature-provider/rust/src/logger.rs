//! Log management for sending flag logs to the Confidence API.

use std::sync::Arc;

use prost::Message;
use reqwest_middleware::ClientWithMiddleware;
use tokio::sync::RwLock;

use confidence_resolver::assign_logger::AssignLogger;
use confidence_resolver::proto::confidence::flags::resolver::v1::{Sdk, WriteFlagLogsRequest};
use confidence_resolver::resolve_logger::ResolveLogger;

use crate::error::Result;
use crate::host::{NativeHost, LAST_FLUSHED, TELEMETRY};
use crate::state::LogDestination;

const EDGE_URL: &str = "https://resolver.confidence.dev/v1/clientFlagLogs:write";
const CLOUDFLARE_URL: &str =
    "https://epx-flags-logs.experimentation-platform.workers.dev/v1/flagLogs:ingest";

/// Target size for log batches (4 MB).
const LOG_TARGET_BYTES: usize = 4 * 1024 * 1024;

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

        let destinations = self.destinations.read().await.clone();
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

        let response = self
            .client
            .post(url)
            .header("Content-Type", content_type)
            .header(
                "Authorization",
                format!("ClientSecret {}", self.client_secret),
            )
            .body(body)
            .send()
            .await;

        match response {
            Ok(resp) if resp.status().is_success() => Ok(()),
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                tracing::error!(
                    "Failed to send flag logs to {:?}: {} - {}",
                    dest,
                    status,
                    body
                );
                Err(())
            }
            Err(e) => {
                tracing::error!("Failed to send flag logs to {:?}: {}", dest, e);
                Err(())
            }
        }
    }
}

fn encode_ingest_request(account_id: &str, batch: &[u8]) -> Vec<u8> {
    let account_bytes = account_id.as_bytes();
    let mut body = Vec::new();
    // field 1: string account_id (tag = 0x0a)
    body.push(0x0a);
    encode_varint(account_bytes.len() as u64, &mut body);
    body.extend_from_slice(account_bytes);
    // field 2: WriteFlagLogsRequest batch (tag = 0x12)
    body.push(0x12);
    encode_varint(batch.len() as u64, &mut body);
    body.extend_from_slice(batch);
    body
}

fn encode_varint(mut value: u64, buf: &mut Vec<u8>) {
    while value >= 0x80 {
        buf.push((value as u8) | 0x80);
        value >>= 7;
    }
    buf.push(value as u8);
}

/// Log manager that coordinates flushing logs from the loggers.
pub struct LogManager {
    sender: LogSender,
    sdk: Sdk,
}

impl LogManager {
    pub fn new(
        client: ClientWithMiddleware,
        client_secret: String,
        sdk: Sdk,
        account_id: Arc<RwLock<Option<String>>>,
        destinations: Arc<RwLock<Vec<LogDestination>>>,
    ) -> Self {
        Self {
            sender: LogSender::new(client, client_secret, account_id, destinations),
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

    /// Helper to decode a varint from a byte slice, returning (value, bytes_consumed).
    fn decode_varint(buf: &[u8]) -> (u64, usize) {
        let mut value: u64 = 0;
        let mut shift = 0;
        for (i, &byte) in buf.iter().enumerate() {
            value |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return (value, i + 1);
            }
            shift += 7;
        }
        panic!("unterminated varint");
    }

    /// Decode the manually-encoded ingest request protobuf, returning (account_id, batch).
    fn decode_ingest_fields(data: &[u8]) -> (String, Vec<u8>) {
        let mut account_id = String::new();
        let mut batch = Vec::new();
        let mut pos = 0;

        while pos < data.len() {
            let tag_byte = data[pos];
            pos += 1;
            let field_number = tag_byte >> 3;
            // wire type 2 (length-delimited)
            let (len, consumed) = decode_varint(&data[pos..]);
            pos += consumed;
            let field_data = &data[pos..pos + len as usize];
            pos += len as usize;

            match field_number {
                1 => account_id = String::from_utf8(field_data.to_vec()).unwrap(),
                2 => batch = field_data.to_vec(),
                _ => {}
            }
        }

        (account_id, batch)
    }

    #[test]
    fn test_encode_ingest_request_roundtrip() {
        let account_id = "test-account-123";
        let batch = vec![0x0a, 0x0b, 0x0c, 0x0d];

        let encoded = encode_ingest_request(account_id, &batch);
        let (decoded_account, decoded_batch) = decode_ingest_fields(&encoded);

        assert_eq!(decoded_account, account_id);
        assert_eq!(decoded_batch, batch);
    }

    #[test]
    fn test_encode_ingest_request_empty_account_id() {
        let encoded = encode_ingest_request("", &[0x01, 0x02]);
        let (decoded_account, decoded_batch) = decode_ingest_fields(&encoded);

        assert_eq!(decoded_account, "");
        assert_eq!(decoded_batch, vec![0x01, 0x02]);
    }

    #[test]
    fn test_encode_ingest_request_empty_batch() {
        let encoded = encode_ingest_request("acct", &[]);
        let (decoded_account, decoded_batch) = decode_ingest_fields(&encoded);

        assert_eq!(decoded_account, "acct");
        assert!(decoded_batch.is_empty());
    }

    #[test]
    fn test_encode_varint_small() {
        let mut buf = Vec::new();
        encode_varint(5, &mut buf);
        assert_eq!(buf, vec![0x05]);
    }

    #[test]
    fn test_encode_varint_large() {
        // 300 = 0b100101100 -> varint bytes: 0xAC 0x02
        let mut buf = Vec::new();
        encode_varint(300, &mut buf);
        assert_eq!(buf, vec![0xAC, 0x02]);
    }
}
