//! State management for fetching and updating resolver state from CDN.

use std::sync::Arc;

use arc_swap::ArcSwapOption;
use bytes::Bytes;
use prost::Message;
use reqwest::Client;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use confidence_resolver::proto::confidence::flags::admin::v1::ResolverState as ResolverStatePb;
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
    client: Client,
    client_secret: String,
    cdn_url: String,
    etag: RwLock<Option<String>>,
}

impl StateFetcher {
    /// Create a new state fetcher for the given client secret.
    pub fn new(client_secret: String) -> Result<Self> {
        let hash = Self::hash_client_secret(&client_secret);
        let cdn_url = format!("{}/{}", CDN_BASE_URL, hash);

        Ok(Self {
            client: Client::builder()
                .build()
                .map_err(|e| Error::Configuration(e.to_string()))?,
            client_secret,
            cdn_url,
            etag: RwLock::new(None),
        })
    }

    /// Create a new state fetcher with a custom HTTP client.
    pub fn with_client(client_secret: String, client: Client) -> Self {
        let hash = Self::hash_client_secret(&client_secret);
        let cdn_url = format!("{}/{}", CDN_BASE_URL, hash);

        Self {
            client,
            client_secret,
            cdn_url,
            etag: RwLock::new(None),
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

        let response = request.send().await?;

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

        // Parse response body
        let bytes = response.bytes().await?;
        let request = SetResolverStateRequest::decode(bytes).map_err(|e| {
            Error::StateParse(format!("Failed to decode SetResolverStateRequest: {}", e))
        })?;

        // Parse the inner ResolverState
        let state_pb = ResolverStatePb::decode(request.state)
            .map_err(|e| Error::StateParse(format!("Failed to decode ResolverState: {}", e)))?;

        // Convert to ResolverState
        let state = ResolverState::from_proto(state_pb, &request.account_id)
            .map_err(|e| Error::StateParse(format!("Failed to create ResolverState: {:?}", e)))?;

        Ok(Some((state, request.account_id)))
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

    #[test]
    fn test_hash_client_secret() {
        let hash = StateFetcher::hash_client_secret("test-secret");
        // SHA-256 produces 64 hex characters
        assert_eq!(hash.len(), 64);
    }
}
