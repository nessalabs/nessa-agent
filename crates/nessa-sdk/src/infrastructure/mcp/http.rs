//! Streamable HTTP, and the 2024-11-05 HTTP+SSE fallback that follows only an
//! initial POST status of 400, 404, or 405
//! (`docs/adr/todo/392-remote-mcp-servers.md`).
//!
//! ```text
//! Connection writer ──dispatch(one JSON-RPC frame)──▶ HttpSession
//!        ▲                                              ├── POST modern, or legacy endpoint
//!        │                                              ├── HttpExchange
//!        └── reader ◀── inbound JSON messages ─────────┴── optional GET stream
//! ```
//!
//! Arrows are messages. One [`HttpSession`] is one local opening. It keeps the
//! upstream `Mcp-Session-Id` when the server sends one, refuses an id another
//! opening of the same URL already holds, and DELETE that id at most once on
//! close. A 404 on a session-bound request fails that request with
//! [`McpError::SessionExpired`] and starts one fresh `initialize` without the
//! old id; the failed request is not replayed. Legacy mode records that DELETE
//! does not apply.
//!
//! Dropping the session aborts its GET stream and asks for the same single
//! DELETE. The DELETE itself is best-effort and bounded by the exchange.
#![deny(missing_docs)]

use super::authorization::{Bearer, RemoteAuthorization};
use super::framing::MAX_FRAME_BYTES;
use super::http_exchange::{HttpExchange, HttpMethod, HttpRequest, HttpResponse};
use super::remote::RemoteMcpUrl;
use super::sse::{self, SseError, SseParser};
use super::wire::{self, SUPPORTED_VERSIONS};
use super::McpError;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use uuid::Uuid;

/// What one dispatched frame did to the connection.
#[derive(Debug)]
pub(crate) enum SendOutcome {
    /// The frame was accepted. A response, if any, arrives on the inbound channel.
    Done,
    /// Fail the call with this id and leave the session up.
    FailCall { id: Option<u64>, error: McpError },
    /// End the session. Every pending call gets `error`.
    End(McpError),
}

/// Upstream session ids held by openings in this process, keyed by endpoint
/// URL plus the server's id. A second claim of the same pair fails; the loser
/// does not DELETE it.
#[derive(Default)]
pub struct SessionClaims {
    held: Mutex<HashMap<(String, String), u64>>,
}

impl SessionClaims {
    /// Claim `session_id` at `url` for `local`. False when another local id
    /// already holds it.
    pub fn claim(&self, url: &str, session_id: &str, local: u64) -> bool {
        let mut held = self.held.lock().expect("session claims");
        let key = (url.to_owned(), session_id.to_owned());
        if let Some(owner) = held.get(&key) {
            return *owner == local;
        }
        held.insert(key, local);
        true
    }

    /// Release a claim `local` still holds.
    pub fn release(&self, url: &str, session_id: &str, local: u64) {
        let mut held = self.held.lock().expect("session claims");
        let key = (url.to_owned(), session_id.to_owned());
        if held.get(&key).copied() == Some(local) {
            held.remove(&key);
        }
    }
}

struct Phase {
    /// `None` when the server omitted `Mcp-Session-Id`.
    session_id: Option<String>,
    version: Option<String>,
    /// Set once the initial POST took the legacy path.
    legacy_post: Option<String>,
    /// The id was claimed in [`SessionClaims`].
    claimed: bool,
    /// One 404 recovery has been spent.
    recovery_used: bool,
    open: bool,
}

/// One remote opening's HTTP session.
pub struct HttpSession {
    local: u64,
    server: Uuid,
    url: RemoteMcpUrl,
    exchange: Arc<dyn HttpExchange>,
    authorization: Arc<dyn RemoteAuthorization>,
    claims: Arc<SessionClaims>,
    inbound: mpsc::Sender<Result<Vec<u8>, McpError>>,
    phase: Mutex<Phase>,
    closing: AtomicBool,
    delete_started: AtomicBool,
    get_task: Mutex<Option<JoinHandle<()>>>,
    /// Becomes true once close has nothing left to wait for, including a
    /// DELETE that does not apply.
    finished: watch::Sender<bool>,
}

