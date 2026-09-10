use crate::{
    backend::{read_limited, Backend},
    config::{normalize_account, Config},
    service::ResolverService,
};
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use confidence_resolver::{
    proto::confidence::flags::admin::v1::ResolverState as StateProto, ResolverState,
};
use prost::Message;
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub struct StateFetcher {
    backend: Arc<Backend>,
    configured_url: Option<reqwest::Url>,
    account_id: Option<String>,
    key: Option<Vec<u8>>,
    etag: Option<reqwest::header::HeaderValue>,
    signed: Option<(reqwest::Url, Instant)>,
}
impl StateFetcher {
    pub fn new(config: &Config, backend: Arc<Backend>) -> Self {
        Self {
            backend,
            configured_url: config.state_url.clone(),
            account_id: config.account_id.clone(),
            key: config.encryption_key.clone(),
            etag: None,
            signed: None,
        }
    }
    async fn url(&mut self) -> Result<reqwest::Url, String> {
        if let Some(url) = &self.configured_url {
            return Ok(url.clone());
        }
        if let Some((url, refresh_at)) = &self.signed {
            if *refresh_at > Instant::now() {
                return Ok(url.clone());
            }
        }
        let response = self.backend.discover_state().await?;
        let id = normalize_account(&response.account)?;
        if self
            .account_id
            .as_ref()
            .is_some_and(|expected| expected != &id)
        {
            return Err("State account mismatch".into());
        }
        let url = crate::config::http_url(&response.signed_uri, "signed state URL")?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "Invalid system time")?
            .as_secs();
        let ttl = response
            .expire_time
            .map(|t| (t.seconds.max(0) as u64).saturating_sub(now))
            .unwrap_or(60);
        self.signed = Some((
            url.clone(),
            Instant::now() + Duration::from_secs((ttl / 2).max(1)),
        ));
        self.account_id = Some(id);
        Ok(url)
    }

    pub async fn reload(&mut self, service: &ResolverService) -> Result<bool, String> {
        let url = self.url().await?;
        let mut request = self.backend.http.get(url);
        if let Some(etag) = &self.etag {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        let response = request.send().await.map_err(|_| "State download failed")?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return if service.ready() {
                Ok(false)
            } else {
                Err("State returned 304 before initial load".into())
            };
        }
        if !response.status().is_success() {
            self.signed = None;
            return Err(format!(
                "State download HTTP status {}",
                response.status().as_u16()
            ));
        }
        let etag = response.headers().get(reqwest::header::ETAG).cloned();
        let bytes = read_limited(response, 256 * 1024 * 1024).await?;
        let key = self.key.clone();
        let expected = self.account_id.clone();
        // Protobuf parsing must not block an async runtime worker.
        let (state, account, log_key, fields) = tokio::task::spawn_blocking(move || {
            decode_state(bytes, key.as_deref(), expected.as_deref())
        })
        .await
        .map_err(|_| "State loader task failed")??;
        self.backend.validate_account(&account).await?;
        service.replace_state(state, log_key, fields);
        self.account_id = Some(account);
        self.etag = etag;
        Ok(true)
    }
}

fn decode_state(
    bytes: Vec<u8>,
    key: Option<&[u8]>,
    expected: Option<&str>,
) -> Result<
    (
        ResolverState,
        String,
        Option<String>,
        Vec<crate::sampling::FieldOverride>,
    ),
    String,
