//! HTTPS calls to an authorization server. HTTP is not sent. A redirect is
//! the status, not a followed `Location`. Loopback HTTP is not an
//! authorization-server endpoint; the resource probe is the one caller that
//! may use a loopback MCP URL, and it still sends no bearer.
use std::time::Duration;

use async_trait::async_trait;

use crate::agent_install::infrastructure::https_archives::install_tls_backend;
use crate::mcp_authorization::application::{
    https_url, OAuthCallFailure, OAuthHttp, OAuthResponse,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

pub struct HttpsOAuth {
    client: reqwest::Client,
}

impl HttpsOAuth {
    pub fn new() -> Option<Self> {
        install_tls_backend();
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()
            .map(|client| Self { client })
    }
}

#[async_trait]
impl OAuthHttp for HttpsOAuth {
    async fn get(&self, url: &str) -> Result<OAuthResponse, OAuthCallFailure> {
        self.call(reqwest::Method::GET, url, None).await
    }

    async fn post_form(&self, url: &str, body: &str) -> Result<OAuthResponse, OAuthCallFailure> {
        self.call(reqwest::Method::POST, url, Some(body)).await
    }
}

impl HttpsOAuth {
    async fn call(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<&str>,
    ) -> Result<OAuthResponse, OAuthCallFailure> {
        if !allowed(url) {
            return Err(OAuthCallFailure::NotSent);
        }
        let mut request = self.client.request(method, url);
        if body.is_some() {
            request = request.header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            );
            request = request.body(body.unwrap_or("").to_owned());
        }
        match request.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let www_authenticate = response
                    .headers()
                    .get(reqwest::header::WWW_AUTHENTICATE)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                let body = response.text().await.map_err(|_| OAuthCallFailure::Lost)?;
                Ok(OAuthResponse {
                    status,
                    body,
                    www_authenticate,
                })
            }
            Err(error) if error.is_connect() || error.is_builder() || error.is_request() => {
                Err(OAuthCallFailure::NotSent)
            }
            Err(_) => Err(OAuthCallFailure::Lost),
        }
    }
}

/// Authorization-server URLs are HTTPS. A resource probe may be loopback HTTP.
fn allowed(url: &str) -> bool {
    if https_url(url) {
        return true;
    }
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    parsed.scheme() == "http"
        && matches!(
            parsed.host_str(),
            Some("localhost") | Some("127.0.0.1") | Some("::1")
        )
}