/// The next local id handed to a session.
static NEXT_LOCAL: AtomicU64 = AtomicU64::new(1);

impl HttpSession {
    /// A session that posts to `url` through `exchange`. `incoming` is the
    /// JSON messages the connection reader consumes. `finished` starts false
    /// until [`Self::shutdown`].
    pub fn open(
        server: Uuid,
        url: RemoteMcpUrl,
        exchange: Arc<dyn HttpExchange>,
        authorization: Arc<dyn RemoteAuthorization>,
        claims: Arc<SessionClaims>,
    ) -> (Arc<Self>, mpsc::Receiver<Result<Vec<u8>, McpError>>) {
        let (inbound, incoming) = mpsc::channel(64);
        let session = Arc::new(Self {
            local: NEXT_LOCAL.fetch_add(1, Ordering::Relaxed),
            server,
            url,
            exchange,
            authorization,
            claims,
            inbound,
            phase: Mutex::new(Phase {
                session_id: None,
                version: None,
                legacy_post: None,
                claimed: false,
                recovery_used: false,
                open: false,
            }),
            closing: AtomicBool::new(false),
            delete_started: AtomicBool::new(false),
            get_task: Mutex::new(None),
            finished: watch::channel(false).0,
        });
        (session, incoming)
    }

    /// Resolves when shutdown has recorded its DELETE observation, or at once
    /// when DELETE does not apply. Subscribed before [`Self::shutdown`].
    pub fn finished(&self) -> watch::Receiver<bool> {
        self.finished.subscribe()
    }

    /// Stop the GET stream and DELETE a claimed modern session id once.
    /// Legacy sessions and sessions with no id record that DELETE does not
    /// apply and do not send one.
    pub fn shutdown(&self) {
        if self.delete_started.swap(true, Ordering::SeqCst) {
            return;
        }
        self.closing.store(true, Ordering::SeqCst);
        if let Some(task) = self.get_task.lock().expect("get stream").take() {
            task.abort();
        }
        let phase = self.phase.lock().expect("http phase");
        let legacy = phase.legacy_post.is_some();
        let session_id = phase
            .session_id
            .clone()
            .filter(|_| phase.claimed && !legacy);
        let url = self.url.as_str().to_owned();
        if phase.claimed {
            if let Some(id) = &phase.session_id {
                self.claims.release(&url, id, self.local);
            }
        }
        drop(phase);
        let Some(session_id) = session_id else {
            self.finished.send_replace(true);
            return;
        };
        let exchange = self.exchange.clone();
        let endpoint = self.url.as_str().to_owned();
        let finished = self.finished.clone();
        tokio::spawn(async move {
            let request = HttpRequest {
                method: HttpMethod::Delete,
                url: endpoint,
                headers: vec![
                    ("Mcp-Session-Id".into(), session_id),
                    (
                        "Accept".into(),
                        "application/json, text/event-stream".into(),
                    ),
                ],
                body: Vec::new(),
            };
            let _ = exchange.exchange(request).await;
            finished.send_replace(true);
        });
    }

