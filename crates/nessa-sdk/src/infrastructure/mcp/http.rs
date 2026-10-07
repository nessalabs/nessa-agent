//! Streamable HTTP, and the 2024-11-05 HTTP+SSE fallback that follows only an
//! initial POST status of 400, 404, or 405
//! (`docs/adr/todo/392-remote-mcp-servers.md`).
//!
//! ```text
//! Connection writer ──dispatch(one JSON-RPC frame)──▶ HttpSession
//!        ▲                                              ├── POST modern, or legacy endpoint
//!        │                                              ├── HttpExchange
//!        │                                              ├── bounded owned POST readers (abort/join on close)
//!        └── reader ◀── inbound JSON messages ─────────┴── optional GET stream
//! ```
//!
//! Arrows are messages. One [`HttpSession`] is one local opening. It keeps the
//! upstream `Mcp-Session-Id` when the server sends one, refuses an id another
//! opening of the same URL already holds, and DELETE that id at most once on
//! close. A 404 on a session-bound request fails that request with
//! [`McpError::SessionExpired`] and starts one fresh `initialize` without the
//! old id under its registered startup owner. Ordinary frames and initialized
//! use the connection writer; the failed request is not replayed.
//! Legacy mode records that DELETE
//! does not apply.
//!
//! Streamed POST bodies (JSON and SSE, including initialize) are session-owned
//! and separately bounded. An SSE request reader
//! forwards notices/requests until its matching result/error, then drops the body
//! even if the peer leaves it open (ADR 392 P1–P14/J1–J9,
//! `tests::post_streams` and `tests::http_progress`).
//!
//! Dropping the session aborts its GET and POST streams and asks for the same single
//! DELETE. The DELETE itself is best-effort and bounded by the exchange.
#![deny(missing_docs)]

use super::authorization::{Bearer, RemoteAuthorization};
use super::connection::Outgoing;
use super::framing::MAX_FRAME_BYTES;
use super::http_exchange::{HttpExchange, HttpMethod, HttpRequest, HttpResponse};
use super::remote::RemoteMcpUrl;
use super::sse::{self, SseError, SseParser};
use super::wire;
use super::McpError;
use crate::infrastructure::clock::{within, Clock, ClockInstant};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch, OwnedSemaphorePermit, Semaphore};
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
    /// The id was claimed in [`SessionClaims`]; close retains it through DELETE.
    claimed: bool,
    recovery: Recovery,
    open: bool,
}

/// Identity publication and replacement call admission are distinct transitions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Recovery {
    Available,
    Initializing,
    ReadyForWriter,
    Completed,
    Failed,
}

impl Recovery {
    fn blocks_requests(self) -> bool {
        match self {
            Self::Available | Self::Completed => false,
            Self::Initializing | Self::ReadyForWriter | Self::Failed => true,
        }
    }
}

/// One remote opening's HTTP session.
pub struct HttpSession {
    local: u64,
    weak: Weak<Self>,
    writer: Mutex<Option<HttpWriter>>,
    runtime: Handle,
    server: Uuid,
    url: RemoteMcpUrl,
    exchange: Arc<dyn HttpExchange>,
    authorization: Arc<dyn RemoteAuthorization>,
    claims: Arc<SessionClaims>,
    inbound: mpsc::Sender<Result<Vec<u8>, McpError>>,
    phase: Mutex<Phase>,
    closing: AtomicBool,
    delete_started: AtomicBool,
    readers: Mutex<ReaderTasks>,
    post_capacity: Arc<Semaphore>,
    /// Becomes true once close has nothing left to wait for, including a
    /// DELETE that does not apply.
    finished: watch::Sender<bool>,
}

#[derive(Clone)]
struct HttpWriter {
    outgoing: mpsc::Sender<Outgoing>,
    clock: Arc<dyn Clock>,
}

/// Retained POST body/startup readers, independent of pending JSON-RPC call capacity.
pub(crate) const MAX_POST_STREAMS: usize = 256;

#[derive(Default)]
struct ReaderTasks {
    get: Vec<JoinHandle<()>>,
    post: Vec<PostReader>,
}

impl ReaderTasks {
    fn stop_get(&mut self) {
        for task in &self.get {
            task.abort();
        }
        self.get.retain(|task| !task.is_finished());
    }
}

