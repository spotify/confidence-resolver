use std::{net::SocketAddr, time::Duration};

pub struct Config {
    pub http_address: SocketAddr,
    pub grpc_address: SocketAddr,
    pub state_url: Option<reqwest::Url>,
    pub account_id: Option<String>,
    pub encryption_key: Option<Vec<u8>>,
    pub credentials: Option<(String, String)>,
    pub grpc_target: String,
    pub api_url: reqwest::Url,
    pub poll_interval: Duration,
    pub log_interval: Duration,
    pub metadata_interval: Duration,
    pub send_apply_logs: bool,
    pub log_capacity: usize,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let port = |name, default: u16| -> Result<SocketAddr, String> {
            let value = get(name).unwrap_or_else(|| default.to_string());
            let port = value
                .parse::<u16>()
                .map_err(|_| format!("Invalid {name}"))?;
            Ok(([0, 0, 0, 0], port).into())
        };
        let interval = |name, default: u64| -> Result<Duration, String> {
            get(name)
                .unwrap_or_else(|| default.to_string())
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0)
                .map(Duration::from_secs)
                .ok_or_else(|| format!("{name} must be positive"))
        };
        let boolean = |name, default| -> Result<bool, String> {
            match get(name).as_deref() {
                None => Ok(default),
                Some("true") => Ok(true),
                Some("false") => Ok(false),
                _ => Err(format!("{name} must be true or false")),
            }
        };
        let credentials = match (get("CONFIDENCE_CLIENT_ID"), get("CONFIDENCE_CLIENT_SECRET")) {
            (Some(id), Some(secret)) if !id.is_empty() && !secret.is_empty() => Some((id, secret)),
            (None, None) => None,
            _ => return Err("Set both CONFIDENCE_CLIENT_ID and CONFIDENCE_CLIENT_SECRET".into()),
        };
        let state_url = get("CONFIDENCE_RESOLVER_STATE_URL")
            .map(|value| http_url(&value, "CONFIDENCE_RESOLVER_STATE_URL"))
            .transpose()?;
        let account_id = get("CONFIDENCE_ACCOUNT")
            .map(|value| normalize_account(&value))
            .transpose()?;
        let encryption_key = get("CONFIDENCE_RESOLVER_STATE_ENCRYPTION_KEY")
            .map(|value| {
                hex::decode(value)
                    .ok()
                    .filter(|bytes| bytes.len() == 32)
                    .ok_or_else(|| {
                        "CONFIDENCE_RESOLVER_STATE_ENCRYPTION_KEY must be 32 bytes in hex"
                            .to_string()
                    })
            })
            .transpose()?;
        if state_url.is_none() && credentials.is_none() {
            return Err("Set CONFIDENCE_RESOLVER_STATE_URL or API-client credentials".into());
        }
        if state_url.is_none() && encryption_key.is_some() {
            return Err("Encrypted state requires CONFIDENCE_RESOLVER_STATE_URL".into());
        }
        if state_url.is_some() && encryption_key.is_none() && account_id.is_none() {
            return Err("Plaintext direct state requires CONFIDENCE_ACCOUNT".into());
        }
        let domain = get("CONFIDENCE_DOMAIN").unwrap_or_else(|| "edge-grpc.spotify.com".into());
        let scheme = if boolean("CONFIDENCE_GRPC_PLAINTEXT", false)? {
            "http"
        } else {
            "https"
        };
        let grpc_target = format!("{scheme}://{domain}");
        let api_url = http_url(
            &get("CONFIDENCE_RESOLVER_API_URL")
                .unwrap_or_else(|| "https://resolver.confidence.dev".into()),
            "CONFIDENCE_RESOLVER_API_URL",
        )?;
        let log_capacity = get("CONFIDENCE_ASSIGN_LOG_CAPACITY")
            .unwrap_or_else(|| "33554432".into())
            .parse::<usize>()
            .ok()
            .filter(|size| *size > 0)
            .ok_or("CONFIDENCE_ASSIGN_LOG_CAPACITY must be positive bytes")?;
        Ok(Self {
            http_address: port("CONFIDENCE_RESOLVER_HTTP_PORT", 8090)?,
            grpc_address: port("CONFIDENCE_RESOLVER_GRPC_PORT", 5990)?,
            state_url,
            account_id,
            encryption_key,
            credentials,
            grpc_target,
            api_url,
            poll_interval: interval("CONFIDENCE_RESOLVER_POLL_INTERVAL_SECONDS", 30)?,
            log_interval: interval("CONFIDENCE_ASSIGN_LOG_INTERVAL_SECONDS", 10)?,
            metadata_interval: interval("CONFIDENCE_METADATA_REPORT_INTERVAL_SECONDS", 300)?,
            send_apply_logs: boolean("CONFIDENCE_SEND_APPLY_LOGS", true)?,
            log_capacity,
        })
    }
}

pub fn http_url(value: &str, name: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(value).map_err(|_| format!("Invalid {name}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(format!("{name} must be an HTTP(S) URL"));
    }
    Ok(url)
}

pub fn normalize_account(account: &str) -> Result<String, String> {
    let id = account.strip_prefix("accounts/").unwrap_or(account);
    if id.is_empty() || id.contains('/') || id.chars().any(char::is_whitespace) {
        return Err("Account must be an ID or accounts/<id>".into());
    }
    Ok(id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_state_source_and_credentials() {
        let values = |name: &str| match name {
            "CONFIDENCE_ACCOUNT" => Some("accounts/test".into()),
            "CONFIDENCE_RESOLVER_STATE_URL" => Some("http://localhost/state".into()),
            _ => None,
        };
        assert_eq!(
            Config::from_lookup(values).unwrap().account_id.as_deref(),
            Some("test")
        );
        assert!(Config::from_lookup(|_| None).is_err());
        assert!(Config::from_lookup(|name| {
            if name == "CONFIDENCE_RESOLVER_POLL_INTERVAL_SECONDS" {
                Some("0".into())
            } else {
                values(name)
            }
        })
        .is_err());
        assert!(normalize_account("accounts/accounts/test").is_err());
    }
}
