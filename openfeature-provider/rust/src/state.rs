//! State management for fetching and updating resolver state from CDN.

use std::sync::Arc;

use arc_swap::ArcSwapOption;
use bytes::Bytes;
use prost::Message;
use reqwest_middleware::ClientWithMiddleware;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use confidence_resolver::proto::confidence::flags::admin::v1::ResolverState as ResolverStatePb;
use confidence_resolver::proto::confidence::flags::resolver::v1::Sdk;
use confidence_resolver::ResolverState;

use crate::error::{Error, Result};

/// CDN base URL for fetching resolver state.
const CDN_BASE_URL: &str = "https://confidence-resolver-state-cdn.spotifycdn.com";

/// SetResolverStateRequest message from the CDN.
/// This is a simple protobuf message containing the state and account ID.
#[derive(Clone, PartialEq, Message)]
pub struct SetResolverStateRequest {
    #[prost(bytes = "bytes", tag = "1")]
    pub state: Bytes,
    #[prost(string, tag = "2")]
    pub account_id: String,
}

/// State fetcher that retrieves resolver state from the CDN.
pub struct StateFetcher {
    client: ClientWithMiddleware,
    client_secret: String,
    cdn_url: String,
    etag: RwLock<Option<String>>,
    sdk: Option<Sdk>,
    encryption_key: Option<Vec<u8>>,
}

impl StateFetcher {
    /// Create a new state fetcher with the given client, client secret, and sdk identity.
    pub fn new(
        client: ClientWithMiddleware,
        client_secret: String,
        sdk: Option<Sdk>,
        encryption_key_hex: Option<String>,
    ) -> Self {
        let hash = Self::hash_client_secret(&client_secret);
        let cdn_url = format!("{}/{}", CDN_BASE_URL, hash);
        let encryption_key = encryption_key_hex.map(|hex_str| {
            hex::decode(&hex_str).expect("encryption_key must be valid hex")
        });

        Self {
            client,
            client_secret,
            cdn_url,
            etag: RwLock::new(None),
            sdk,
            encryption_key,
        }
    }

    /// Hash the client secret using SHA-256 to create the CDN URL path.
    fn hash_client_secret(secret: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(secret.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Fetch the latest state from the CDN.
    ///
    /// Returns `None` if the state has not changed (304 Not Modified).
    /// Returns `Some((state, account_id))` if new state was fetched.
    pub async fn fetch(&self) -> Result<Option<(ResolverState, String)>> {
        let mut request = self.client.get(&self.cdn_url);

        // Add If-None-Match header if we have an ETag
        {
            let etag = self.etag.read().await;
            if let Some(ref etag_value) = *etag {
                request = request.header("If-None-Match", etag_value);
            }
        }

        let response = request.send().await.map_err(|e| {
            tracing::warn!("Failed to fetch state from {}: {}", self.cdn_url, e);
            e
        })?;

        // Check for 304 Not Modified
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(None);
        }

        // Check for successful response
        if !response.status().is_success() {
            return Err(Error::StateFetch(format!(
                "CDN returned status {}",
                response.status()
            )));
        }

        // Update ETag if present
        if let Some(etag_value) = response.headers().get("etag") {
            if let Ok(etag_str) = etag_value.to_str() {
                let mut etag = self.etag.write().await;
                *etag = Some(etag_str.to_string());
            }
        }

        // Check encryption header
        let encrypted_header = response
            .headers()
            .get("x-amz-meta-encrypted")
            .and_then(|v| v.to_str().ok())
            == Some("true");

        // Parse response body
        let raw_bytes = response.bytes().await?;

        // Try unencrypted path first (header check + protobuf fallback)
        let decrypted_bytes = if !encrypted_header {
            match SetResolverStateRequest::decode(raw_bytes.clone()) {
                Ok(request) => {
                    let state_pb = ResolverStatePb::decode(request.state).map_err(|e| {
                        Error::StateParse(format!("Failed to decode ResolverState: {}", e))
                    })?;
                    let state =
                        ResolverState::from_proto(state_pb, &request.account_id, self.sdk.clone())
                            .map_err(|e| {
                                Error::StateParse(format!(
                                    "Failed to create ResolverState: {:?}",
                                    e
                                ))
                            })?;
                    return Ok(Some((state, request.account_id)));
                }
                Err(_) => {
                    tracing::warn!("Protobuf decode failed, treating state as encrypted");
                    Self::decrypt(&raw_bytes, &self.encryption_key)?
                }
            }
        } else {
            Self::decrypt(&raw_bytes, &self.encryption_key)?
        };

        let request = SetResolverStateRequest::decode(decrypted_bytes.as_slice()).map_err(|e| {
            Error::StateParse(format!(
                "Failed to decode decrypted SetResolverStateRequest: {}",
                e
            ))
        })?;

        let state_pb = ResolverStatePb::decode(request.state)
            .map_err(|e| Error::StateParse(format!("Failed to decode ResolverState: {}", e)))?;

        let state = ResolverState::from_proto(state_pb, &request.account_id, self.sdk.clone())
            .map_err(|e| Error::StateParse(format!("Failed to create ResolverState: {:?}", e)))?;

        Ok(Some((state, request.account_id)))
    }

    /// Decrypt AES-256-GCM encrypted state (Tink NO_PREFIX format).
    fn decrypt(data: &[u8], key: &Option<Vec<u8>>) -> Result<Vec<u8>> {
        use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};

        let key_bytes = key.as_ref().ok_or_else(|| {
            Error::StateParse(
                "Resolver state is encrypted but no encryption_key was provided. \
                 Set the encryption key for this client credential."
                    .to_string(),
            )
        })?;

        if data.len() < 12 {
            return Err(Error::StateParse(
                "Encrypted state too short (missing nonce)".to_string(),
            ));
        }

        let cipher = Aes256Gcm::new_from_slice(key_bytes)
            .map_err(|e| Error::StateParse(format!("Invalid encryption key: {}", e)))?;
        let nonce = Nonce::from_slice(&data[..12]);
        cipher
            .decrypt(nonce, &data[12..])
            .map_err(|_| {
                Error::StateParse(
                    "Failed to decrypt resolver state: invalid key or corrupted data".to_string(),
                )
            })
    }

