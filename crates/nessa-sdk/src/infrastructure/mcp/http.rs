//! Streamable HTTP, and the 2024-11-05 HTTP+SSE fallback that follows only an
//! initial POST status of 400, 404, or 405
//! (`docs/adr/todo/392-remote-mcp-servers.md`).
//!
//! ```text
//! Connection writer ──dispatch(one JSON-RPC frame)──▶ HttpSession
//!        ▲                                              ├── POST modern, or legacy endpoint
//!        │                                              ├── HttpExchange
//!        │                                              ├── bounded owned POST readers (abort/join on close)
//!        └── reader ◀── JSON + immutable reply context ─┴── optional GET stream
//! ```
//!
//! Arrows are messages. One [`HttpSession`] is one local opening. It keeps the
//! upstream `Mcp-Session-Id` when the server sends one, refuses an id another
//! opening of the same URL already holds, and DELETE that id at most once on
//! close. A 404 on a session-bound request fails that request with
//! [`McpError::SessionExpired`] and starts one fresh `initialize` without the
//! old id under its registered startup owner. Early peer answers retain a bounded
//! provisional binding without publishing validated readiness (ADR 392 J17–J19).
//! Initial and recovery POSTs share authorization/status policy (J20).
//! Ordinary frames and initialized
//! use the connection writer; the failed request is not replayed.
//! Legacy mode records that DELETE
//! does not apply.
//!
//! Streamed POST bodies (JSON and SSE, including initialize) are session-owned
//! and separately bounded. An SSE request reader
//! forwards notices/requests until its matching result/error, then drops the body
//! even if the peer leaves it open (ADR 392 P1–P14/J1–J20,
//! `tests::post_streams` and `tests::http_progress`).
//!
//! Dropping the session aborts its GET and POST streams and asks for the same single
//! DELETE. The DELETE itself is best-effort and bounded by the exchange. A panic
//! in that DELETE still releases the claimed id and signals [`HttpSession::finished`].
#![deny(missing_docs)]

use super::authorization::{Bearer, RemoteAuthorization};
use super::connection::{Outgoing, OutgoingQueue};
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

#[derive(Debug)]
struct SessionBinding {
    id: Option<String>,
}

/// Immutable HTTP evidence accompanying a peer request's generated answer.
#[derive(Clone, Debug)]
pub(crate) struct ReplyContext {
    binding: Option<Arc<SessionBinding>>,
    version: Option<String>,
}

/// One received JSON frame, retaining transport evidence for a generated reply.
/// The connection owns JSON framing; HTTP owns the private binding evidence.
#[derive(Debug)]
pub(crate) struct HttpMessage {
    pub(crate) bytes: Vec<u8>,
    pub(crate) reply: Option<ReplyContext>,
}
impl std::ops::Deref for HttpMessage {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}
impl From<Vec<u8>> for HttpMessage {
    fn from(bytes: Vec<u8>) -> Self {
        Self { bytes, reply: None }
    }
}

struct Phase {
    /// One claimed binding, provisional until `open` records validated initialize.
    binding: Option<Arc<SessionBinding>>,
    version: Option<String>,
    /// Set once the initial POST took the legacy path.
    legacy_post: Option<String>,
    recovery: Recovery,
    open: bool,
}

/// Identity publication and replacement call admission are distinct transitions.
#[derive(Clone, PartialEq, Eq)]
enum Recovery {
    Available,
    Initializing,
    ReadyForWriter,
    Completed,
    Failed(McpError),
}

