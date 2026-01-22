use http::{Extensions, HeaderValue};
use reqwest::{Request, Response, Url};
use reqwest_middleware::{Middleware, Next, Result};

use crate::Error;

pub struct GatewayMiddleware {
    gateway: Url,
}

impl GatewayMiddleware {
    pub fn from_str(url: &str) -> crate::Result<GatewayMiddleware> {
        let url = Url::parse(url)
            .map_err(|e| Error::Configuration(format!("invalid gateway URL: {e}")))?;
        Ok(GatewayMiddleware { gateway: url })
    }
}

/// Error type for gateway middleware failures.
#[derive(Debug)]
struct GatewayError(String);

impl std::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "gateway error: {}", self.0)
    }
}

impl std::error::Error for GatewayError {}

#[async_trait::async_trait]
impl Middleware for GatewayMiddleware {
    async fn handle(
        &self,
        mut req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        let url = req.url_mut();

        // Capture original host for X-Forwarded-Host header
        let orig_host = url
            .host_str()
            .ok_or_else(|| {
                reqwest_middleware::Error::Middleware(
                    GatewayError("request URL has no host".into()).into(),
                )
            })?
            .to_string();

        // Rewrite URL to point to gateway
        url.set_scheme(self.gateway.scheme()).map_err(|()| {
            reqwest_middleware::Error::Middleware(
                GatewayError(format!(
                    "cannot set scheme '{}' on request URL",
                    self.gateway.scheme()
                ))
                .into(),
            )
        })?;

        url.set_host(self.gateway.host_str()).map_err(|e| {
            reqwest_middleware::Error::Middleware(
                GatewayError(format!("cannot set gateway host: {e}")).into(),
            )
        })?;

        url.set_port(self.gateway.port()).map_err(|()| {
            reqwest_middleware::Error::Middleware(
                GatewayError(format!(
                    "cannot set port {:?} on request URL",
                    self.gateway.port()
                ))
                .into(),
            )
        })?;

        // Add X-Forwarded-Host header
        let header_value = HeaderValue::from_str(&orig_host).map_err(|e| {
            reqwest_middleware::Error::Middleware(
                GatewayError(format!("invalid host for header: {e}")).into(),
            )
        })?;
        req.headers_mut().insert("x-forwarded-host", header_value);

        next.run(req, extensions).await
    }
}