struct PostReader {
    request_id: Option<u64>,
    task: JoinHandle<()>,
    /// Held until this handle is reaped or joined, including after abort.
    _permit: OwnedSemaphorePermit,
}

/// The next local id handed to a session.
static NEXT_LOCAL: AtomicU64 = AtomicU64::new(1);

impl HttpSession {
    /// A session that posts to `url` through `exchange`. `incoming` is the
    /// JSON messages the connection reader consumes. `finished` starts false
    /// until [`Self::shutdown`]. The current Tokio runtime owns reader/close
    /// tasks and must remain running through [`Self::finished`], including when
    /// this handle is dropped from an ordinary thread.
    ///
    /// # Panics
    ///
    /// Panics when called outside a Tokio runtime.
    pub fn open(
        server: Uuid,
        url: RemoteMcpUrl,
        exchange: Arc<dyn HttpExchange>,
        authorization: Arc<dyn RemoteAuthorization>,
        claims: Arc<SessionClaims>,
    ) -> (Arc<Self>, mpsc::Receiver<Result<Vec<u8>, McpError>>) {
        let (inbound, incoming) = mpsc::channel(64);
        let session = Arc::new_cyclic(|weak| Self {
            local: NEXT_LOCAL.fetch_add(1, Ordering::Relaxed),
            weak: weak.clone(),
            writer: Mutex::new(None),
            runtime: Handle::current(),
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
                recovery: Recovery::Available,
                open: false,
            }),
            closing: AtomicBool::new(false),
            delete_started: AtomicBool::new(false),
            readers: Mutex::new(ReaderTasks::default()),
            post_capacity: Arc::new(Semaphore::new(MAX_POST_STREAMS)),
            finished: watch::channel(false).0,
        });
        (session, incoming)
    }

    /// Resolves after shutdown joins owned readers and records its DELETE
    /// observation (or that DELETE does not apply). Subscribe before shutdown.
    pub fn finished(&self) -> watch::Receiver<bool> {
        self.finished.subscribe()
    }

    /// Stop owned GET/POST streams and DELETE a claimed modern session id once.
    /// [`Self::finished`] observes completion after reader joins and DELETE.
    /// Legacy sessions and sessions with no id record that DELETE does not
    /// apply and do not send one.
    pub fn shutdown(&self) {
        if self.delete_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let (readers, get_task): (Vec<_>, _) = {
            let mut readers = self.readers.lock().expect("http readers");
            self.closing.store(true, Ordering::SeqCst);
            (
                std::mem::take(&mut readers.post),
                std::mem::take(&mut readers.get),
            )
        };
        for task in &get_task {
            task.abort();
        }
        for reader in &readers {
            reader.task.abort();
        }
        let phase = self.phase.lock().expect("http phase");
        let legacy = phase.legacy_post.is_some();
        let session_id = phase
            .session_id
            .clone()
            .filter(|_| phase.claimed && !legacy);
        let url = self.url.as_str().to_owned();
        let claims = self.claims.clone();
        let local = self.local;
        drop(phase);

        let exchange = self.exchange.clone();
        let endpoint = self.url.as_str().to_owned();
        let finished = self.finished.clone();
        let authorization = self.authorization.clone();
        let server = self.server;
        self.runtime.spawn(async move {
            for reader in readers {
                let _ = reader.task.await;
            }
            for task in get_task {
                let _ = task.await;
            }
            let Some(session_id) = session_id else {
                finished.send_replace(true);
                return;
            };
            let mut headers = vec![
                ("Mcp-Session-Id".into(), session_id.clone()),
                (
                    "Accept".into(),
                    "application/json, text/event-stream".into(),
                ),
            ];
            if let Ok(Some(bearer)) = authorization.bearer(server).await {
                headers.push(("Authorization".into(), format!("Bearer {}", bearer.token())));
            }
            let request = HttpRequest {
                method: HttpMethod::Delete,
                url: endpoint,
                headers,
                body: Vec::new(),
            };
            let _ = exchange.exchange(request).await;
            claims.release(&url, &session_id, local);
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
        if method.as_deref() == Some("notifications/cancelled") {
            if let Some(id) = message
                .get("params")
                .and_then(|params| params.get("requestId"))
                .and_then(Value::as_u64)
            {
                self.cancel_post(id);
            }
        }
        if method.is_some()
            && id.is_some()
            && self
                .phase
                .lock()
                .expect("http phase")
                .recovery
                .blocks_requests()
        {
            return SendOutcome::FailCall {
                id,
                error: McpError::Busy,
            };
        }
        let legacy = self.phase.lock().expect("http phase").legacy_post.clone();
        if let Some(post) = legacy {
            return self.post_legacy(&post, bytes, id).await;
        }
        let permit = if id.is_some() && method.is_some() {
            self.reap_post_readers();
            match self.post_capacity.clone().try_acquire_owned() {
                Ok(permit) => Some(permit),
                Err(_) => {
                    return SendOutcome::FailCall {
                        id,
                        error: McpError::Busy,
                    }
                }
            }
        } else {
            None
        };
        match self.post_modern(bytes, method.as_deref(), id).await {
            Ok(Modern::Response(body)) => {
                self.deliver_body(body, method.as_deref(), id, permit).await
            }
            Ok(Modern::Accepted) => SendOutcome::Done,
            Ok(Modern::Legacy) => self.enter_legacy(bytes, id).await,
            Err(error) => self.terminal(id, error),
        }
    }

    /// Stop this call's local POST reader, independently of remote cancellation.
    pub(crate) fn cancel_post(&self, id: u64) {
        for reader in self.readers.lock().expect("http readers").post.iter() {
            if reader.request_id == Some(id) {
                reader.task.abort();
            }
        }
    }

    fn reap_post_readers(&self) {
        self.readers
            .lock()
            .expect("http readers")
            .post
            .retain(|reader| !reader.task.is_finished());
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
        permit: Option<OwnedSemaphorePermit>,
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
            return self.recover(id, permit);
        }
        if matches!(status, 400 | 404 | 405) && method == Some("initialize") && !self.is_open() {
            return SendOutcome::Done; // caller enters legacy; unused path
        }
        if !(200..300).contains(&status) {
            // 5xx means the server accepted the request. Only `initialize`
            // is still a definite miss: no session exists to have run anything.
            if status >= 500 {
                return server_error(method == Some("initialize"), id);
            }
            let error = McpError::Malformed(format!("HTTP {status}"));
            return if method == Some("initialize") {
                SendOutcome::End(error)
            } else {
                SendOutcome::FailCall { id, error }
            };
        }
        let content_type = response.header("content-type").map(str::to_owned);
        let event_stream = sse::is_event_stream(content_type.as_deref());
        if !event_stream && content_type.is_some() && !sse::is_json(content_type.as_deref()) {
            return SendOutcome::End(McpError::Malformed(
                "the MCP response content type is neither JSON nor event-stream".into(),
            ));
        }
        if matches!(response.body, super::http_exchange::HttpBody::Stream(_)) {
            let request_id = method.and(id);
            let mut readers = self.readers.lock().expect("http readers");
            readers.post.retain(|reader| !reader.task.is_finished());
            if self.closing.load(Ordering::SeqCst) {
                return SendOutcome::End(McpError::Closed);
            }
            let permit = permit.or_else(|| self.post_capacity.clone().try_acquire_owned().ok());
            let Some(permit) = permit else {
                return SendOutcome::End(McpError::Unconfirmed);
            };
            let inbound = self.inbound.clone();
            let weak = self.weak.clone();
            let initialize = method == Some("initialize");
            let task = self.runtime.spawn(async move {
                match consume_body(
                    response,
                    event_stream,
                    request_id,
                    initialize,
                    &weak,
                    &inbound,
                )
                .await
                {
                    Ok(_) => {}
                    Err(error) => {
                        let _ = inbound.send(Err(error)).await;
                    }
                }
            });
            readers.post.push(PostReader {
                request_id,
                task,
                _permit: permit,
            });
            return SendOutcome::Done;
        }
        if self.closing.load(Ordering::SeqCst) {
            return SendOutcome::End(McpError::Closed);
        }
        // Buffered bodies use the same correlation/EOF owner. Queue faults after
        // preceding messages so an observed neighboring reply keeps its outcome.
        match consume_body(
            response,
            event_stream,
            method.and(id),
            method == Some("initialize"),
            &self.weak,
            &self.inbound,
        )
        .await
        {
            Ok(_) => SendOutcome::Done,
            Err(error) => match self.inbound.send(Err(error)).await {
                Ok(()) => SendOutcome::Done,
                Err(_) => SendOutcome::End(McpError::ServerGone),
            },
        }
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
            .map_err(|_| lost_exchange(body))
    }

    async fn enter_legacy(&self, initialize: &[u8], id: Option<u64>) -> SendOutcome {
        let bearer = match self.authorization.bearer(self.server).await {
            Ok(bearer) => bearer,
            Err(error) => return SendOutcome::End(error),
        };
        let mut headers = vec![("Accept".into(), "text/event-stream".into())];
        if let Some(bearer) = bearer {
            headers.push(("Authorization".into(), format!("Bearer {}", bearer.token())));
        }
        let response = match self
            .exchange
            .exchange(HttpRequest {
                method: HttpMethod::Get,
                url: self.url.as_str().to_owned(),
                headers,
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
        {
            let mut readers = self.readers.lock().expect("http readers");
            if self.closing.load(Ordering::SeqCst) {
                return SendOutcome::End(McpError::Closed);
            }
            let task = self
                .runtime
                .spawn(read_legacy_stream(response, inbound, Some(endpoint_tx)));
            readers.stop_get();
            readers.get.push(task);
        }
        let endpoint = match endpoint_rx.await {
            Ok(endpoint) => endpoint,
            Err(_) => return SendOutcome::End(McpError::Handshake("no endpoint event".into())),
        };
        let Some(post) = self.url.same_origin(&endpoint) else {
            return SendOutcome::End(McpError::Handshake(
                "the legacy endpoint is not on the configured origin".into(),
            ));
        };
        {
            let _readers = self.readers.lock().expect("http readers");
            if self.closing.load(Ordering::SeqCst) {
                return SendOutcome::End(McpError::Closed);
            }
            let mut phase = self.phase.lock().expect("http phase");
            phase.legacy_post = Some(post.as_str().to_owned());
            phase.open = true;
        }
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
            Ok(response) if response.status == 403 => self.refuse_scope(response).await,
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
                            Ok(again) if again.status == 403 => self.refuse_scope(again).await,
                            Ok(again) if again.status >= 500 => {
                                server_error(body_is_initialize(body), id)
                            }
                            Ok(_) => SendOutcome::End(McpError::Unauthorized),
                            Err(_) => SendOutcome::End(lost_exchange(body)),
                        }
                    }
                    Ok(None) | Err(McpError::Unauthorized) => {
                        SendOutcome::End(McpError::Unauthorized)
                    }
                    Err(error) => SendOutcome::End(error),
                }
            }
            Ok(response) if response.status >= 500 => server_error(body_is_initialize(body), id),
            Ok(_) => SendOutcome::FailCall {
                id,
                error: McpError::Unreachable,
            },
            Err(_) => SendOutcome::End(lost_exchange(body)),
        }
    }

    async fn refuse_scope(&self, response: HttpResponse) -> SendOutcome {
        let challenge = response
            .header("www-authenticate")
            .unwrap_or("Bearer")
            .to_owned();
        self.authorization
            .insufficient_scope(self.server, &challenge)
            .await;
        SendOutcome::End(McpError::InsufficientScope)
    }

    fn note_response(
        &self,
        body: &[u8],
        method: Option<&str>,
        request_id: Option<u64>,
        session_header: Option<&str>,
    ) -> Result<bool, McpError> {
        if method != Some("initialize") {
            return Ok(false);
        }
        let message: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        if !request_id.is_some_and(|id| is_response_to(&message, id))
            || message.get("result").is_none()
        {
            return Ok(false);
        }
        let mut readers = self.readers.lock().expect("http readers");
        if self.closing.load(Ordering::SeqCst) {
            return Err(McpError::Closed);
        }
        if self.phase.lock().expect("http phase").open {
            return Ok(true);
        }
        let result = &message["result"];
        wire::initialized(result)?;
        let version = result["protocolVersion"].as_str().map(str::to_owned);
        let Some(session_id) = session_header.filter(|id| !id.is_empty()) else {
            let mut phase = self.phase.lock().expect("http phase");
            phase.version = version;
            phase.open = true;
            drop(phase);
            self.start_get(None, &mut readers);
            return Ok(true);
        };
        if session_id.len() > 1024 {
            return Err(McpError::TooLarge("Mcp-Session-Id"));
        }
        if !self.claims.claim(self.url.as_str(), session_id, self.local) {
            return Err(McpError::SessionCollision);
        }
        let mut phase = self.phase.lock().expect("http phase");
        phase.session_id = Some(session_id.to_owned());
        phase.version = version;
        phase.claimed = true;
        phase.open = true;
        drop(phase);
        self.start_get(Some(session_id.to_owned()), &mut readers);
        Ok(true)
    }

    fn start_get(&self, session_id: Option<String>, readers: &mut ReaderTasks) {
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
        let authorization = self.authorization.clone();
        let server = self.server;
        let task = self.runtime.spawn(async move {
            let mut headers = vec![("Accept".into(), "text/event-stream".into())];
            headers.push(("Mcp-Session-Id".into(), session_id));
            if let Some(version) = version {
                headers.push(("MCP-Protocol-Version".into(), version));
            }
            if let Ok(Some(bearer)) = authorization.bearer(server).await {
                headers.push(("Authorization".into(), format!("Bearer {}", bearer.token())));
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
        readers.stop_get();
        readers.get.push(task);
    }

    pub(crate) fn set_writer(&self, writer: mpsc::Sender<Outgoing>, clock: Arc<dyn Clock>) {
        *self.writer.lock().expect("http writer") = Some(HttpWriter {
            outgoing: writer,
            clock,
        });
    }

    fn recover(&self, id: Option<u64>, permit: Option<OwnedSemaphorePermit>) -> SendOutcome {
        let mut readers = self.readers.lock().expect("http readers");
        if self.closing.load(Ordering::SeqCst) {
            return SendOutcome::End(McpError::Closed);
        }
        let mut phase = self.phase.lock().expect("http phase");
        if phase.recovery != Recovery::Available {
            return SendOutcome::End(McpError::SessionExpired);
        }
        phase.recovery = Recovery::Initializing;
        if phase.claimed {
            if let Some(old) = phase.session_id.take() {
                self.claims.release(self.url.as_str(), &old, self.local);
            }
        }
        phase.claimed = false;
        phase.open = false;
        phase.version = None;
        drop(phase);
        readers.stop_get();
        let Some(HttpWriter {
            outgoing: writer,
            clock,
        }) = self.writer.lock().expect("http writer").clone()
        else {
            return SendOutcome::End(McpError::SessionExpired);
        };
        let Some(permit) = permit.or_else(|| self.post_capacity.clone().try_acquire_owned().ok())
        else {
            return SendOutcome::End(McpError::Unconfirmed);
        };
        let exchange = self.exchange.clone();
        let authorization = self.authorization.clone();
        let server = self.server;
        let url = self.url.as_str().to_owned();
        let weak = self.weak.clone();
        let inbound = self.inbound.clone();
        let deadline = clock.now() + super::servers::INITIALIZE_TIMEOUT;
        let (registered, registration) = oneshot::channel();
        let task = self.runtime.spawn(async move {
            if registration.await.is_err() {
                return;
            }
            let startup = async {
                let bearer = authorization.bearer(server).await?;
                let mut headers = vec![
                    (
                        "Accept".into(),
                        "application/json, text/event-stream".into(),
                    ),
                    ("Content-Type".into(), "application/json".into()),
                ];
                if let Some(bearer) = bearer {
                    headers.push(("Authorization".into(), format!("Bearer {}", bearer.token())));
                }
                let body = serde_json::to_vec(&json!({
                    "jsonrpc": "2.0", "id": 0, "method": "initialize",
                    "params": wire::initialize_params(),
                }))
                .unwrap_or_default();
                let response = exchange
                    .exchange(HttpRequest {
                        method: HttpMethod::Post,
                        url,
                        headers,
                        body,
                    })
                    .await
                    .map_err(|_| McpError::SessionExpired)?;
                if !(200..300).contains(&response.status) || response.status == 202 {
                    return Err(McpError::SessionExpired);
                }
                let content_type = response.header("content-type");
                let event_stream = sse::is_event_stream(content_type);
                if !event_stream && content_type.is_some() && !sse::is_json(content_type) {
                    return Err(McpError::SessionExpired);
                }
                if !consume_body(response, event_stream, Some(0), true, &weak, &inbound)
                    .await
                    .map_err(|error| match error {
                        McpError::Unconfirmed => McpError::SessionExpired,
                        other => other,
                    })?
                {
                    return Err(McpError::SessionExpired);
                }
                {
                    let session = weak.upgrade().ok_or(McpError::Closed)?;
                    let _readers = session.readers.lock().expect("http readers");
                    if session.closing.load(Ordering::SeqCst) {
                        return Err(McpError::Closed);
                    }
                    session.phase.lock().expect("http phase").recovery = Recovery::ReadyForWriter;
                }
                let (completed, completion) = oneshot::channel();
                writer
                    .send(Outgoing::RecoveryReady {
                        deadline,
                        completed,
                    })
                    .await
                    .map_err(|_| McpError::Closed)?;
                completion.await.map_err(|_| McpError::Closed)?
            };
            let result = within(&*clock, deadline, startup)
                .await
                .unwrap_or(Err(McpError::Timeout));
            if let Err(error) = result {
                if let Some(session) = weak.upgrade() {
                    let _readers = session.readers.lock().expect("http readers");
                    session.phase.lock().expect("http phase").recovery = Recovery::Failed;
                }
                let _ = inbound.send(Err(error)).await;
            }
        });
        readers.post.push(PostReader {
            request_id: None,
            task,
            _permit: permit,
        });
        let _ = registered.send(());
        SendOutcome::FailCall {
            id,
            error: McpError::SessionExpired,
        }
    }

    pub(crate) async fn finish_recovery(
        &self,
        deadline: ClockInstant,
        completed: &oneshot::Sender<Result<(), McpError>>,
    ) -> SendOutcome {
        if self.closing.load(Ordering::SeqCst)
            || completed.is_closed()
            || self.phase.lock().expect("http phase").recovery != Recovery::ReadyForWriter
        {
            return SendOutcome::End(McpError::Closed);
        }
        let clock = self
            .writer
            .lock()
            .expect("http writer")
            .as_ref()
            .expect("installed HTTP writer")
            .clock
            .clone();
        if clock.now() >= deadline {
            return SendOutcome::End(McpError::Timeout);
        }
        let body =
            serde_json::to_vec(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .unwrap_or_default();
        let sending = async {
            match self
                .post_modern(&body, Some("notifications/initialized"), None)
                .await
            {
                Ok(Modern::Accepted) => SendOutcome::Done,
                Ok(Modern::Response(response)) if response.status == 403 => {
                    self.refuse_scope(response).await
                }
                Ok(_) => SendOutcome::End(McpError::Malformed(
                    "recovery initialized was not accepted".into(),
                )),
                Err(error) => SendOutcome::End(error),
            }
        };
        let outcome = within(&*clock, deadline, sending)
            .await
            .unwrap_or(SendOutcome::End(McpError::Timeout));
        if matches!(outcome, SendOutcome::Done) {
            let _readers = self.readers.lock().expect("http readers");
            let mut phase = self.phase.lock().expect("http phase");
            if self.closing.load(Ordering::SeqCst)
                || completed.is_closed()
                || phase.recovery != Recovery::ReadyForWriter
                || clock.now() >= deadline
            {
                return SendOutcome::End(McpError::Closed);
            }
            phase.recovery = Recovery::Completed;
        }
        outcome
    }

    fn has_session_id(&self) -> bool {
        self.phase.lock().expect("http phase").session_id.is_some()
    }

    fn is_open(&self) -> bool {
        self.phase.lock().expect("http phase").open
    }
}

impl Drop for HttpSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The task owns the body; weak upgrades end before asynchronous delivery.
async fn consume_body(
    response: HttpResponse,
    event_stream: bool,
    request_id: Option<u64>,
    initialize: bool,
    weak: &Weak<HttpSession>,
    inbound: &mpsc::Sender<Result<Vec<u8>, McpError>>,
) -> Result<bool, McpError> {
    let header = response.header("mcp-session-id").map(str::to_owned);
    if !event_stream {
        let bytes = response
            .bytes(MAX_FRAME_BYTES)
            .await
            .map_err(|error| match error {
                super::http_exchange::BodyRead::TooLarge => {
                    McpError::TooLarge("an HTTP response body")
                }
                super::http_exchange::BodyRead::Closed => McpError::Unconfirmed,
            })?;
        let accepted = if initialize {
            weak.upgrade().ok_or(McpError::Closed)?.note_response(
                &bytes,
                Some("initialize"),
                request_id,
                header.as_deref(),
            )?
        } else {
            false
        };
        let matched = response_in(&bytes, request_id);
        if request_id != Some(0) || !initialize {
            inbound
                .send(Ok(bytes))
                .await
                .map_err(|_| McpError::ServerGone)?;
        }
        completed_body(request_id, matched)?;
        return Ok(accepted);
    }
    let mut stream = match response.body {
        super::http_exchange::HttpBody::Stream(stream) => stream,
        super::http_exchange::HttpBody::Buffered(mut bytes) => {
            bytes.extend_from_slice(b"\n\n");
            Box::new(BufferedChunks(Some(bytes))) as Box<dyn super::http_exchange::HttpChunks>
        }
    };
    let mut parser = SseParser::bounded();
    loop {
        let Some(chunk) = stream.next().await.map_err(|_| McpError::Unconfirmed)? else {
            completed_body(request_id, false)?;
            return Ok(false);
        };
        let mut chunk = chunk.as_slice();
        loop {
            let event = parser.next_event(&mut chunk).map_err(|error| match error {
                SseError::TooLarge => McpError::TooLarge("an SSE event"),
                SseError::Utf8 => McpError::Malformed("an SSE event is not UTF-8".into()),
            })?;
            let Some(event) = event else {
                break;
            };
            if event.data.is_empty() {
                continue;
            }
            let matched = response_in(event.data.as_bytes(), request_id);
            let accepted = if initialize {
                weak.upgrade().ok_or(McpError::Closed)?.note_response(
                    event.data.as_bytes(),
                    Some("initialize"),
                    request_id,
                    header.as_deref(),
                )?
            } else {
                false
            };
            if request_id != Some(0) || !initialize || !matched {
                inbound
                    .send(Ok(event.data.into_bytes()))
                    .await
                    .map_err(|_| McpError::ServerGone)?;
            }
            if matched {
                return Ok(accepted);
            }
        }
    }
}

struct BufferedChunks(Option<Vec<u8>>);
#[async_trait::async_trait]
impl super::http_exchange::HttpChunks for BufferedChunks {
    async fn next(&mut self) -> Result<Option<Vec<u8>>, super::http_exchange::HttpFailure> {
        Ok(self.0.take())
    }
}

/// Both codecs settle unanswered correlated bodies at completion; uncorrelated
/// bodies have no request to fail (ADR 392 P3 and J2).
fn completed_body(id: Option<u64>, matched: bool) -> Result<(), McpError> {
    if id.is_some() && !matched {
        Err(McpError::Unconfirmed)
    } else {
        Ok(())
    }
}

/// Parsing does not alter the single terminal/correlation rule below.
fn response_in(bytes: &[u8], id: Option<u64>) -> bool {
    id.is_some_and(|id| {
        serde_json::from_slice::<Value>(bytes)
            .ok()
            .is_some_and(|message| is_response_to(&message, id))
    })
}

/// Correlation and terminal shape shared by POST retirement and initialization.
fn is_response_to(message: &Value, id: u64) -> bool {
    message.get("method").is_none()
        && message.get("id").and_then(Value::as_u64) == Some(id)
        && (message.get("result").is_some() ^ message.get("error").is_some())
}

enum Modern {
    Response(HttpResponse),
    Accepted,
    Legacy,
}

/// HTTP 5xx. `initialize` still failed before a session existed. A later
/// request was accepted by the server, so its effect is unconfirmed.
fn server_error(initialize: bool, id: Option<u64>) -> SendOutcome {
    if initialize {
        SendOutcome::End(McpError::Unreachable)
    } else {
        SendOutcome::FailCall {
            id,
            error: McpError::Unconfirmed,
        }
    }
}

/// `initialize` failed before a session existed. Any other request may
/// already have been received: headers were lost, or the header deadline
/// fired after the bytes were sent.
fn lost_exchange(body: &[u8]) -> McpError {
    if body_is_initialize(body) {
        McpError::Unreachable
    } else {
        McpError::Unconfirmed
    }
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
