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

/// Shorter than [`CALL_TIMEOUT`]. A connect that never completes has to fail
/// on its own: the call timeout replaces that failure with a plain timeout,
/// which is how a refused connect was reported as lost on Windows.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// One metadata or token document. A larger body is dropped, not stored.
const BODY_LIMIT: usize = 1_048_576;

pub struct HttpsOAuth {
    client: reqwest::Client,
    body_limit: usize,
}

impl HttpsOAuth {
    pub fn new() -> Option<Self> {
        Self::build(CONNECT_TIMEOUT, CALL_TIMEOUT, BODY_LIMIT)
    }

    fn build(connect: Duration, total: Duration, body_limit: usize) -> Option<Self> {
        install_tls_backend();
        reqwest::Client::builder()
            .connect_timeout(connect)
            .timeout(total)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()
            .map(|client| Self { client, body_limit })
    }

    /// A short total timeout and a small body cap, for the loopback tests.
    /// The connect timeout stays shorter so a refused connect is not rewritten
    /// as the call timeout.
    #[cfg(test)]
    fn bounded(timeout: Duration, body_limit: usize) -> Self {
        let connect = (timeout / 4).max(Duration::from_millis(50));
        Self::build(connect, timeout, body_limit).expect("oauth client")
    }
}

#[async_trait]
impl OAuthHttp for HttpsOAuth {
    async fn get(&self, url: &str) -> Result<OAuthResponse, OAuthCallFailure> {
        self.call(reqwest::Method::GET, url, None, None).await
    }

    async fn post_form(&self, url: &str, body: &str) -> Result<OAuthResponse, OAuthCallFailure> {
        self.call(
            reqwest::Method::POST,
            url,
            Some("application/x-www-form-urlencoded"),
            Some(body),
        )
        .await
    }

    async fn post_json(&self, url: &str, body: &str) -> Result<OAuthResponse, OAuthCallFailure> {
        self.call(
            reqwest::Method::POST,
            url,
            Some("application/json"),
            Some(body),
        )
        .await
    }
}

impl HttpsOAuth {
    async fn call(
        &self,
        method: reqwest::Method,
        url: &str,
        content_type: Option<&str>,
        body: Option<&str>,
    ) -> Result<OAuthResponse, OAuthCallFailure> {
        if !allowed(url) {
            return Err(OAuthCallFailure::NotSent);
        }
        let mut request = self.client.request(method, url);
        if let Some(content_type) = content_type {
            request = request.header(reqwest::header::CONTENT_TYPE, content_type);
        }
        if let Some(body) = body {
            request = request.body(body.to_owned());
        }
        let mut response = match request.send().await {
            Ok(response) => response,
            Err(error) => return Err(classify_send(&error)),
        };
        let status = response.status().as_u16();
        let www_authenticate = response
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let mut collected = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if collected.len().saturating_add(chunk.len()) > self.body_limit {
                        return Err(OAuthCallFailure::Lost);
                    }
                    collected.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(_) => return Err(OAuthCallFailure::Lost),
            }
        }
        let body = String::from_utf8(collected).map_err(|_| OAuthCallFailure::Lost)?;
        Ok(OAuthResponse {
            status,
            body,
            www_authenticate,
        })
    }
}

/// `NotSent` is only a failure that cannot have left the machine: building
/// the request, connecting, or a refused connect Windows did not mark
/// `is_connect`. `is_request` does not say the bytes stayed local, so every
/// other send error, including a timeout, is [`OAuthCallFailure::Lost`].
fn classify_send(error: &reqwest::Error) -> OAuthCallFailure {
    if error.is_builder() || error.is_connect() || connection_refused(error) {
        OAuthCallFailure::NotSent
    } else {
        OAuthCallFailure::Lost
    }
}

fn connection_refused(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(item) = current {
        if let Some(io) = item.downcast_ref::<std::io::Error>() {
            if io.kind() == std::io::ErrorKind::ConnectionRefused
                || io.raw_os_error() == Some(10061)
            {
                return true;
            }
        }
        current = item.source();
    }
    false
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
            // `host_str` keeps the brackets `Url` parsed from `[::1]`.
            Some("localhost") | Some("127.0.0.1") | Some("::1") | Some("[::1]")
        )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use crate::mcp_authorization::application::OAuthCallFailure;

    #[test]
    fn http_ipv6_loopback_is_allowed_and_other_http_is_not() {
        assert!(allowed("http://[::1]/mcp"));
        assert!(allowed("http://[::1]:9/mcp"));
        assert!(allowed("http://localhost/mcp"));
        assert!(allowed("http://127.0.0.1/mcp"));
        assert!(allowed("https://mcp.example/mcp"));
        assert!(!allowed("http://192.0.2.1/mcp"));
        assert!(!allowed("http://[2001:db8::1]/mcp"));
    }

    #[test]
    fn a_refused_io_error_buried_in_the_source_chain_is_refused() {
        #[derive(Debug)]
        struct Wrap(std::io::Error);
        impl std::fmt::Display for Wrap {
            fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                output.write_str("wrapped")
            }
        }
        impl std::error::Error for Wrap {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }
        let wrapped = Wrap(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "refused",
        ));
        assert!(connection_refused(&wrapped));
    }

    #[test]
    fn windows_connection_refused_code_is_refused_even_when_the_kind_is_not() {
        let refused = std::io::Error::from_raw_os_error(10061);
        assert!(connection_refused(&refused));
        let timeout = std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out");
        assert!(!connection_refused(&timeout));
    }

    async fn read_headers(stream: &mut tokio::net::TcpStream) {
        let mut buf = Vec::new();
        let mut tmp = [0_u8; 1024];
        loop {
            let read = stream.read(&mut tmp).await.unwrap();
            if read == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..read]);
            if buf.windows(4).any(|mark| mark == b"\r\n\r\n") {
                break;
            }
        }
    }

    #[tokio::test]
    async fn a_stalled_peer_returns_within_the_call_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_headers(&mut stream).await;
            std::future::pending::<()>().await;
        });
        let client = HttpsOAuth::bounded(Duration::from_millis(200), 64);
        let started = std::time::Instant::now();
        let result = client.get(&format!("http://127.0.0.1:{port}/probe")).await;
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(result, Err(OAuthCallFailure::Lost));
    }

    #[tokio::test]
    async fn a_refused_connection_is_not_sent() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let client = HttpsOAuth::bounded(Duration::from_millis(500), 64);
        let result = client.get(&format!("http://127.0.0.1:{port}/probe")).await;
        assert_eq!(result, Err(OAuthCallFailure::NotSent));
    }

    #[tokio::test]
    async fn a_body_past_the_limit_is_not_kept() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_headers(&mut stream).await;
            let body = "x".repeat(200);
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        });
        let client = HttpsOAuth::bounded(Duration::from_secs(2), 32);
        let result = client.get(&format!("http://127.0.0.1:{port}/")).await;
        assert_eq!(result, Err(OAuthCallFailure::Lost));
    }
}
