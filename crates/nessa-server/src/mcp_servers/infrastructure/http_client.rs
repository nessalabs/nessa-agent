//! The gateway's HTTP calls for a remote MCP server.
//!
//! ```text
//! HttpSession ──exchange──▶ ReqwestExchange ──▶ peer
//!                         ◀── status, headers, body stream
//! ```
//!
//! Redirects are not followed. A 3xx is a status the transport reads, so a
//! credential is never sent on to a URL the SDK did not accept. Only the wait
//! for response headers is bounded. The client has no response timeout, so an
//! event-stream body stays open after those headers arrive. TLS uses the provider
//! [`install_tls_backend`](crate::agent_install::infrastructure::https_archives::install_tls_backend)
//! names.
use crate::agent_install::infrastructure::https_archives::install_tls_backend;
use async_trait::async_trait;
use futures_util::StreamExt;
use nessa_sdk::infrastructure::mcp::{
    HttpBody, HttpChunks, HttpExchange, HttpFailure, HttpMethod, HttpRequest, HttpResponse,
};
use std::{pin::Pin, time::Duration};

/// How long a connect may take before the exchange is unreachable. A request
/// already connected is bounded by the SDK, not here.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long `.send()` may wait for response headers. The body is read after
/// that, so this does not close an event stream.
const RESPONSE_HEADERS_TIMEOUT: Duration = Duration::from_secs(30);

/// Remote MCP calls for one gateway process.
pub struct ReqwestExchange {
    client: reqwest::Client,
    header_timeout: Duration,
}

impl ReqwestExchange {
    /// A client that does not follow redirects.
    ///
    /// `None` when the client cannot be built. Remote openings then fail as
    /// unreachable rather than as a successful call.
    pub fn new() -> Option<Self> {
        Self::bounded(RESPONSE_HEADERS_TIMEOUT)
    }

    fn bounded(header_timeout: Duration) -> Option<Self> {
        install_tls_backend();
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()
            .map(|client| Self {
                client,
                header_timeout,
            })
    }
}

struct Chunks {
    stream: Pin<Box<dyn futures_util::Stream<Item = Result<Vec<u8>, HttpFailure>> + Send>>,
}

#[async_trait]
impl HttpChunks for Chunks {
    async fn next(&mut self) -> Result<Option<Vec<u8>>, HttpFailure> {
        match self.stream.next().await {
            Some(Ok(bytes)) => Ok(Some(bytes)),
            Some(Err(failure)) => Err(failure),
            None => Ok(None),
        }
    }
}

#[async_trait]
impl HttpExchange for ReqwestExchange {
    async fn exchange(&self, request: HttpRequest) -> Result<HttpResponse, HttpFailure> {
        let method = match request.method {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Post => reqwest::Method::POST,
            HttpMethod::Delete => reqwest::Method::DELETE,
        };
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in &request.headers {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| HttpFailure::Unreachable)?;
            let value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| HttpFailure::Unreachable)?;
            headers.append(name, value);
        }
        let response = match tokio::time::timeout(
            self.header_timeout,
            self.client
                .request(method, &request.url)
                .headers(headers)
                .body(request.body)
                .send(),
        )
        .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(_)) | Err(_) => return Err(HttpFailure::Unreachable),
        };
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_owned(), value.to_owned()))
            })
            .collect();
        let stream = response.bytes_stream().map(|chunk| {
            chunk
                .map(|bytes| bytes.to_vec())
                .map_err(|_| HttpFailure::Unreachable)
        });
        Ok(HttpResponse {
            status,
            headers,
            body: HttpBody::Stream(Box::new(Chunks {
                stream: Box::pin(stream),
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    async fn serve(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            socket.write_all(body.as_bytes()).await.unwrap();
        });
        format!("http://127.0.0.1:{port}/mcp")
    }

    #[tokio::test]
    async fn a_redirect_is_a_status_and_is_not_followed() {
        let url = serve(
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/other\r\nContent-Length: 0\r\n\r\n",
        )
        .await;
        let exchange = ReqwestExchange::new().expect("client");
        let response = exchange
            .exchange(HttpRequest {
                method: HttpMethod::Post,
                url,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            })
            .await
            .expect("status");
        assert_eq!(response.status, 302);
        assert_eq!(
            response.header("location"),
            Some("http://127.0.0.1:1/other")
        );
    }

    #[tokio::test]
    async fn a_json_body_is_delivered_whole() {
        let url = serve(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
        )
        .await;
        let exchange = ReqwestExchange::new().expect("client");
        let response = exchange
            .exchange(HttpRequest {
                method: HttpMethod::Get,
                url,
                headers: Vec::new(),
                body: Vec::new(),
            })
            .await
            .expect("status");
        assert_eq!(response.status, 200);
        assert_eq!(response.bytes(16).await.unwrap(), b"{}");
    }

    #[tokio::test]
    async fn a_peer_that_never_sends_headers_is_unreachable() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 1024];
            let _ = socket.read(&mut buf).await;
            std::future::pending::<()>().await;
        });
        let exchange = ReqwestExchange::bounded(Duration::from_millis(200)).expect("client");
        let started = std::time::Instant::now();
        let result = exchange
            .exchange(HttpRequest {
                method: HttpMethod::Post,
                url: format!("http://127.0.0.1:{port}/mcp"),
                headers: Vec::new(),
                body: b"{}".to_vec(),
            })
            .await;
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(matches!(result, Err(HttpFailure::Unreachable)));
    }

    #[tokio::test]
    async fn a_body_after_the_header_timeout_is_still_read() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 1024];
            let _ = socket.read(&mut buf).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n")
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(400)).await;
            socket.write_all(b"{}").await.unwrap();
        });
        let exchange = ReqwestExchange::bounded(Duration::from_millis(150)).expect("client");
        let response = exchange
            .exchange(HttpRequest {
                method: HttpMethod::Post,
                url: format!("http://127.0.0.1:{port}/mcp"),
                headers: Vec::new(),
                body: b"{}".to_vec(),
            })
            .await
            .expect("headers");
        assert_eq!(response.bytes(16).await.unwrap(), b"{}");
    }
}
