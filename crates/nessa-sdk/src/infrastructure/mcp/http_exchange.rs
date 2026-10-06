//! Nessa-owned HTTP request and response values. The SDK never names an HTTP
//! stack: gateway composition injects an [`HttpExchange`] (`docs/adr/todo/392-remote-mcp-servers.md`).
//!
//! ```text
//! HttpTransport ──exchange(HttpRequest)──▶ HttpExchange (injected)
//!                ◀──HttpResponse──────────┘ status, headers, body bytes or stream
//! ```
//!
//! Arrows are one exchange. Redirects are the adapter's to refuse: this port
//! does not follow them, and a 3xx comes back as a status the transport reads.
#![deny(missing_docs)]

use async_trait::async_trait;
use std::fmt;

/// How an exchange failed before a status was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpFailure {
    /// DNS, TCP, TLS, or the adapter's own deadline passed with no status.
    Unreachable,
}

impl fmt::Display for HttpFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable => f.write_str("the HTTP exchange could not be completed"),
        }
    }
}

/// The method of one MCP or OAuth HTTP call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    /// Read a stream or a metadata document.
    Get,
    /// Send one JSON-RPC message, or a form.
    Post,
    /// Ask the server to forget an upstream session.
    Delete,
}

impl HttpMethod {
    /// The method token on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Delete => "DELETE",
        }
    }
}

/// One HTTP call. Header names are compared case-insensitively by the
/// transport; the adapter sends them as given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequest {
    /// `GET`, `POST`, or `DELETE`.
    pub method: HttpMethod,
    /// Absolute URL the SDK already validated for this call.
    pub url: String,
    /// Name/value pairs. No authorization secret is put here by a caller that
    /// does not already hold one for this exact URL.
    pub headers: Vec<(String, String)>,
    /// The body, empty when there is none.
    pub body: Vec<u8>,
}

/// A response body: already bounded bytes, or a stream the caller reads
/// under its own bound.
pub enum HttpBody {
    /// The whole body, already within the caller's bound.
    Buffered(Vec<u8>),
    /// Chunks as they arrive. `None` is the end.
    Stream(Box<dyn HttpChunks>),
}

/// The next bytes of a response body.
#[async_trait]
pub trait HttpChunks: Send {
    /// The next chunk, or `None` at the end.
    async fn next(&mut self) -> Result<Option<Vec<u8>>, HttpFailure>;
}

/// A status, its headers, and its body.
pub struct HttpResponse {
    /// The HTTP status code.
    pub status: u16,
    /// Header name/value pairs, as the peer sent them.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: HttpBody,
}

impl HttpResponse {
    /// The first value of `name`, compared case-insensitively, trimmed.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find_map(|(header, value)| header.eq_ignore_ascii_case(name).then_some(value.trim()))
    }

    /// Read the body to completion, refusing once `limit` bytes would be
    /// passed. The excess is not kept.
    pub async fn bytes(self, limit: usize) -> Result<Vec<u8>, BodyRead> {
        match self.body {
            HttpBody::Buffered(bytes) => {
                if bytes.len() > limit {
                    Err(BodyRead::TooLarge)
                } else {
                    Ok(bytes)
                }
            }
            HttpBody::Stream(mut stream) => {
                let mut body = Vec::new();
                loop {
                    let Some(chunk) = stream.next().await.map_err(|_| BodyRead::Closed)? else {
                        return Ok(body);
                    };
                    if body.len().saturating_add(chunk.len()) > limit {
                        return Err(BodyRead::TooLarge);
                    }
                    body.extend_from_slice(&chunk);
                }
            }
        }
    }
}

/// Why a bounded body read stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyRead {
    /// The stream ended in an error.
    Closed,
    /// The body grew past the bound. Nothing past the bound was kept.
    TooLarge,
}

/// Sends one HTTP call and returns its status, headers, and body.
///
/// The adapter does not follow redirects. A redirect is a status the caller
/// reads. Resource credentials are not attached by the adapter; the caller
/// puts a bearer on the request only for a URL it has already accepted.
#[async_trait]
pub trait HttpExchange: Send + Sync {
    /// Perform `request`.
    async fn exchange(&self, request: HttpRequest) -> Result<HttpResponse, HttpFailure>;
}
