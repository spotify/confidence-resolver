//! Outbound Confidence calls. Errors intentionally omit URLs, tokens and bodies.
use base64::Engine;
use prost::Message;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tonic::{
    transport::{Channel, ClientTlsConfig, Endpoint},
    Request,
};

use crate::{api, config::Config, remote_proto as remote};
use confidence_resolver::proto::{
    confidence::flags::admin::v1::ResolverStateUriResponse, google::Timestamp,
};

pub struct Backend {
    pub http: reqwest::Client,
    api_url: reqwest::Url,
    credentials: Option<(String, String)>,
    channel: Option<Channel>,
    token: Mutex<Option<Token>>,
}
struct Token {
    value: String,
    refresh_at: Instant,
    account: Option<String>,
}

impl Backend {
    pub fn new(config: &Config) -> Result<Arc<Self>, String> {
        let channel = if config.credentials.is_some() {
            let mut endpoint = Endpoint::from_shared(config.grpc_target.clone())
                .map_err(|_| "Invalid CONFIDENCE_DOMAIN")?
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(10));
            if config.grpc_target.starts_with("https:") {
                endpoint = endpoint
                    .tls_config(ClientTlsConfig::new().with_webpki_roots())
                    .map_err(|_| "Could not configure gRPC TLS")?;
            }
            Some(endpoint.connect_lazy())
        } else {
            None
        };
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|_| "Could not create HTTP client")?;
        Ok(Arc::new(Self {
            http,
            api_url: config.api_url.clone(),
            credentials: config.credentials.clone(),
            channel,
            token: Mutex::new(None),
        }))
    }

    async fn rpc<Q, R>(
        &self,
        path: &'static str,
        message: Q,
        bearer: Option<&str>,
    ) -> Result<R, String>
    where
        Q: Message + Default + Send + Sync + 'static,
        R: Message + Default + Send + Sync + 'static,
    {
        let channel = self
            .channel
            .clone()
            .ok_or("API-client credentials are required")?;
        let mut client = tonic::client::Grpc::new(channel);
        client
            .ready()
            .await
            .map_err(|_| "Confidence gRPC connection unavailable")?;
        let mut request = Request::new(message);
        request.set_timeout(Duration::from_secs(10));
        if let Some(token) = bearer {
            request.metadata_mut().insert(
                "authorization",
                format!("Bearer {token}")
                    .parse()
                    .map_err(|_| "Invalid access token")?,
            );
        }
        let result: tonic::Response<R> = client
            .unary(
                request,
                tonic::codegen::http::uri::PathAndQuery::from_static(path),
                tonic::codec::ProstCodec::default(),
            )
            .await
            .map_err(|error| format!("Confidence RPC failed: {:?}", error.code()))?;
        Ok(result.into_inner())
    }

    async fn access_token(&self) -> Result<(String, Option<String>), String> {
        let mut cached = self.token.lock().await;
        if let Some(token) = cached
            .as_ref()
            .filter(|token| token.refresh_at > Instant::now())
        {
            return Ok((token.value.clone(), token.account.clone()));
        }
        let (id, secret) = self
            .credentials
            .as_ref()
            .ok_or("API-client credentials are required")?;
        let token: AccessToken = self
            .rpc(
                "/confidence.iam.v1.AuthService/RequestAccessToken",
                AccessTokenRequest {
                    grant_type: "client_credentials".into(),
                    client_id: id.clone(),
                    client_secret: secret.clone(),
                },
                None,
            )
            .await?;
        if token.access_token.is_empty() || token.expires_in <= 0 {
            return Err("Invalid access token response".into());
        }
        // The account claim is metadata from the trusted auth service, not inbound authentication.
        let account = token
            .access_token
            .split('.')
            .nth(1)
            .and_then(|part| {
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(part)
                    .ok()
            })
            .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
            .and_then(|claims| {
                claims
                    .get("https://confidence.dev/account_name")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
            });
        let lifetime = (token.expires_in as u64).saturating_mul(4) / 5;
        let result = (token.access_token.clone(), account.clone());
        *cached = Some(Token {
            value: token.access_token,
            account,
            refresh_at: Instant::now() + Duration::from_secs(lifetime.max(1)),
        });
        Ok(result)
    }

    pub async fn discover_state(&self) -> Result<ResolverStateUriResponse, String> {
        let (token, account) = self.access_token().await?;
        let mut response: ResolverStateUriResponse = self
            .rpc(
                "/confidence.flags.resolver.v1.ResolverStateService/ResolverStateUri",
                Empty {},
                Some(&token),
            )
            .await?;
        if response.account.is_empty() {
            response.account = account.unwrap_or_default();
        }
        self.validate_account(&response.account).await?;
        Ok(response)
    }

    pub async fn validate_account(&self, account: &str) -> Result<(), String> {
        if self.credentials.is_some() {
            let (_, authenticated) = self.access_token().await?;
            let authenticated = authenticated.ok_or("Access token has no account claim")?;
            if crate::config::normalize_account(&authenticated)?
                != crate::config::normalize_account(account)?
            {
                return Err("API-client and resolver state accounts differ".into());
            }
        }
        Ok(())
    }

    async fn client_post<Q: Message>(
        &self,
        path: &str,
        secret: &str,
        message: Q,
        timeout: Duration,
    ) -> Result<Vec<u8>, String> {
        let url = self.api_url.join(path).map_err(|_| "Invalid API URL")?;
        let response = self
            .http
            .post(url)
            .header("Content-Type", "application/x-protobuf")
            .header("Authorization", format!("ClientSecret {secret}"))
            .body(message.encode_to_vec())
            .timeout(timeout)
            .send()
            .await
            .map_err(|_| "Confidence HTTP request failed")?;
        if !response.status().is_success() {
            return Err(format!(
                "Confidence HTTP status {}",
                response.status().as_u16()
            ));
        }
        read_limited(response, 4 * 1024 * 1024).await
    }

    pub async fn read_materializations(
        &self,
        secret: &str,
        records: Vec<api::MaterializationRecord>,
    ) -> Result<Vec<api::MaterializationRecord>, String> {
        let ops = records
            .into_iter()
            .map(|r| remote::ReadOp {
                op: Some(if r.rule.is_empty() {
                    remote::read_op::Op::InclusionReadOp(remote::InclusionReadOp {
                        unit: r.unit,
                        materialization: r.materialization,
                    })
                } else {
                    remote::read_op::Op::VariantReadOp(remote::VariantReadOp {
                        unit: r.unit,
                        materialization: r.materialization,
                        rule: r.rule,
                    })
                }),
            })
            .collect();
        let bytes = self
            .client_post(
                "/v1/materialization:readMaterializedOperations",
                secret,
                remote::ReadOperationsRequest { ops },
                Duration::from_millis(500),
            )
            .await?;
        let response = remote::ReadOperationsResult::decode(bytes.as_slice())
            .map_err(|_| "Invalid materialization response")?;
        Ok(response
            .results
            .into_iter()
            .filter_map(|r| match r.result? {
                remote::read_result::Result::VariantResult(v) if !v.variant.is_empty() => {
                    Some(api::MaterializationRecord {
                        unit: v.unit,
                        materialization: v.materialization,
                        rule: v.rule,
                        variant: v.variant,
                    })
                }
                remote::read_result::Result::InclusionResult(v) if v.is_included => {
                    Some(api::MaterializationRecord {
                        unit: v.unit,
                        materialization: v.materialization,
                        ..Default::default()
                    })
                }
                _ => None,
            })
            .collect())
    }

    pub async fn write_materializations(
        &self,
        secret: &str,
        records: Vec<api::MaterializationRecord>,
    ) -> Result<(), String> {
        if records.is_empty() {
            return Ok(());
        }
        let request = remote::WriteOperationsRequest {
            store_variant_op: records
                .into_iter()
                .map(|r| remote::VariantData {
                    unit: r.unit,
                    materialization: r.materialization,
                    rule: r.rule,
                    variant: r.variant,
                })
                .collect(),
        };
        self.client_post(
            "/v1/materialization:writeMaterializedOperations",
            secret,
            request,
            Duration::from_secs(5),
        )
        .await?;
        Ok(())
    }

    pub async fn send_logs(
        &self,
        secret: &str,
        logs: crate::sampling::WireLogs,
    ) -> Result<(), String> {
        if self.credentials.is_some() {
            let (token, _) = self.access_token().await?;
            let _: api::WriteFlagLogsResponse = self
                .rpc(
                    "/confidence.flags.resolver.v1.InternalFlagLoggerService/WriteFlagLogs",
                    logs,
                    Some(&token),
                )
                .await?;
        } else {
            self.client_post(
                "/v1/clientFlagLogs:write",
                secret,
                logs,
                Duration::from_secs(10),
            )
            .await?;
        }
        Ok(())
    }

    pub async fn send_metadata(&self, metadata: Metadata) -> Result<(), String> {
        if self.credentials.is_none() {
            return Ok(());
        }
        let (token, _) = self.access_token().await?;
        let _: Empty = self
            .rpc(
                "/confidence.flags.resolver.v1.InternalFlagLoggerService/WriteSidecarMetadata",
                MetadataRequest {
                    sidecar_metadata: Some(metadata),
                },
                Some(&token),
            )
            .await?;
        Ok(())
    }
}