    /// Send one newline-framed JSON-RPC message.
    pub async fn dispatch(&self, frame: &[u8]) -> SendOutcome {
        if self.closing.load(Ordering::SeqCst) {
            return SendOutcome::End(McpError::Closed);
        }
        let bytes = frame.strip_suffix(b"\n").unwrap_or(frame).trim_ascii();
        let message: Value = match serde_json::from_slice(bytes) {
            Ok(message) => message,
            Err(_) => {
                return SendOutcome::End(McpError::Malformed(
                    "an outgoing MCP frame is not JSON".into(),
                ))
            }
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let id = message.get("id").and_then(Value::as_u64);
        let legacy = self.phase.lock().expect("http phase").legacy_post.clone();
        if let Some(post) = legacy {
            return self.post_legacy(&post, bytes, id).await;
        }
        match self.post_modern(bytes, method.as_deref(), id).await {
            Ok(Modern::Response(body)) => self.deliver_body(body, method.as_deref(), id).await,
            Ok(Modern::Accepted) => SendOutcome::Done,
            Ok(Modern::Legacy) => self.enter_legacy(bytes, id).await,
            Err(error) => self.terminal(id, error),
        }
    }

    fn terminal(&self, id: Option<u64>, error: McpError) -> SendOutcome {
        match error {
            McpError::SessionExpired => SendOutcome::FailCall { id, error },
            other => SendOutcome::End(other),
        }
    }

    async fn deliver_body(
        &self,
        response: HttpResponse,
        method: Option<&str>,
        id: Option<u64>,
    ) -> SendOutcome {
        let status = response.status;
        if status == 202 {
            return SendOutcome::Done;
        }
        if status == 401 {
            return SendOutcome::End(McpError::Unauthorized);
        }
        if status == 403 {
            let challenge = response
                .header("www-authenticate")
                .unwrap_or("Bearer")
                .to_owned();
            self.authorization
                .insufficient_scope(self.server, &challenge)
                .await;
            return SendOutcome::End(McpError::InsufficientScope);
        }
        if status == 404 && self.has_session_id() && method != Some("initialize") {
            return self.recover(id).await;
        }
        if matches!(status, 400 | 404 | 405) && method == Some("initialize") && !self.is_open() {
            return SendOutcome::Done; // caller enters legacy; unused path
        }
        if !(200..300).contains(&status) {
            let error = if status >= 500 {
                McpError::Unreachable
            } else {
                McpError::Malformed(format!("HTTP {status}"))
            };
            return if method == Some("initialize") {
                SendOutcome::End(error)
            } else {
                SendOutcome::FailCall { id, error }
            };
        }
        let content_type = response.header("content-type").map(str::to_owned);
        let session_header = response.header("mcp-session-id").map(str::to_owned);
        if sse::is_event_stream(content_type.as_deref()) {
            return self
                .read_sse_body(response, method, id, session_header)
                .await;
        }
        if content_type.is_some() && !sse::is_json(content_type.as_deref()) {
            return SendOutcome::End(McpError::Malformed(
                "the MCP response content type is neither JSON nor event-stream".into(),
            ));
        }
        let bytes = match response.bytes(MAX_FRAME_BYTES).await {
            Ok(bytes) => bytes,
            Err(super::http_exchange::BodyRead::TooLarge) => {
                return SendOutcome::End(McpError::TooLarge("an HTTP response body"))
            }
            Err(super::http_exchange::BodyRead::Closed) => {
                return SendOutcome::End(McpError::Unconfirmed)
            }
        };
        if let Err(error) = self.note_response(&bytes, method, session_header.as_deref()) {
            return SendOutcome::End(error);
        }
        self.push_inbound(bytes).await
    }

    async fn post_modern(
        &self,
        body: &[u8],
        method: Option<&str>,
        id: Option<u64>,
    ) -> Result<Modern, McpError> {
        let bearer = self.authorization.bearer(self.server).await?;
        let first = self.exchange(body, bearer.as_ref()).await?;
        if first.status == 401 {
            let challenge = first
                .header("www-authenticate")
                .unwrap_or("Bearer")
                .to_owned();
            // Drop the body without treating 401 as legacy.
            drop(first);
            if let Some(retry) = self.authorization.rejected(self.server, &challenge).await? {
                let second = self.exchange(body, Some(&retry)).await?;
                return self.classify(second, method, id).await;
            }
            return Err(McpError::Unauthorized);
        }
        self.classify(first, method, id).await
    }

    async fn classify(
        &self,
        response: HttpResponse,
        method: Option<&str>,
        _id: Option<u64>,
    ) -> Result<Modern, McpError> {
        if matches!(response.status, 400 | 404 | 405)
            && method == Some("initialize")
            && !self.is_open()
            && self.phase.lock().expect("http phase").session_id.is_none()
        {
            return Ok(Modern::Legacy);
        }
        if response.status == 202 {
            return Ok(Modern::Accepted);
        }
        Ok(Modern::Response(response))
    }

    async fn exchange(
        &self,
        body: &[u8],
        bearer: Option<&Bearer>,
    ) -> Result<HttpResponse, McpError> {
        if self.closing.load(Ordering::SeqCst) {
            return Err(McpError::Closed);
        }
        let (session_id, version) = {
            let phase = self.phase.lock().expect("http phase");
            (phase.session_id.clone(), phase.version.clone())
        };
        let include_version = version.is_some() && !body_is_initialize(body);
        let mut headers = vec![
            (
                "Accept".into(),
                "application/json, text/event-stream".into(),
            ),
            ("Content-Type".into(), "application/json".into()),
        ];
        if include_version {
            if let Some(version) = version {
                headers.push(("MCP-Protocol-Version".into(), version));
            }
        }
        if let Some(session_id) = session_id {
            if !body_is_initialize(body) {
                headers.push(("Mcp-Session-Id".into(), session_id));
            }
        }
        if let Some(bearer) = bearer {
            headers.push(("Authorization".into(), format!("Bearer {}", bearer.token())));
        }
        self.exchange
            .exchange(HttpRequest {
                method: HttpMethod::Post,
                url: self.url.as_str().to_owned(),
                headers,
                body: body.to_vec(),
            })
            .await
            .map_err(|_| {
                // `HttpFailure` carries no status. The call did not complete,
                // so it is unreachable on the opening request and on a later
                // one. A body that has already started is handled above.
                McpError::Unreachable
            })
    }

    async fn enter_legacy(&self, initialize: &[u8], id: Option<u64>) -> SendOutcome {
        let response = match self
            .exchange
            .exchange(HttpRequest {
                method: HttpMethod::Get,
                url: self.url.as_str().to_owned(),
                headers: vec![("Accept".into(), "text/event-stream".into())],
                body: Vec::new(),
            })
            .await
        {
            Ok(response) => response,
            Err(_) => return SendOutcome::End(McpError::Unreachable),
        };
        if response.status != 200 || !sse::is_event_stream(response.header("content-type")) {
            return SendOutcome::End(McpError::Handshake(
                "the legacy SSE stream did not start".into(),
            ));
        }
        let (endpoint_tx, endpoint_rx) = tokio::sync::oneshot::channel();
        let inbound = self.inbound.clone();
        let closing = &self.closing;
        let task = tokio::spawn(read_legacy_stream(response, inbound, Some(endpoint_tx)));
        *self.get_task.lock().expect("get stream") = Some(task);
        let endpoint = match endpoint_rx.await {
            Ok(endpoint) => endpoint,
            Err(_) => return SendOutcome::End(McpError::Handshake("no endpoint event".into())),
        };
        let Some(post) = self.url.same_origin(&endpoint) else {
            return SendOutcome::End(McpError::Handshake(
                "the legacy endpoint is not on the configured origin".into(),
            ));
        };
        if closing.load(Ordering::SeqCst) {
            return SendOutcome::End(McpError::Closed);
        }
        self.phase.lock().expect("http phase").legacy_post = Some(post.as_str().to_owned());
        self.phase.lock().expect("http phase").open = true;
        self.post_legacy(post.as_str(), initialize, id).await
    }

    async fn post_legacy(&self, post: &str, body: &[u8], id: Option<u64>) -> SendOutcome {
        let bearer = match self.authorization.bearer(self.server).await {
            Ok(bearer) => bearer,
            Err(error) => return SendOutcome::End(error),
        };
        let mut headers = vec![("Content-Type".into(), "application/json".into())];
        if let Some(bearer) = bearer {
            headers.push(("Authorization".into(), format!("Bearer {}", bearer.token())));
        }
        match self
            .exchange
            .exchange(HttpRequest {
                method: HttpMethod::Post,
                url: post.to_owned(),
                headers,
                body: body.to_vec(),
            })
            .await
        {
            Ok(response) if response.status == 202 || (200..300).contains(&response.status) => {
                SendOutcome::Done
            }
            Ok(response) if response.status == 401 => {
                let challenge = response
                    .header("www-authenticate")
                    .unwrap_or("Bearer")
                    .to_owned();
                match self.authorization.rejected(self.server, &challenge).await {
                    Ok(Some(retry)) => {
                        let mut headers = vec![("Content-Type".into(), "application/json".into())];
                        headers.push(("Authorization".into(), format!("Bearer {}", retry.token())));
                        match self
                            .exchange
                            .exchange(HttpRequest {
                                method: HttpMethod::Post,
                                url: post.to_owned(),
                                headers,
                                body: body.to_vec(),
                            })
                            .await
                        {
                            Ok(again)
                                if again.status == 202 || (200..300).contains(&again.status) =>
                            {
                                SendOutcome::Done
                            }
                            Ok(_) => SendOutcome::End(McpError::Unauthorized),
                            Err(_) => SendOutcome::End(McpError::Unreachable),
                        }
                    }
                    Ok(None) | Err(McpError::Unauthorized) => {
                        SendOutcome::End(McpError::Unauthorized)
                    }
                    Err(error) => SendOutcome::End(error),
                }
            }
            Ok(_) => SendOutcome::FailCall {
                id,
                error: McpError::Unreachable,
            },
            Err(_) => SendOutcome::End(McpError::Unreachable),
        }
    }

    async fn read_sse_body(
        &self,
        response: HttpResponse,
        method: Option<&str>,
        id: Option<u64>,
        session_header: Option<String>,
    ) -> SendOutcome {
        let mut stream = match response.body {
            super::http_exchange::HttpBody::Stream(stream) => stream,
            super::http_exchange::HttpBody::Buffered(bytes) => {
                return self
                    .finish_sse_bytes(&bytes, method, id, session_header)
                    .await
            }
        };
        let mut parser = SseParser::bounded();
        let mut saw = false;
        loop {
            let chunk = match stream.next().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(_) => return SendOutcome::End(McpError::Unconfirmed),
            };
            let events = match parser.push(&chunk) {
                Ok(events) => events,
                Err(SseError::TooLarge) => {
                    return SendOutcome::End(McpError::TooLarge("an SSE event"))
                }
                Err(SseError::Utf8) => {
                    return SendOutcome::End(McpError::Malformed(
                        "an SSE event is not UTF-8".into(),
                    ))
                }
            };
            for event in events {
                if event.data.is_empty() {
                    continue;
                }
                if let Err(error) =
                    self.note_response(event.data.as_bytes(), method, session_header.as_deref())
                {
                    return SendOutcome::End(error);
                }
                saw = true;
                if let SendOutcome::End(error) = self.push_inbound(event.data.into_bytes()).await {
                    return SendOutcome::End(error);
                }
            }
        }
        if saw {
            SendOutcome::Done
        } else {
            SendOutcome::FailCall {
                id,
                error: McpError::Unconfirmed,
            }
        }
    }