    /// Get the client secret.
    pub fn client_secret(&self) -> &str {
        &self.client_secret
    }
}

/// Shared state holder that can be atomically updated.
pub struct SharedState {
    state: ArcSwapOption<ResolverState>,
    account_id: RwLock<Option<String>>,
}

impl SharedState {
    /// Create a new empty shared state.
    pub fn new() -> Self {
        Self {
            state: ArcSwapOption::empty(),
            account_id: RwLock::new(None),
        }
    }

    /// Update the state and account ID.
    pub async fn update(&self, state: ResolverState, account_id: String) {
        self.state.store(Some(Arc::new(state)));
        let mut id = self.account_id.write().await;
        *id = Some(account_id);
    }

    /// Get the current state, if available.
    pub fn get(&self) -> Option<Arc<ResolverState>> {
        self.state.load_full()
    }

    /// Get the current account ID, if available.
    pub async fn account_id(&self) -> Option<String> {
        self.account_id.read().await.clone()
    }

    /// Check if state is initialized.
    pub fn is_initialized(&self) -> bool {
        self.state.load().is_some()
    }
}

impl Default for SharedState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{create_minimal_state, create_state_with_flag};
    use std::path::PathBuf;

    fn data_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("data")
    }

    #[test]
    fn test_hash_client_secret() {
        let hash = StateFetcher::hash_client_secret("test-secret");
        // SHA-256 produces 64 hex characters
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_hash_client_secret_consistency() {
        // Same input should produce same hash
        let hash1 = StateFetcher::hash_client_secret("my-secret");
        let hash2 = StateFetcher::hash_client_secret("my-secret");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_client_secret_different_inputs() {
        // Different inputs should produce different hashes
        let hash1 = StateFetcher::hash_client_secret("secret-1");
        let hash2 = StateFetcher::hash_client_secret("secret-2");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_shared_state_new() {
        let state = SharedState::new();
        assert!(!state.is_initialized());
        assert!(state.get().is_none());
    }

    #[tokio::test]
    async fn test_shared_state_update() {
        let shared_state = SharedState::new();
        assert!(!shared_state.is_initialized());

        let (state, account_id) = create_minimal_state();
        shared_state.update(state, account_id.clone()).await;

        assert!(shared_state.is_initialized());
        assert!(shared_state.get().is_some());
        assert_eq!(shared_state.account_id().await, Some(account_id));
    }

    #[tokio::test]
    async fn test_shared_state_update_with_flag() {
        let shared_state = SharedState::new();

        let (state, account_id) = create_state_with_flag();
        shared_state.update(state, account_id).await;

        let retrieved_state = shared_state.get().unwrap();
        assert_eq!(retrieved_state.flags.len(), 1);
        assert!(retrieved_state.flags.contains_key("flags/test-flag"));
    }

    #[tokio::test]
    async fn test_shared_state_update_replaces_previous() {
        let shared_state = SharedState::new();

        // First update with minimal state
        let (state1, account_id1) = create_minimal_state();
        shared_state.update(state1, account_id1).await;
        assert_eq!(shared_state.get().unwrap().flags.len(), 0);

        // Second update with state containing a flag
        let (state2, account_id2) = create_state_with_flag();
        shared_state.update(state2, account_id2).await;
        assert_eq!(shared_state.get().unwrap().flags.len(), 1);
    }

    #[tokio::test]
    async fn test_shared_state_account_id() {
        let shared_state = SharedState::new();

        // Initially no account ID
        assert_eq!(shared_state.account_id().await, None);

        // After update, account ID is set
        let (state, _) = create_minimal_state();
        shared_state
            .update(state, "custom-account-id".to_string())
            .await;
        assert_eq!(
            shared_state.account_id().await,
            Some("custom-account-id".to_string())
        );
    }

    #[test]
    fn test_decrypt_encrypted_state() {
        let encrypted = std::fs::read(data_dir().join("resolver_state_encrypted.pb")).unwrap();
        let hex_key =
            std::fs::read_to_string(data_dir().join("encryption_key_test.hex")).unwrap();
        let key = Some(hex::decode(hex_key.trim()).unwrap());

        let decrypted = StateFetcher::decrypt(&encrypted, &key).unwrap();
        let request = SetResolverStateRequest::decode(decrypted.as_slice()).unwrap();
        assert_eq!(request.account_id, "confidence-test");

        let state_pb = ResolverStatePb::decode(request.state).unwrap();
        let state = ResolverState::from_proto(state_pb, &request.account_id, None).unwrap();
        assert!(!state.flags.is_empty());
    }

    #[test]
    fn test_decrypt_rejects_wrong_key() {
        let encrypted = std::fs::read(data_dir().join("resolver_state_encrypted.pb")).unwrap();
        let mut wrong_key = hex::decode(
            std::fs::read_to_string(data_dir().join("encryption_key_test.hex"))
                .unwrap()
                .trim(),
        )
        .unwrap();
        wrong_key[0] ^= 0xff;
        let result = StateFetcher::decrypt(&encrypted, &Some(wrong_key));
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_rejects_missing_key() {
        let encrypted = std::fs::read(data_dir().join("resolver_state_encrypted.pb")).unwrap();

        let result = StateFetcher::decrypt(&encrypted, &None);
        assert!(result.is_err());
    }
}