impl Recovery {
    fn blocks_requests(&self) -> bool {
        match self {
            Self::Available | Self::Completed => false,
            Self::Initializing | Self::ReadyForWriter | Self::Failed(_) => true,
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
    inbound: mpsc::Sender<Result<HttpMessage, McpError>>,
    phase: Mutex<Phase>,
    closing: AtomicBool,
    delete_started: AtomicBool,
    readers: Mutex<ReaderTasks>,
    post_capacity: Arc<Semaphore>,
    /// Becomes true once close has nothing left to wait for, including a
    /// DELETE that does not apply.
    finished: watch::Sender<bool>,
    /// `None` while the HTTP writer task is running. `Some(true)` after it
    /// panics. `Some(false)` after it finishes or is cancelled. A dropped
    /// recovery completion waits for this instead of deciding an end cause.
    writer_settled: watch::Sender<Option<bool>>,
}

#[derive(Clone)]
struct HttpWriter {
    outgoing: OutgoingQueue,
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
    pub(crate) fn open(
        server: Uuid,
        url: RemoteMcpUrl,
        exchange: Arc<dyn HttpExchange>,
        authorization: Arc<dyn RemoteAuthorization>,
        claims: Arc<SessionClaims>,
    ) -> (Arc<Self>, mpsc::Receiver<Result<HttpMessage, McpError>>) {
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
                binding: None,
                version: None,
                legacy_post: None,
                recovery: Recovery::Available,
                open: false,
            }),
            closing: AtomicBool::new(false),
            delete_started: AtomicBool::new(false),
            readers: Mutex::new(ReaderTasks::default()),
            post_capacity: Arc::new(Semaphore::new(MAX_POST_STREAMS)),
            finished: watch::channel(false).0,
            writer_settled: watch::channel(None).0,
        });
        (session, incoming)
    }

    /// The configured server this session posts for. This is an identity, not
    /// a credential or a request body.
    pub(crate) fn server(&self) -> Uuid {
        self.server
    }

    /// Record the HTTP writer task's outcome once. Later calls keep the first.
    /// `panicked` is the writer's own result: a dropped recovery completion
    /// resolves [`McpError::ServerGone`] from it, and a clean finish or
    /// cancellation resolves [`McpError::Unconfirmed`].
    pub(crate) fn settle_writer(&self, panicked: bool) {
        self.writer_settled.send_if_modified(|current| {
            if current.is_some() {
                return false;
            }
            *current = Some(panicked);
            true
        });
    }

    /// The end cause already stored by [`Self::settle_writer`]. Panic is
    /// [`McpError::ServerGone`]. A clean finish, a cancellation, or a writer
    /// that has not settled is [`McpError::Unconfirmed`].
    pub(crate) fn writer_settlement_cause(&self) -> McpError {
        if matches!(*self.writer_settled.borrow(), Some(true)) {
            McpError::ServerGone
        } else {
            McpError::Unconfirmed
        }
    }

    /// The end cause for a recovery completion the writer dropped without
    /// sending. Waits until [`Self::settle_writer`] so this task does not
    /// decide ahead of the writer's outcome.
    async fn dropped_writer_sender(&self) -> McpError {
        let mut settled = self.writer_settled.subscribe();
        loop {
            if settled.borrow_and_update().is_some() {
                return self.writer_settlement_cause();
            }
            if settled.changed().await.is_err() {
                return McpError::Unconfirmed;
            }
        }
    }

    /// Resolves after shutdown joins owned readers and records its DELETE
    /// observation (or that DELETE does not apply). Subscribe before shutdown.
    pub fn finished(&self) -> watch::Receiver<bool> {
        self.finished.subscribe()
    }

    /// Stop owned GET/POST streams and DELETE a claimed modern session id once.
    /// [`Self::finished`] observes completion after reader joins and DELETE.
    /// A panic in that DELETE still releases the claim and signals finished.
    /// Legacy sessions and sessions with no id record that DELETE does not
    /// apply and do not send one.
    pub fn shutdown(&self) {
        let (readers, get_task): (Vec<_>, _) = {
            let mut readers = self.readers.lock().expect("http readers");
            self.closing.store(true, Ordering::SeqCst);
            if self.delete_started.swap(true, Ordering::SeqCst) {
                return;
            }
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
            .binding
            .as_ref()
            .filter(|_| !legacy)
            .and_then(|binding| binding.id.clone());
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
            // Drop runs on success, on a DELETE panic, and if this task is
            // aborted. The payload is not logged.
            let _release = ReleaseClaimOnDrop {
                claims,
                url,
                session_id: session_id.clone(),
                local,
                finished,
            };
            let mut headers = vec![
                ("Mcp-Session-Id".into(), session_id),
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
        });
    }

    /// Send one newline-framed JSON-RPC message.
    pub async fn dispatch(&self, frame: &[u8]) -> SendOutcome {
        self.dispatch_frame(frame, PostPurpose::Ordinary).await
    }

    pub(crate) async fn dispatch_reply(&self, frame: &[u8], context: ReplyContext) -> SendOutcome {
        self.dispatch_frame(frame, PostPurpose::PeerReply(context))
            .await
    }

    async fn dispatch_frame(&self, frame: &[u8], purpose: PostPurpose) -> SendOutcome {
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
        let peer_reply = matches!(purpose, PostPurpose::PeerReply(_));
        let id = (!peer_reply)
            .then(|| message.get("id").and_then(Value::as_u64))
            .flatten();
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
        match self.post_modern(bytes, purpose).await {
            Ok(Modern::Response(attempt)) => {
                self.deliver_body(attempt, method.as_deref(), id, permit)
                    .await
            }
            Ok(Modern::Accepted) => SendOutcome::Done,
            Ok(Modern::Legacy) => self.enter_legacy(bytes, id).await,
            Err(error) => {
                if peer_reply {
                    SendOutcome::End(error)
                } else {
                    self.terminal(id, error)
                }
            }
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
        attempt: PostResponse,
        method: Option<&str>,
        id: Option<u64>,
        permit: Option<OwnedSemaphorePermit>,
    ) -> SendOutcome {
        let PostResponse {
            response,
            context,
            origin,
            failure,
        } = attempt;
        let status = response.status;
        if status == 202 {
            return SendOutcome::Done;
        }
        if let Some(error) = failure {
            if matches!(
                context,
                ResponseContext::PeerReplyBound | ResponseContext::PeerReplyStateless
            ) {
                return SendOutcome::End(error);
            }
            if error == McpError::SessionExpired {
                return self.recover(id, permit);
            }
            return if status == 401 || method == Some("initialize") {
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
                    BodyPurpose::public(request_id, initialize),
                    &weak,
                    &inbound,
                    &origin,
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
            BodyPurpose::public(method.and(id), method == Some("initialize")),
            &self.weak,
            &self.inbound,
            &origin,
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

    async fn post_modern(&self, body: &[u8], purpose: PostPurpose) -> Result<Modern, McpError> {
        authorized_modern_post(
            &self.weak,
            self.exchange.clone(),
            self.authorization.clone(),
            self.server,
            body,
            purpose,
        )
        .await
    }

    /// Build evidence synchronously; external I/O holds no upgraded session.
    fn modern_request(
        &self,
        body: &[u8],
        bearer: Option<&Bearer>,
        purpose: &PostPurpose,
    ) -> Result<(HttpRequest, ResponseContext, ReplyContext), McpError> {
        let _readers = self.readers.lock().expect("http readers");
        if self.closing.load(Ordering::SeqCst) {
            return Err(McpError::Closed);
        }
        let phase = self.phase.lock().expect("http phase");
        let initialize = body_is_initialize(body);
        let binding = match purpose {
            PostPurpose::PeerReply(reply) => {
                check_reply_binding(&phase, reply)?;
                reply.binding.clone()
            }
            PostPurpose::RecoveryInitialize => {
                if let Recovery::Failed(error) = &phase.recovery {
                    return Err(error.clone());
                }
                None
            }
            PostPurpose::Ordinary => phase.binding.clone(),
        };
        let version = if initialize {
            None
        } else {
            match purpose {
                PostPurpose::PeerReply(reply) => reply.version.clone(),
                _ => phase.version.clone(),
            }
        };
        let mut context = if initialize {
            ResponseContext::Initialize {
                legacy_allowed: matches!(purpose, PostPurpose::Ordinary)
                    && !phase.open
                    && phase.binding.is_none()
                    && matches!(phase.recovery, Recovery::Available),
            }
        } else if matches!(purpose, PostPurpose::PeerReply(_)) {
            ResponseContext::PeerReplyStateless
        } else {
            ResponseContext::Stateless
        };
        let mut headers = vec![
            (
                "Accept".into(),
                "application/json, text/event-stream".into(),
            ),
            ("Content-Type".into(), "application/json".into()),
        ];
        if !initialize {
            if let Some(id) = binding.as_ref().and_then(|binding| binding.id.as_ref()) {
                headers.push(("Mcp-Session-Id".into(), id.clone()));
                context = if matches!(purpose, PostPurpose::PeerReply(_)) {
                    ResponseContext::PeerReplyBound
                } else {
                    ResponseContext::SessionBound
                };
            }
            if let Some(version) = &version {
                headers.push(("MCP-Protocol-Version".into(), version.clone()));
            }
        }
        if let Some(bearer) = bearer {
            headers.push(("Authorization".into(), format!("Bearer {}", bearer.token())));
        }
        Ok((
            HttpRequest {
                method: HttpMethod::Post,
                url: self.url.as_str().to_owned(),
                headers,
                body: body.to_vec(),
            },
            context,
            ReplyContext { binding, version },
        ))
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
            let task = self.runtime.spawn(read_legacy_stream(
                response,
                inbound,
                Some(endpoint_tx),
                None,
            ));
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

    fn publish_initialize(
        &self,
        result: &Value,
        session_header: Option<&str>,
    ) -> Result<(), McpError> {
        let mut readers = self.readers.lock().expect("http readers");
        if self.closing.load(Ordering::SeqCst) {
            return Err(McpError::Closed);
        }
        if self.phase.lock().expect("http phase").open {
            return Ok(());
        }
        wire::initialized(result)?;
        let version = result["protocolVersion"].as_str().map(str::to_owned);
        let mut phase = self.phase.lock().expect("http phase");
        let binding = self.claim_binding(&mut phase, session_header)?;
        phase.version = version;
        phase.open = true;
        drop(phase);
        self.start_get(binding, &mut readers);
        Ok(())
    }

    fn claim_binding(
        &self,
        phase: &mut Phase,
        header: Option<&str>,
    ) -> Result<Option<Arc<SessionBinding>>, McpError> {
        let id = bounded_session_id(header)?;
        if let Some(binding) = &phase.binding {
            return if binding.id.as_deref() == id {
                Ok(Some(binding.clone()))
            } else {
                Err(McpError::Malformed(
                    "initialize changed its response binding".into(),
                ))
            };
        }
        if let Some(id) = id {
            if !self.claims.claim(self.url.as_str(), id, self.local) {
                return Err(McpError::SessionCollision);
            }
        }
        let binding = Arc::new(SessionBinding {
            id: id.map(str::to_owned),
        });
        phase.binding = Some(binding.clone());
        Ok(Some(binding))
    }

    fn peer_reply_context(
        &self,
        purpose: BodyPurpose,
        header: Option<&str>,
        origin: &ReplyContext,
    ) -> Result<ReplyContext, McpError> {
        let _readers = self.readers.lock().expect("http readers");
        if self.closing.load(Ordering::SeqCst) {
            return Err(McpError::Closed);
        }
        let mut phase = self.phase.lock().expect("http phase");
        if let Recovery::Failed(error) = &phase.recovery {
            return Err(error.clone());
        }
        if matches!(
            purpose,
            BodyPurpose::Initialize(_) | BodyPurpose::RecoveryInitialize
        ) {
            Ok(ReplyContext {
                binding: self.claim_binding(&mut phase, header)?,
                version: phase.version.clone(),
            })
        } else {
            check_reply_binding(&phase, origin)?;
            Ok(origin.clone())
        }
    }

    fn start_get(&self, binding: Option<Arc<SessionBinding>>, readers: &mut ReaderTasks) {
        if self.phase.lock().expect("http phase").legacy_post.is_some() {
            return;
        }
        let Some(binding) = binding else {
            return;
        };
        let Some(session_id) = binding.id.clone() else {
            return;
        };
        let origin = ReplyContext {
            binding: Some(binding),
            version: self.phase.lock().expect("http phase").version.clone(),
        };
        let weak = self.weak.clone();
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
            read_legacy_stream(response, inbound, None, Some((weak, origin))).await;
        });
        readers.stop_get();
        readers.get.push(task);
    }

    pub(crate) fn set_writer(&self, writer: OutgoingQueue, clock: Arc<dyn Clock>) {
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
        if let Some(old) = phase.binding.take() {
            if let Some(id) = &old.id {
                self.claims.release(self.url.as_str(), id, self.local);
            }
        }
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
        let weak = self.weak.clone();
        let inbound = self.inbound.clone();
        let deadline = clock.now() + super::servers::INITIALIZE_TIMEOUT;
        let (registered, registration) = oneshot::channel();
        let task = self.runtime.spawn(async move {
            if registration.await.is_err() {
                return;
            }
            let startup = async {
                let body = serde_json::to_vec(&json!({
                    "jsonrpc": "2.0", "id": 0, "method": "initialize",
                    "params": wire::initialize_params(),
                }))
                .unwrap_or_default();
                let attempt = match authorized_modern_post(
                    &weak,
                    exchange,
                    authorization,
                    server,
                    &body,
                    PostPurpose::RecoveryInitialize,
                )
                .await?
                {
                    Modern::Response(attempt) => attempt,
                    Modern::Accepted => return Err(McpError::SessionExpired),
                    Modern::Legacy => {
                        return Err(McpError::Malformed(
                            "recovery initialize selected legacy transport".into(),
                        ))
                    }
                };
                if let Some(error) = attempt.failure {
                    return Err(error);
                }
                let response = attempt.response;
                let origin = attempt.origin;
                let content_type = response.header("content-type");
                let event_stream = sse::is_event_stream(content_type);
                if !event_stream && content_type.is_some() && !sse::is_json(content_type) {
                    return Err(McpError::SessionExpired);
                }
                if !consume_body(
                    response,
                    event_stream,
                    BodyPurpose::RecoveryInitialize,
                    &weak,
                    &inbound,
                    &origin,
                )
                .await
                .map_err(|error| match error {
                    McpError::Unconfirmed => McpError::SessionExpired,
                    other => other,
                })? {
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
                    .map_err(|_| McpError::Unconfirmed)?;
                match completion.await {
                    Ok(result) => result,
                    // The writer owned this sender. Its drop is not itself
                    // Unconfirmed: the writer's settlement decides.
                    Err(_) => {
                        let session = weak.upgrade().ok_or(McpError::Closed)?;
                        Err(session.dropped_writer_sender().await)
                    }
                }
            };
            let result = within(&*clock, deadline, startup)
                .await
                .unwrap_or(Err(McpError::Timeout));
            if let Err(error) = result {
                let result = weak
                    .upgrade()
                    .map_or(Err(error.clone()), |session| session.fail_recovery(error));
                if let Err(error) = result {
                    let _ = inbound.send(Err(error)).await;
                }
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
        completed: oneshot::Sender<Result<(), McpError>>,
    ) -> SendOutcome {
        let clock = self
            .writer
            .lock()
            .expect("http writer")
            .as_ref()
            .expect("installed HTTP writer")
            .clock
            .clone();
        if let Err(error) = self.check_handoff(&*clock, deadline, &completed) {
            return self.complete_recovery(&*clock, deadline, completed, Err(error));
        }
        let body =
            serde_json::to_vec(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .unwrap_or_default();
        let sending = async {
            match self.post_modern(&body, PostPurpose::Ordinary).await {
                Ok(Modern::Accepted) => SendOutcome::Done,
                Ok(Modern::Response(attempt)) => {
                    SendOutcome::End(attempt.failure.unwrap_or_else(|| {
                        McpError::Malformed("recovery initialized was not accepted".into())
                    }))
                }
                Ok(Modern::Legacy) => SendOutcome::End(McpError::Malformed(
                    "recovery initialized selected legacy transport".into(),
                )),
                Err(error) => SendOutcome::End(error),
            }
        };
        let outcome = within(&*clock, deadline, sending)
            .await
            .unwrap_or(SendOutcome::End(McpError::Timeout));
        let result = match outcome {
            SendOutcome::Done => Ok(()),
            SendOutcome::End(error) | SendOutcome::FailCall { error, .. } => Err(error),
        };
        self.complete_recovery(&*clock, deadline, completed, result)
    }

    /// Startup failure cannot overwrite the writer's committed completion.
    fn fail_recovery(&self, error: McpError) -> Result<(), McpError> {
        let _readers = self.readers.lock().expect("http readers");
        recovery_failure(&mut self.phase.lock().expect("http phase"), error)
    }

    fn check_handoff(
        &self,
        clock: &dyn Clock,
        deadline: ClockInstant,
        completed: &oneshot::Sender<Result<(), McpError>>,
    ) -> Result<(), McpError> {
        let _readers = self.readers.lock().expect("http readers");
        let phase = self.phase.lock().expect("http phase");
        validate_handoff(
            &phase,
            self.closing.load(Ordering::SeqCst),
            clock.now(),
            deadline,
        )?;
        if completed.is_closed() {
            Err(McpError::Unconfirmed)
        } else {
            Ok(())
        }
    }

    /// Delivery and admission have one terminal owner and no asynchronous gap.
    fn complete_recovery(
        &self,
        clock: &dyn Clock,
        deadline: ClockInstant,
        completed: oneshot::Sender<Result<(), McpError>>,
        result: Result<(), McpError>,
    ) -> SendOutcome {
        let _readers = self.readers.lock().expect("http readers");
        let mut phase = self.phase.lock().expect("http phase");
        let result = validate_handoff(
            &phase,
            self.closing.load(Ordering::SeqCst),
            clock.now(),
            deadline,
        )
        .and(result);
        let error = match result {
            Ok(()) => {
                if completed.send(Ok(())).is_ok() {
                    phase.recovery = Recovery::Completed;
                    return SendOutcome::Done;
                }
                if clock.now() >= deadline {
                    McpError::Timeout
                } else {
                    McpError::Unconfirmed
                }
            }
            Err(error) => {
                let _ = completed.send(Err(error.clone()));
                error
            }
        };
        match recovery_failure(&mut phase, error) {
            Ok(()) => SendOutcome::Done,
            Err(error) => SendOutcome::End(error),
        }
    }
}

impl Drop for HttpSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Release one claimed session id and signal shutdown finished.
/// Constructed after reader joins and before DELETE, so both happen when
/// DELETE returns, panics, or the cleanup task is dropped.
struct ReleaseClaimOnDrop {
    claims: Arc<SessionClaims>,
    url: String,
    session_id: String,
    local: u64,
    finished: watch::Sender<bool>,
}

impl Drop for ReleaseClaimOnDrop {
    fn drop(&mut self) {
        self.claims.release(&self.url, &self.session_id, self.local);
        self.finished.send_replace(true);
    }
}

/// The task owns the body; weak upgrades end before asynchronous delivery.
async fn consume_body(
    response: HttpResponse,
    event_stream: bool,
    purpose: BodyPurpose,
    weak: &Weak<HttpSession>,
    inbound: &mpsc::Sender<Result<HttpMessage, McpError>>,
    origin: &ReplyContext,
) -> Result<bool, McpError> {
    let header = if matches!(
        purpose,
        BodyPurpose::Initialize(_) | BodyPurpose::RecoveryInitialize
    ) {
        bounded_session_id(response.header("mcp-session-id"))?.map(str::to_owned)
    } else {
        None
    };
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
        return match receive_message(bytes, purpose, header.as_deref(), weak, inbound, origin)
            .await?
        {
            MessageOutcome::Terminal { initialized } => Ok(initialized),
            MessageOutcome::Continue => {
                completed_body(purpose.request_id(), false)?;
                Ok(false)
            }
        };
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
            completed_body(purpose.request_id(), false)?;
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
            if let MessageOutcome::Terminal { initialized } = receive_message(
                event.data.into_bytes(),
                purpose,
                header.as_deref(),
                weak,
                inbound,
                origin,
            )
            .await?
            {
                return Ok(initialized);
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

/// Framing hands every message to this policy, regardless of codec or storage.
#[derive(Clone, Copy)]
enum BodyPurpose {
    Protocol(Option<u64>),
    Initialize(Option<u64>),
    RecoveryInitialize,
}
impl BodyPurpose {
    fn public(id: Option<u64>, initialize: bool) -> Self {
        if initialize {
            Self::Initialize(id)
        } else {
            Self::Protocol(id)
        }
    }
    fn request_id(self) -> Option<u64> {
        match self {
            Self::Protocol(id) | Self::Initialize(id) => id,
            Self::RecoveryInitialize => Some(0),
        }
    }
}
enum MessageOutcome {
    Continue,
    Terminal { initialized: bool },
}
async fn receive_message(
    bytes: Vec<u8>,
    purpose: BodyPurpose,
    header: Option<&str>,
    weak: &Weak<HttpSession>,
    inbound: &mpsc::Sender<Result<HttpMessage, McpError>>,
    origin: &ReplyContext,
) -> Result<MessageOutcome, McpError> {
    let message: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let matched = purpose
        .request_id()
        .is_some_and(|id| is_response_to(&message, id));
    let initialized =
        matched && !matches!(purpose, BodyPurpose::Protocol(_)) && message.get("result").is_some();
    if initialized {
        weak.upgrade()
            .ok_or(McpError::Closed)?
            .publish_initialize(&message["result"], header)?;
    }
    if !matched || !matches!(purpose, BodyPurpose::RecoveryInitialize) {
        let reply = if message.get("method").and_then(Value::as_str).is_some()
            && message.get("id").is_some_and(|id| !id.is_null())
        {
            Some(
                weak.upgrade()
                    .ok_or(McpError::Closed)?
                    .peer_reply_context(purpose, header, origin)?,
            )
        } else {
            None
        };
        inbound
            .send(Ok(HttpMessage { bytes, reply }))
            .await
            .map_err(|_| McpError::ServerGone)?;
    }
    Ok(if matched {
        MessageOutcome::Terminal { initialized }
    } else {
        MessageOutcome::Continue
    })
}

/// Correlation and terminal shape shared by POST retirement and initialization.
fn is_response_to(message: &Value, id: u64) -> bool {
    message.get("method").is_none()
        && message.get("id").and_then(Value::as_u64) == Some(id)
        && (message.get("result").is_some() ^ message.get("error").is_some())
}

/// The response retains the context of its own emitted request headers.
struct PostResponse {
    response: HttpResponse,
    context: ResponseContext,
    origin: ReplyContext,
    failure: Option<McpError>,
}

enum Modern {
    Response(PostResponse),
    Accepted,
    Legacy,
}

#[derive(Clone, Copy)]
enum ResponseContext {
    Initialize { legacy_allowed: bool },
    SessionBound,
    PeerReplyBound,
    PeerReplyStateless,
    Stateless,
}

/// Typed HTTP failures are independent of ordinary or recovery dispatch.
fn response_failure(status: u16, context: ResponseContext) -> Option<McpError> {
    match status {
        401 => Some(McpError::Unauthorized),
        404 if matches!(
            context,
            ResponseContext::SessionBound | ResponseContext::PeerReplyBound
        ) =>
        {
            Some(McpError::SessionExpired)
        }
        500.. => Some(if matches!(context, ResponseContext::Initialize { .. }) {
            McpError::Unreachable
        } else {
            McpError::Unconfirmed
        }),
        200..=299 => None,
        _ => Some(McpError::Malformed(format!("HTTP {status}"))),
    }
}

/// Legacy dispatch uses the same typed 5xx classifier.
fn server_error(initialize: bool, id: Option<u64>) -> SendOutcome {
    let error = response_failure(
        500,
        if initialize {
            ResponseContext::Initialize {
                legacy_allowed: false,
            }
        } else {
            ResponseContext::Stateless
        },
    )
    .expect("5xx failure");
    if initialize {
        SendOutcome::End(error)
    } else {
        SendOutcome::FailCall { id, error }
    }
}

/// Both terminal contenders use this transition while holding the registry fence.
fn recovery_failure(phase: &mut Phase, error: McpError) -> Result<(), McpError> {
    match &phase.recovery {
        Recovery::Completed => Ok(()),
        Recovery::Failed(first) => Err(first.clone()),
        _ => {
            phase.recovery = Recovery::Failed(error.clone());
            Err(error)
        }
    }
}

fn validate_handoff(
    phase: &Phase,
    closing: bool,
    now: ClockInstant,
    deadline: ClockInstant,
) -> Result<(), McpError> {
    match &phase.recovery {
        Recovery::Failed(first) => Err(first.clone()),
        _ if closing => Err(McpError::Closed),
        _ if now >= deadline => Err(McpError::Timeout),
        Recovery::ReadyForWriter => Ok(()),
        _ => Err(McpError::Unconfirmed),
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
    inbound: mpsc::Sender<Result<HttpMessage, McpError>>,
    mut endpoint: Option<tokio::sync::oneshot::Sender<String>>,
    modern: Option<(Weak<HttpSession>, ReplyContext)>,
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
                        if let Err(error) = forward_stream_message(
                            event.data.into_bytes(),
                            &inbound,
                            modern.as_ref(),
                        )
                        .await
                        {
                            let _ = inbound.send(Err(error)).await;
                            return;
                        }
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
            if let Err(error) =
                forward_stream_message(event.data.into_bytes(), &inbound, modern.as_ref()).await
            {
                let _ = inbound.send(Err(error)).await;
                return;
            }
        }
    }
}

fn check_reply_binding(phase: &Phase, reply: &ReplyContext) -> Result<(), McpError> {
    if let Recovery::Failed(error) = &phase.recovery {
        return Err(error.clone());
    }
    match (&phase.binding, &reply.binding) {
        (None, None) => Ok(()),
        (Some(current), Some(captured)) if Arc::ptr_eq(current, captured) => Ok(()),
        _ => Err(McpError::SessionExpired),
    }
}

#[derive(Clone)]
enum PostPurpose {
    Ordinary,
    PeerReply(ReplyContext),
    RecoveryInitialize,
}

async fn authorized_modern_post(
    weak: &Weak<HttpSession>,
    exchange: Arc<dyn HttpExchange>,
    authorization: Arc<dyn RemoteAuthorization>,
    server: Uuid,
    body: &[u8],
    purpose: PostPurpose,
) -> Result<Modern, McpError> {
    let mut bearer = authorization.bearer(server).await?;
    for attempt_index in 0..2 {
        let (request, context, origin) = {
            let session = weak.upgrade().ok_or(McpError::Closed)?;
            session.modern_request(body, bearer.as_ref(), &purpose)?
        };
        let response = exchange
            .exchange(request)
            .await
            .map_err(|_| lost_exchange(body))?;
        if response.status == 401 && attempt_index == 0 {
            let challenge = response
                .header("www-authenticate")
                .unwrap_or("Bearer")
                .to_owned();
            drop(response);
            bearer = authorization.rejected(server, &challenge).await?;
            if bearer.is_some() {
                continue;
            }
            return Err(McpError::Unauthorized);
        }
        if response.status == 403 {
            let challenge = response
                .header("www-authenticate")
                .unwrap_or("Bearer")
                .to_owned();
            drop(response);
            authorization.insufficient_scope(server, &challenge).await;
            return Err(McpError::InsufficientScope);
        }
        if matches!(response.status, 400 | 404 | 405)
            && matches!(
                context,
                ResponseContext::Initialize {
                    legacy_allowed: true
                }
            )
        {
            return Ok(Modern::Legacy);
        }
        if response.status == 202 {
            return Ok(Modern::Accepted);
        }
        let failure = response_failure(response.status, context);
        return Ok(Modern::Response(PostResponse {
            response,
            context,
            origin,
            failure,
        }));
    }
    unreachable!("the second authorization attempt returns its classified response")
}

/// The single initialize identity bound, applied before copying authoritative headers.
fn bounded_session_id(header: Option<&str>) -> Result<Option<&str>, McpError> {
    let id = header.filter(|id| !id.is_empty());
    if id.is_some_and(|id| id.len() > 1024) {
        Err(McpError::TooLarge("Mcp-Session-Id"))
    } else {
        Ok(id)
    }
}

async fn forward_stream_message(
    bytes: Vec<u8>,
    inbound: &mpsc::Sender<Result<HttpMessage, McpError>>,
    modern: Option<&(Weak<HttpSession>, ReplyContext)>,
) -> Result<(), McpError> {
    if let Some((weak, origin)) = modern {
        receive_message(
            bytes,
            BodyPurpose::Protocol(None),
            None,
            weak,
            inbound,
            origin,
        )
        .await?;
        Ok(())
    } else {
        inbound
            .send(Ok(bytes.into()))
            .await
            .map_err(|_| McpError::ServerGone)
    }
}

#[cfg(all(test, unix))]
#[path = "../../../tests/infrastructure/mcp/http_shutdown.rs"]
mod shutdown_tests;