    async fn finish_sse_bytes(
        &self,
        bytes: &[u8],
        method: Option<&str>,
        id: Option<u64>,
        session_header: Option<String>,
    ) -> SendOutcome {
        let mut parser = SseParser::bounded();
        let events = match parser.push(bytes) {
            Ok(events) => events,
            Err(SseError::TooLarge) => return SendOutcome::End(McpError::TooLarge("an SSE event")),
            Err(SseError::Utf8) => {
                return SendOutcome::End(McpError::Malformed("an SSE event is not UTF-8".into()))
            }
        };
        // A buffered body may not end with a blank line. Parse a trailing event.
        let mut events = events;
        if !bytes.ends_with(b"\n\n") {
            let mut extra = parser;
            if let Ok(more) = extra.push(b"\n\n") {
                events.extend(more);
            }
        }
        if events.is_empty() {
            return SendOutcome::FailCall {
                id,
                error: McpError::Malformed("an SSE response carried no event".into()),
            };
        }
        for event in events {
            if event.data.is_empty() {
                continue;
            }
            if let Err(error) =
                self.note_response(event.data.as_bytes(), method, session_header.as_deref())
            {
                return SendOutcome::End(error);
            }
            if let SendOutcome::End(error) = self.push_inbound(event.data.into_bytes()).await {
                return SendOutcome::End(error);
            }
        }
        SendOutcome::Done
    }