pub async fn read_limited(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Failed to read response")?
    {
        if chunk.len() > limit.saturating_sub(data.len()) {
            return Err("Response exceeds size limit".into());
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

// Wire projections of the existing auth/metadata APIs; no evaluator schema changes.
#[derive(Clone, PartialEq, Message)]
struct Empty {}
#[derive(Clone, PartialEq, Message)]
struct AccessTokenRequest {
    #[prost(string, tag = "1")]
    grant_type: String,
    #[prost(string, tag = "2")]
    client_id: String,
    #[prost(string, tag = "3")]
    client_secret: String,
}
#[derive(Clone, PartialEq, Message)]
struct AccessToken {
    #[prost(string, tag = "1")]
    access_token: String,
    #[prost(int64, tag = "2")]
    expires_in: i64,
}
#[derive(Clone, PartialEq, Message)]
struct MetadataRequest {
    #[prost(message, optional, tag = "1")]
    sidecar_metadata: Option<Metadata>,
}
#[derive(Clone, PartialEq, Message)]
pub struct Metadata {
    #[prost(string, tag = "1")]
    pub server_id: String,
    #[prost(string, tag = "2")]
    pub sidecar_version: String,
    #[prost(double, tag = "3")]
    pub rps_resolves: f64,
    #[prost(double, tag = "4")]
    pub rps_applies: f64,
    // Zero (unspecified): the legacy enum has no remote-API storage value.
    #[prost(int32, tag = "5")]
    pub sticky_storage_config: i32,
    #[prost(message, optional, tag = "6")]
    pub measurement_time: Option<Timestamp>,
    #[prost(int64, tag = "7")]
    pub unrecognized_targeting_rules_count: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio_stream::wrappers::TcpListenerStream;

    struct Reply<F>(F);
    impl<Q, R, F> tonic::server::UnaryService<Q> for Reply<F>
    where
        Q: Send + 'static,
        R: Send + 'static,
        F: Fn(Q) -> R + Send + 'static,
    {
        type Response = R;
        type Future = std::future::Ready<Result<tonic::Response<R>, tonic::Status>>;
        fn call(&mut self, request: Request<Q>) -> Self::Future {
            std::future::ready(Ok(tonic::Response::new((self.0)(request.into_inner()))))
        }
    }
    async fn reply<Q, R>(
        request: axum::extract::Request,
        f: impl Fn(Q) -> R + Send + 'static,
    ) -> axum::http::Response<tonic::body::Body>
    where
        Q: Message + Default + Send + 'static,
        R: Message + Default + Send + 'static,
    {
        tonic::server::Grpc::new(tonic::codec::ProstCodec::default())
            .unary(Reply(f), request)
            .await
    }

    #[tokio::test]
    async fn bootstrap_authenticates_caches_token_and_checks_account() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let auth_calls = Arc::new(AtomicUsize::new(0));
        let count = auth_calls.clone();
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"https://confidence.dev/account_name":"accounts/test"}"#);
        let token = format!("header.{claims}.signature");
        let expected = format!("Bearer {token}");
        let router = Router::new()
            .route(
                "/confidence.iam.v1.AuthService/RequestAccessToken",
                post(move |request| {
                    let count = count.clone();
                    let token = token.clone();
                    async move {
                        reply(request, move |request: AccessTokenRequest| {
                            count.fetch_add(1, Ordering::Relaxed);
                            assert_eq!(request.grant_type, "client_credentials");
                            assert_eq!(request.client_id, "test-id");
                            assert_eq!(request.client_secret, "test-secret");
                            AccessToken {
                                access_token: token.clone(),
                                expires_in: 3600,
                            }
                        })
                        .await
                    }
                }),
            )
            .route(
                "/confidence.flags.resolver.v1.ResolverStateService/ResolverStateUri",
                post(move |request: axum::extract::Request| {
                    assert_eq!(request.headers()["authorization"], expected);
                    async move {
                        reply(request, |_: Empty| ResolverStateUriResponse {
                            signed_uri: "http://localhost/state".into(),
                            ..Default::default()
                        })
                        .await
                    }
                }),
            );
        let task = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_routes(router.into())
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
                .unwrap();
        });
        let config = Config::from_lookup(|name| match name {
            "CONFIDENCE_CLIENT_ID" => Some("test-id".into()),
            "CONFIDENCE_CLIENT_SECRET" => Some("test-secret".into()),
            "CONFIDENCE_DOMAIN" => Some(address.to_string()),
            "CONFIDENCE_GRPC_PLAINTEXT" => Some("true".into()),
            _ => None,
        })
        .unwrap();
        let backend = Backend::new(&config).unwrap();
        assert_eq!(
            backend.discover_state().await.unwrap().account,
            "accounts/test"
        );
        backend.discover_state().await.unwrap();
        backend.validate_account("test").await.unwrap();
        assert!(backend.validate_account("other").await.is_err());
        assert_eq!(auth_calls.load(Ordering::Relaxed), 1);
        task.abort();
    }
}