> {
    let (state, account, log_key) = if let Some(key) = key {
        if bytes.len() < 28 {
            return Err("Encrypted state is too short".into());
        }
        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| "Invalid encryption key")?;
        let plaintext = cipher
            .decrypt(Nonce::from_slice(&bytes[..12]), &bytes[12..])
            .map_err(|_| "State decryption failed")?;
        let envelope =
            StateEnvelope::decode(plaintext.as_slice()).map_err(|_| "Invalid state envelope")?;
        if envelope
            .log_destinations
            .iter()
            .any(|d| !matches!(d, 0 | 1))
        {
            return Err("This image supports the Confidence edge log destination only".into());
        }
        let state = envelope.state.ok_or("State envelope has no state")?;
        (
            state,
            normalize_account(&envelope.account)?,
            Some(envelope.client_write_flag_logs_api_key).filter(|k| !k.is_empty()),
        )
    } else {
        (
            bytes,
            expected
                .ok_or("Account is required for plaintext state")?
                .to_string(),
            None,
        )
    };
    if expected.is_some_and(|expected| expected != account) {
        return Err("State account mismatch".into());
    }
    // Correct parent ownership before using the core loader's prefix-based matcher.
    // This also rejects duplicate/ambiguous keys instead of accepting ordering-dependent ownership.
    let fields = crate::sampling::StateFields::decode(state.as_slice())
        .map_err(|_| "Invalid state metadata")?
        .fields;
    let state = StateProto::decode(state.as_slice()).map_err(|_| "Invalid resolver state")?;
    let clients: std::collections::HashSet<_> =
        state.clients.iter().map(|c| c.name.as_str()).collect();
    if clients.len() != state.clients.len() {
        return Err("Duplicate client name in state".into());
    }
    let mut owners = std::collections::HashMap::new();
    for credential in &state.client_credentials {
        let Some((parent, _)) = credential
            .name
            .rsplit_once('/')
            .and_then(|(parent, id)| (!id.is_empty()).then_some(parent))
            .and_then(|parent| parent.rsplit_once('/'))
        else {
            return Err("Invalid credential resource name".into());
        };
        if !clients.contains(parent) {
            return Err("Credential refers to unknown client".into());
        }
        if let Some(confidence_resolver::proto::confidence::iam::v1::client_credential::Credential::ClientSecret(secret)) = &credential.credential {
            if secret.secret.is_empty() || owners.insert(secret.secret.clone(), (parent.to_string(), credential.name.clone(), credential.environments.clone())).is_some() {
                return Err("Empty or duplicate client secret in state".into());
            }
        }
    }
    let mut parsed = ResolverState::from_proto(state, &account, None)
        .map_err(|_| "Could not load resolver state")?;
    for (secret, (client, credential, environments)) in owners {
        if let Some(entry) = parsed.secrets.get_mut(&secret) {
            entry.client_name = client;
            entry.client_credential_name = credential;
            entry.environments = environments;
        }
    }
    Ok((parsed, account, log_key, fields))
}

/// Wire-compatible projection of the full-account encrypted envelope from flags-admin.
#[derive(Clone, PartialEq, Message)]
struct StateEnvelope {
    #[prost(bytes = "vec", optional, tag = "1")]
    state: Option<Vec<u8>>,
    #[prost(string, tag = "2")]
    account: String,
    #[prost(string, tag = "3")]
    client_write_flag_logs_api_key: String,
    #[prost(int32, repeated, tag = "4")]
    log_destinations: Vec<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encrypted_state_validates_account_and_integrity() {
        let key = [7u8; 32];
        let account: StateProto =
            serde_json::from_str(include_str!("../tests/fixtures/account.json")).unwrap();
        let envelope = StateEnvelope {
            state: Some(account.encode_to_vec()),
            account: "accounts/test".into(),
            client_write_flag_logs_api_key: "log-key".into(),
            log_destinations: vec![1],
        };
        let nonce = [3u8; 12];
        let mut payload = nonce.to_vec();
        payload.extend(
            Aes256Gcm::new_from_slice(&key)
                .unwrap()
                .encrypt(
                    Nonce::from_slice(&nonce),
                    envelope.encode_to_vec().as_slice(),
                )
                .unwrap(),
        );
        let (state, account, log_key, _) =
            decode_state(payload.clone(), Some(&key), Some("test")).unwrap();
        assert_eq!(account, "test");
        assert_eq!(log_key.as_deref(), Some("log-key"));
        assert_eq!(state.secrets["test-ab"].client_name, "clients/ab");
        assert!(decode_state(payload.clone(), Some(&key), Some("other")).is_err());
        *payload.last_mut().unwrap() ^= 1;
        assert!(decode_state(payload, Some(&key), None).is_err());
    }
}