    fn note_response(
        &self,
        body: &[u8],
        method: Option<&str>,
        session_header: Option<&str>,
    ) -> Result<(), McpError> {
        if method != Some("initialize") {
            return Ok(());
        }
        let message: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        if message.get("result").is_none() {
            return Ok(());
        }
        if let Some(version) = message
            .get("result")
            .and_then(|result| result.get("protocolVersion"))
            .and_then(Value::as_str)
        {
            if SUPPORTED_VERSIONS.contains(&version) {
                self.phase.lock().expect("http phase").version = Some(version.to_owned());
            }
        }
        let Some(session_id) = session_header.filter(|id| !id.is_empty()) else {
            self.phase.lock().expect("http phase").open = true;
            self.start_get(None);
            return Ok(());
        };
        if session_id.len() > 1024 {
            return Err(McpError::TooLarge("Mcp-Session-Id"));
        }
        if !self.claims.claim(self.url.as_str(), session_id, self.local) {
            return Err(McpError::SessionCollision);
        }
        let mut phase = self.phase.lock().expect("http phase");
        phase.session_id = Some(session_id.to_owned());
        phase.claimed = true;
        phase.open = true;
        drop(phase);
        self.start_get(Some(session_id.to_owned()));
        Ok(())
    }

    fn start_get(&self, session_id: Option<String>) {
        if self.phase.lock().expect("http phase").legacy_post.is_some() {
            return;
        }
        let Some(session_id) = session_id else {
            return;
        };
        let exchange = self.exchange.clone();
        let url = self.url.as_str().to_owned();
        let version = self.phase.lock().expect("http phase").version.clone();
        let inbound = self.inbound.clone();
        let task = tokio::spawn(async move {
            let mut headers = vec![("Accept".into(), "text/event-stream".into())];
            headers.push(("Mcp-Session-Id".into(), session_id));
            if let Some(version) = version {
                headers.push(("MCP-Protocol-Version".into(), version));
            }
            let response = match exchange
                .exchange(HttpRequest {
                    method: HttpMethod::Get,
                    url,
                    headers,
                    body: Vec::new(),
                })
                .await
            {
                Ok(response) => response,
                Err(_) => return,
            };
            if response.status == 405 || response.status != 200 {
                return;
            }
            read_legacy_stream(response, inbound, None).await;
        });
        if let Some(previous) = self.get_task.lock().expect("get stream").replace(task) {
            previous.abort();
        }
    }

    async fn recover(&self, id: Option<u64>) -> SendOutcome {
        let (old, claimed) = {
            let mut phase = self.phase.lock().expect("http phase");
            if phase.recovery_used {
                return SendOutcome::End(McpError::SessionExpired);
            }
            phase.recovery_used = true;
            let old = phase.session_id.take();
            let claimed = phase.claimed;
            phase.claimed = false;
            phase.open = false;
            phase.version = None;
            (old, claimed)
        };
        if claimed {
            if let Some(old) = &old {
                self.claims.release(self.url.as_str(), old, self.local);
            }
        }
        if let Some(task) = self.get_task.lock().expect("get stream").take() {
            task.abort();
        }
        if self.closing.load(Ordering::SeqCst) {
            return SendOutcome::End(McpError::Closed);
        }
        let bearer = match self.authorization.bearer(self.server).await {
            Ok(bearer) => bearer,
            Err(error) => return SendOutcome::End(error),
        };
        let body = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": wire::initialize_params(),
        }))
        .unwrap_or_default();
        let response = match self.exchange(&body, bearer.as_ref()).await {
            Ok(response) => response,
            Err(error) => return SendOutcome::End(error),
        };
        if !(200..300).contains(&response.status) || response.status == 202 {
            return SendOutcome::End(McpError::SessionExpired);
        }
        let header = response.header("mcp-session-id").map(str::to_owned);
        let bytes = match response.bytes(MAX_FRAME_BYTES).await {
            Ok(bytes) => bytes,
            Err(_) => return SendOutcome::End(McpError::SessionExpired),
        };
        if self.closing.load(Ordering::SeqCst) {
            if let Some(new_id) = &header {
                self.delete_id(new_id);
            }
            return SendOutcome::End(McpError::Closed);
        }
        if let Err(error) = self.note_response(&bytes, Some("initialize"), header.as_deref()) {
            return SendOutcome::End(error);
        }
        let note = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .unwrap_or_default();
        let notified = match self.authorization.bearer(self.server).await {
            Ok(bearer) => bearer,
            Err(error) => return SendOutcome::End(error),
        };
        let _ = self.exchange(&note, notified.as_ref()).await;
        SendOutcome::FailCall {
            id,
            error: McpError::SessionExpired,
        }
    }

    fn delete_id(&self, session_id: &str) {
        let exchange = self.exchange.clone();
        let url = self.url.as_str().to_owned();
        let session_id = session_id.to_owned();
        tokio::spawn(async move {
            let _ = exchange
                .exchange(HttpRequest {
                    method: HttpMethod::Delete,
                    url,
                    headers: vec![("Mcp-Session-Id".into(), session_id)],
                    body: Vec::new(),
                })
                .await;
        });
    }

    fn has_session_id(&self) -> bool {
        self.phase.lock().expect("http phase").session_id.is_some()
    }

    fn is_open(&self) -> bool {
        self.phase.lock().expect("http phase").open
    }

    async fn push_inbound(&self, bytes: Vec<u8>) -> SendOutcome {
        match self.inbound.send(Ok(bytes)).await {
            Ok(()) => SendOutcome::Done,
            Err(_) => SendOutcome::End(McpError::ServerGone),
        }
    }
}

impl Drop for HttpSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

enum Modern {
    Response(HttpResponse),
    Accepted,
    Legacy,
}

fn body_is_initialize(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|message| {
            message
                .get("method")
                .and_then(Value::as_str)
                .map(|method| method == "initialize")
        })
        .unwrap_or(false)
}

async fn read_legacy_stream(
    response: HttpResponse,
    inbound: mpsc::Sender<Result<Vec<u8>, McpError>>,
    mut endpoint: Option<tokio::sync::oneshot::Sender<String>>,
) {
    let mut stream = match response.body {
        super::http_exchange::HttpBody::Stream(stream) => stream,
        super::http_exchange::HttpBody::Buffered(bytes) => {
            let mut parser = SseParser::bounded();
            if let Ok(events) = parser.push(&bytes) {
                for event in events {
                    if event.event == "endpoint" {
                        if let Some(tx) = endpoint.take() {
                            let _ = tx.send(event.data);
                        }
                    } else if !event.data.is_empty() {
                        let _ = inbound.send(Ok(event.data.into_bytes())).await;
                    }
                }
            }
            return;
        }
    };
    let mut parser = SseParser::bounded();
    loop {
        let chunk = match stream.next().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) | Err(_) => {
                let _ = inbound.send(Err(McpError::Unconfirmed)).await;
                return;
            }
        };
        let events = match parser.push(&chunk) {
            Ok(events) => events,
            Err(SseError::TooLarge) => {
                let _ = inbound.send(Err(McpError::TooLarge("an SSE event"))).await;
                return;
            }
            Err(SseError::Utf8) => {
                let _ = inbound
                    .send(Err(McpError::Malformed("an SSE event is not UTF-8".into())))
                    .await;
                return;
            }
        };
        for event in events {
            if event.event == "endpoint" {
                if let Some(tx) = endpoint.take() {
                    let _ = tx.send(event.data);
                }
                continue;
            }
            if event.data.is_empty() {
                continue;
            }
            if inbound.send(Ok(event.data.into_bytes())).await.is_err() {
                return;
            }
        }
    }
}
