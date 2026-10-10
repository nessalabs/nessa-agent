//! One JSON-RPC connection to one MCP server, shared by everything that talks
//! to it: the client's own requests and every stand-in's forwarded ones.
//!
//! ```text
//! call / HTTP controls ──bounded FIFO (ordinary permits, HTTP reserve)──▶ writer task
//!                                                                    └──▶ stdin / POST
//!   ▲                                  │
//!   └── pending[id] ◀── reader task ◀──┘ server stdout
//!                         ├── server request ──▶ answered here (ping, else -32601)
//!                         └── */list_changed ──▶ notices (broadcast)
//! ```
//!
//! Arrows are frames. Ids are this connection's own, so callers with ids of
//! their own (stand-ins) cannot collide. The connection ends once, with one
//! cause, and every pending call gets that cause; a call admitted after the
//! end gets it too, because admission and the end share one lock. A panicked
//! HTTP writer catches that panic on every await it performs, discards the
//! payload, and settles the writer as panicked. It then waits until its clock
//! says the current moment has arrived and asks the same owner for that
//! settlement: panic resolves [`McpError::ServerGone`], and a clean finish or
//! cancellation resolves [`McpError::Unconfirmed`]. The runtime clock has
//! already reached that moment. A recovery completion the writer drops waits
//! for the same settlement. The watch still joins a panic that escapes the
//! writer and asks for [`McpError::ServerGone`]. The tracing record is a fixed
//! marker and the server id. The default panic hook can still print the panic
//! message; request and response debug output does not include header values
//! or body bytes. Drop and close abort that writer before its watch, so an
//! exchange still in flight cannot retain the session.
use super::framing::{self, FrameEnd, Frames, MAX_FRAME_BYTES};
use super::http::{HttpSession, SendOutcome};
use super::McpError;
use crate::infrastructure::clock::{within, Clock, ClockInstant};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    future::Future,
    panic::AssertUnwindSafe,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
    task::Poll,
    time::Duration,
};
#[cfg(all(test, unix))]
use tokio::sync::mpsc::error::TryRecvError;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{
        broadcast,
        mpsc::{
            self,
            error::{SendError, TrySendError},
            Receiver, Sender,
        },
        oneshot, watch, OwnedSemaphorePermit, Semaphore,
    },
    task::{AbortHandle, JoinHandle},
};

/// The most calls waiting on one server at a time.
pub(crate) const MAX_IN_FLIGHT: usize = 256;
/// Frames queued for the server's stdin before callers wait.
pub(super) const OUTGOING_FRAMES: usize = 64;
/// Extra physical room for HTTP controls under ordinary frame pressure.
pub(super) const HTTP_CONTROL_RESERVE: usize = 1;
/// Change notices held for a stand-in that has not read them yet. They are
/// idempotent, so one that lags misses only repeats.
const NOTICES: usize = 16;
/// The most bytes of a server's error message kept.
const MAX_ERROR_MESSAGE_BYTES: usize = 1024;

/// What the server answered: a result, or a JSON-RPC error object, verbatim.
pub(crate) type Reply = Result<Value, Value>;

struct State {
    pending: HashMap<u64, oneshot::Sender<Result<Reply, McpError>>>,
    ended: Option<McpError>,
}

struct Shared {
    state: Mutex<State>,
    ended: watch::Sender<Option<McpError>>,
    notices: broadcast::Sender<Arc<Value>>,
    next_id: AtomicU64,
}
impl Shared {
    /// End the connection with `cause`, once. Later causes are not recorded:
    /// the first is what happened.
    fn end(&self, cause: McpError) {
        let pending = {
            let mut state = self.state.lock().expect("connection state");
            if state.ended.is_some() {
                return;
            }
            state.ended = Some(cause.clone());
            std::mem::take(&mut state.pending)
        };
        self.ended.send_replace(Some(cause.clone()));
        for (_, waiter) in pending {
            let _ = waiter.send(Err(cause.clone()));
        }
    }

    /// Fail one admitted call and leave the connection open. The first end
    /// cause still wins if the connection has already ended.
    fn fail_one(&self, id: u64, error: McpError) {
        let mut state = self.state.lock().expect("connection state");
        if state.ended.is_some() {
            return;
        }
        if let Some(waiter) = state.pending.remove(&id) {
            drop(state);
            let _ = waiter.send(Err(error));
        }
    }
}

/// Frames and HTTP controls share the bounded FIFO owned by [`OutgoingQueue`].
pub(crate) enum Outgoing {
    Frame(Vec<u8>),
    PeerReply {
        frame: Vec<u8>,
        context: super::http::ReplyContext,
    },
    RecoveryReady {
        deadline: ClockInstant,
        completed: oneshot::Sender<Result<(), McpError>>,
    },
}

/// One FIFO admission owner: ordinary frames retain permits until dequeue;
/// HTTP controls can use reserved or otherwise free physical capacity.
#[derive(Clone)]
pub(crate) struct OutgoingQueue {
    sender: Sender<Queued>,
    ordinary: Arc<Semaphore>,
}

struct Queued {
    message: Outgoing,
    _ordinary: Option<OwnedSemaphorePermit>,
}

/// The writer releases queue capacity before it starts dispatching a frame.
pub(crate) struct OutgoingFrames {
    receiver: Receiver<Queued>,
}

impl Outgoing {
    fn is_ordinary(&self) -> bool {
        match self {
            Self::Frame(_) => true,
            Self::PeerReply { .. } | Self::RecoveryReady { .. } => false,
        }
    }
}

impl OutgoingQueue {
    /// Derive the physical bound from ordinary admission and its control reserve.
    pub(crate) fn new(ordinary: usize, reserve: usize) -> (Self, OutgoingFrames) {
        let (sender, receiver) = mpsc::channel(ordinary + reserve);
        (
            Self {
                sender,
                ordinary: Arc::new(Semaphore::new(ordinary)),
            },
            OutgoingFrames { receiver },
        )
    }

    /// Wait for bounded admission; cancellation releases any acquired permit.
    pub(crate) async fn send(&self, message: Outgoing) -> Result<(), SendError<Outgoing>> {
        let permit = if message.is_ordinary() {
            match self.ordinary.clone().acquire_owned().await {
                Ok(permit) => Some(permit),
                Err(_) => return Err(SendError(message)),
            }
        } else {
            None
        };
        self.sender
            .send(Queued {
                message,
                _ordinary: permit,
            })
            .await
            .map_err(|error| SendError(error.0.message))
    }

    /// A peer-answer reader and a dropped caller cannot wait for queue capacity.
    pub(crate) fn try_send(&self, message: Outgoing) -> Result<(), TrySendError<Outgoing>> {
        if self.sender.is_closed() {
            return Err(TrySendError::Closed(message));
        }
        let permit = if message.is_ordinary() {
            match self.ordinary.clone().try_acquire_owned() {
                Ok(permit) => Some(permit),
                Err(_) => return Err(TrySendError::Full(message)),
            }
        } else {
            None
        };
        self.sender
            .try_send(Queued {
                message,
                _ordinary: permit,
            })
            .map_err(|error| match error {
                TrySendError::Full(queued) => TrySendError::Full(queued.message),
                TrySendError::Closed(queued) => TrySendError::Closed(queued.message),
            })
    }
}

impl OutgoingFrames {
    pub(crate) async fn recv(&mut self) -> Option<Outgoing> {
        self.receiver.recv().await.map(|queued| queued.message)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn try_recv(&mut self) -> Result<Outgoing, TryRecvError> {
        self.receiver.try_recv().map(|queued| queued.message)
    }
}

/// A connection to one MCP server. Dropping it stops its tasks, which closes
/// the server's stdin.
pub(crate) struct Connection {
    shared: Arc<Shared>,
    outgoing: OutgoingQueue,
    clock: Arc<dyn Clock>,
    /// For stdio, the writer task. For HTTP, the watch joined to [`Self::http_writer`].
    writer_watch: JoinHandle<()>,
    /// Abort handle of the HTTP writer the watch joins. `None` for stdio,
    /// where [`Self::writer_watch`] is the writer.
    http_writer: Option<AbortHandle>,
    reader: JoinHandle<()>,
    /// Set for a remote session, so close can DELETE its upstream id once.
    http: Option<Arc<HttpSession>>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.abort_writer();
        self.reader.abort();
    }
}
impl Connection {
    /// Start reading `input` and writing `output`, the server's stdout and stdin.
    pub(crate) fn open(
        input: impl AsyncRead + Unpin + Send + 'static,
        mut output: impl AsyncWrite + Unpin + Send + 'static,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                pending: HashMap::new(),
                ended: None,
            }),
            ended: watch::channel(None).0,
            notices: broadcast::channel(NOTICES).0,
            next_id: AtomicU64::new(1),
        });
        let (outgoing, mut frames) = OutgoingQueue::new(OUTGOING_FRAMES, 0);
        let writer_watch = tokio::spawn({
            let shared = shared.clone();
            async move {
                while let Some(frame) = frames.recv().await {
                    let Outgoing::Frame(frame) = frame else {
                        continue;
                    };
                    if framing::write(&mut output, &frame).await.is_err() {
                        shared.end(McpError::ServerGone);
                        return;
                    }
                }
            }
        });
        let reader = tokio::spawn(read(
            Frames::new(input, MAX_FRAME_BYTES),
            shared.clone(),
            outgoing.clone(),
        ));
        Self {
            shared,
            outgoing,
            clock,
            writer_watch,
            http_writer: None,
            reader,
            http: None,
        }
    }

    /// A connection whose frames are HTTP calls on `session`. The reader
    /// consumes the JSON messages `session` pushes, including server requests
    /// and notices. Dropping it aborts the tasks; [`Self::close`] also shuts
    /// the HTTP session down so its DELETE runs once.
    pub(crate) fn open_http(
        session: Arc<HttpSession>,
        incoming: mpsc::Receiver<Result<super::http::HttpMessage, McpError>>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                pending: HashMap::new(),
                ended: None,
            }),
            ended: watch::channel(None).0,
            notices: broadcast::channel(NOTICES).0,
            next_id: AtomicU64::new(1),
        });
        let (outgoing, mut frames) = OutgoingQueue::new(OUTGOING_FRAMES, HTTP_CONTROL_RESERVE);
        session.set_writer(outgoing.clone(), clock.clone());
        let writing = tokio::spawn({
            let shared = shared.clone();
            let session = session.clone();
            let clock = clock.clone();
            async move {
                // A caught panic settles itself before this drops. The first
                // settlement sticks, so this drop does not overwrite it.
                let _settle = SettleWriter(&session);
                // The loop runs on its own task. Awaiting a future in this
                // task would keep that future, and a queued completion
                // sender, alive until this task ends. The child task's
                // completion drops them before the join handle resolves.
                let session_loop = session.clone();
                let shared_loop = shared.clone();
                let mut writer_loop = AbortOnDrop(tokio::spawn(catch_writer_panic(async move {
                    // Every await the writer performs is inside this catch:
                    // the queue recv, dispatch, dispatch_reply, and
                    // finish_recovery (including the initialized POST).
                    while let Some(frame) = frames.recv().await {
                        let outcome = match frame {
                            Outgoing::Frame(frame) => session_loop.dispatch(&frame).await,
                            Outgoing::PeerReply { frame, context } => {
                                session_loop.dispatch_reply(&frame, context).await
                            }
                            Outgoing::RecoveryReady {
                                deadline,
                                completed,
                            } => session_loop.finish_recovery(deadline, completed).await,
                        };
                        match outcome {
                            SendOutcome::Done => {}
                            SendOutcome::FailCall { id, error } => {
                                if let Some(id) = id {
                                    shared_loop.fail_one(id, error);
                                }
                            }
                            SendOutcome::End(error) => {
                                shutdown_and_end(&session_loop, &shared_loop, error);
                                return;
                            }
                        }
                    }
                })));
                let panicked = match (&mut writer_loop.0).await {
                    Ok(Ok(())) => false,
                    Ok(Err(())) => true,
                    Err(error) => error.is_panic(),
                };
                if !panicked {
                    return;
                }
                // Senders owned by the child are already dropped. Settlement
                // is visible before the end is recorded. An already-due sleep
                // returns immediately on the runtime clock. A test clock can
                // hold it so recovery publishes that settlement before this
                // record.
                session.settle_writer(true);
                let now = clock.now();
                clock.sleep_until(now).await;
                log_writer_panic(&session);
                shutdown_and_end(&session, &shared, session.writer_settlement_cause());
            }
        });
        let http_writer = writing.abort_handle();
        let writer_watch =
            tokio::spawn(watch_http_writer(writing, shared.clone(), session.clone()));
        let reader = tokio::spawn(read_messages(
            incoming,
            shared.clone(),
            outgoing.clone(),
            Arc::downgrade(&session),
        ));
        Self {
            shared,
            outgoing,
            clock,
            writer_watch,
            http_writer: Some(http_writer),
            reader,
            http: Some(session),
        }
    }

    /// Resolves when a remote session's close has joined readers and finished DELETE, or
    /// immediately for stdio.
    pub(crate) fn http_finished(&self) -> Option<watch::Receiver<bool>> {
        self.http.as_ref().map(|session| session.finished())
    }

    /// Send `method` and wait for the answer, with no deadline of its own.
    /// Dropping the returned future before the answer cancels the request
    /// upstream (`notifications/cancelled`), when it had been sent.
    ///
    /// # Errors
    ///
    /// [`McpError::Busy`] with [`MAX_IN_FLIGHT`] calls already waiting,
    /// [`McpError::TooLarge`] for a request past the frame bound, and the
    /// connection's end cause once it has ended.
    pub(crate) async fn call(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Reply, McpError> {
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let mut request = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if let Some(params) = params {
            request["params"] = params;
        }
        let frame = framing::encode(&request)?;
        let (answer, answered) = oneshot::channel();
        {
            let mut state = self.shared.state.lock().expect("connection state");
            if let Some(cause) = &state.ended {
                return Err(cause.clone());
            }
            if state.pending.len() >= MAX_IN_FLIGHT {
                return Err(McpError::Busy);
            }
            state.pending.insert(id, answer);
        }
        let mut guard = Pending {
            connection: self,
            id,
            sent: false,
        };
        self.send_frame(frame).await?;
        guard.sent = true;
        let reply = answered.await.unwrap_or(Err(McpError::ServerGone));
        guard.id = 0;
        reply
    }

    /// [`Self::call`] within `timeout` on the connection's clock, with a
    /// JSON-RPC error answer as [`McpError::Remote`].
    ///
    /// # Errors
    ///
    /// [`McpError::Timeout`] when no answer came in time (the request is
    /// cancelled upstream), [`McpError::Remote`] for an error answer, and
    /// what [`Self::call`] fails with.
    pub(crate) async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        let deadline = self.clock.now() + timeout;
        match within(&*self.clock, deadline, self.call(method, params)).await {
            None => Err(McpError::Timeout),
            Some(Ok(Ok(result))) => Ok(result),
            Some(Ok(Err(error))) => Err(remote(&error)),
            Some(Err(error)) => Err(error),
        }
    }

    /// Send a notification. Nothing answers it.
    pub(crate) async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), McpError> {
        let mut notification = json!({ "jsonrpc": "2.0", "method": method });
        if let Some(params) = params {
            notification["params"] = params;
        }
        let frame = framing::encode(&notification)?;
        self.send_frame(frame).await
    }

    /// Admission waits observe the existing end publication without draining the writer.
    async fn send_frame(&self, frame: Vec<u8>) -> Result<(), McpError> {
        let ended = self.ended();
        if let Some(cause) = self.end_cause() {
            return Err(cause);
        }
        tokio::select! {
            biased;
            cause = ended => Err(cause),
            sent = self.outgoing.send(Outgoing::Frame(frame)) => {
                sent.map_err(|_| self.end_cause().unwrap_or(McpError::ServerGone))
            }
        }
    }

    /// Why the connection ended, or `None` while it is open.
    pub(crate) fn end_cause(&self) -> Option<McpError> {
        self.shared
            .state
            .lock()
            .expect("connection state")
            .ended
            .clone()
    }

    /// Resolves with the end cause once the connection has ended.
    pub(crate) fn ended(&self) -> impl std::future::Future<Output = McpError> + Send + 'static {
        let mut ended = self.shared.ended.subscribe();
        async move {
            match ended.wait_for(Option::is_some).await {
                Ok(cause) => cause.clone().unwrap_or(McpError::ServerGone),
                Err(_) => McpError::ServerGone,
            }
        }
    }

    /// The server's `*/list_changed` notifications from now on, verbatim.
    pub(crate) fn notices(&self) -> broadcast::Receiver<Arc<Value>> {
        self.shared.notices.subscribe()
    }

    /// End the connection with `cause` and close the server's stdin. Calls
    /// waiting on it get `cause`.
    pub(crate) fn close(&self, cause: McpError) {
        if let Some(http) = &self.http {
            shutdown_and_end(http, &self.shared, cause);
        } else {
            self.shared.end(cause);
        }
        self.abort_writer();
    }

    /// Stop the frame writer. For HTTP this aborts the writer before the
    /// watch, so dropping the watch cannot leave that writer holding the session.
    fn abort_writer(&self) {
        if let Some(http_writer) = &self.http_writer {
            http_writer.abort();
        }
        self.writer_watch.abort();
    }

    /// Abort the HTTP writer and leave its watch running, so a test can see
    /// that cancellation does not record an end. The callers are the Unix
    /// MCP fixtures.
    #[cfg(all(test, unix))]
    pub(crate) fn abort_http_writer_for_test(&self) {
        self.http_writer.as_ref().expect("http writer").abort();
    }

    /// Whether the HTTP writer watch has finished joining that task.
    #[cfg(all(test, unix))]
    pub(crate) fn http_writer_watch_finished(&self) -> bool {
        self.writer_watch.is_finished()
    }

    /// Frames waiting in the writer queue, not the one the writer already
    /// dequeued. Unix MCP fixtures use this to see a `RecoveryReady` arrive
    /// before the writer panics.
    #[cfg(all(test, unix))]
    pub(crate) fn outgoing_queued_for_test(&self) -> usize {
        self.outgoing
            .sender
            .max_capacity()
            .saturating_sub(self.outgoing.sender.capacity())
    }
}

/// Fence new HTTP posts, then record `cause` once. The fence is first so a
/// wake inside [`Shared::end`] already sees it.
fn shutdown_and_end(session: &HttpSession, shared: &Shared, cause: McpError) {
    session.shutdown();
    shared.end(cause);
}

/// Poll `future` and discard a panic payload. The future is not resumed after
/// that panic. The caller settles the writer after this await returns, which
/// drops `future` and any sender it still owns.
fn catch_writer_panic<F: Future>(future: F) -> impl Future<Output = Result<F::Output, ()>> {
    let mut future = Box::pin(future);
    std::future::poll_fn(move |context| {
        match std::panic::catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(context))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(value)) => Poll::Ready(Ok(value)),
            Err(_) => Poll::Ready(Err(())),
        }
    })
}

/// Log a fixed marker and the server id. The panic message stays out of that
/// record. The default panic hook can still print the message; request and
/// response debug output does not include header values or body bytes.
fn log_writer_panic(session: &HttpSession) {
    tracing::error!(server = %session.server(), "custom HTTP writer panicked");
}

/// Log the marker, then [`shutdown_and_end`] with [`McpError::ServerGone`].
/// This is the watch path: an escaped panic can settle as not panicked,
/// because the task future is dropped after the panic was caught.
fn record_writer_panic(session: &HttpSession, shared: &Shared) {
    log_writer_panic(session);
    shutdown_and_end(session, shared, McpError::ServerGone);
}

/// Abort the writer loop if the task that joins it is dropped first.
struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// First settlement wins. A caught panic settles itself as panicked before
/// this drop. Cancellation and a clean exit settle as not panicked.
struct SettleWriter<'a>(&'a HttpSession);

impl Drop for SettleWriter<'_> {
    fn drop(&mut self) {
        self.0.settle_writer(std::thread::panicking());
    }
}

/// Join the HTTP writer. A panic that still fails the task is not a second
/// end owner: [`record_writer_panic`]. An exchange panic is caught in the
/// writer, which settles that outcome and records it after an already-due
/// clock sleep. A cause already recorded stays. A cancellation records nothing.
async fn watch_http_writer(writer: JoinHandle<()>, shared: Arc<Shared>, session: Arc<HttpSession>) {
    let Err(error) = writer.await else {
        return;
    };
    if error.is_panic() {
        record_writer_panic(&session, &shared);
    }
}

/// A call that has been admitted. Dropped before its answer, it gives up its
/// place, and cancels the request upstream when the request was sent.
struct Pending<'a> {
    connection: &'a Connection,
    /// Zero once answered.
    id: u64,
    sent: bool,
}
impl Drop for Pending<'_> {
    fn drop(&mut self) {
        if self.id == 0 {
            return;
        }
        let waiting = self
            .connection
            .shared
            .state
            .lock()
            .expect("connection state")
            .pending
            .remove(&self.id)
            .is_some();
        if waiting && self.sent {
            if let Some(http) = &self.connection.http {
                http.cancel_post(self.id);
            }
            // Best effort: a drop cannot wait for room in the queue, and a
            // server that misses it answers a request nobody reads.
            let cancelled = json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": { "requestId": self.id, "reason": "the caller stopped waiting" },
            });
            if let Ok(frame) = framing::encode(&cancelled) {
                let _ = self.connection.outgoing.try_send(Outgoing::Frame(frame));
            }
        }
    }
}

/// A JSON-RPC error object as [`McpError::Remote`]; an object without an
/// integer code is [`McpError::Malformed`].
pub(crate) fn remote(error: &Value) -> McpError {
    let Some(code) = error.get("code").and_then(Value::as_i64) else {
        return McpError::Malformed("an error answer without an integer code".into());
    };
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut end = message.len().min(MAX_ERROR_MESSAGE_BYTES);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    McpError::Remote {
        code,
        message: message[..end].to_owned(),
    }
}

async fn read<R: AsyncRead + Unpin>(
    mut frames: Frames<R>,
    shared: Arc<Shared>,
    outgoing: OutgoingQueue,
) {
    let cause = loop {
        let bytes = match frames.next().await {
            Ok(bytes) => bytes,
            Err(FrameEnd::Closed) => break McpError::ServerGone,
            Err(FrameEnd::TooLarge) => break McpError::TooLarge("a frame from the MCP server"),
        };
        let Ok(message) = serde_json::from_slice::<Value>(&bytes) else {
            break McpError::Malformed("a frame from the MCP server is not JSON".into());
        };
        if let Some(cause) = deliver(&message, &shared, &outgoing, None) {
            break cause;
        }
    };
    shared.end(cause);
}

/// Apply one JSON-RPC message. `Some` ends the connection with that cause.
fn deliver(
    message: &Value,
    shared: &Shared,
    outgoing: &OutgoingQueue,
    reply: Option<super::http::ReplyContext>,
) -> Option<McpError> {
    let method = message.get("method").and_then(Value::as_str);
    let id = message.get("id").filter(|id| !id.is_null());
    match (method, id) {
        // A request of the server's own. This client declared no
        // sampling, roots or elicitation, so only `ping` has an answer.
        (Some(method), Some(id)) => {
            let answer = if method == "ping" {
                json!({ "jsonrpc": "2.0", "id": id, "result": {} })
            } else {
                json!({ "jsonrpc": "2.0", "id": id, "error": {
                    "code": -32601, "message": "not supported by this MCP client" } })
            };
            // Never awaited: waiting here for room in a queue the server
            // is not draining would stop reading what it writes.
            if let Ok(frame) = framing::encode(&answer) {
                match reply {
                    Some(context) => {
                        match outgoing.try_send(Outgoing::PeerReply { frame, context }) {
                            Ok(()) => {}
                            Err(TrySendError::Full(_)) => {
                                return Some(McpError::TooLarge("queued MCP control frames"));
                            }
                            Err(TrySendError::Closed(_)) => {
                                return Some(
                                    shared
                                        .state
                                        .lock()
                                        .expect("connection state")
                                        .ended
                                        .clone()
                                        .unwrap_or(McpError::ServerGone),
                                );
                            }
                        }
                    }
                    None => {
                        let _ = outgoing.try_send(Outgoing::Frame(frame));
                    }
                }
            }
        }
        (Some(method), None) => {
            if method.ends_with("/list_changed") {
                let _ = shared.notices.send(Arc::new(message.clone()));
            }
        }
        (None, Some(id)) => {
            let id = id.as_u64()?;
            let waiter = shared
                .state
                .lock()
                .expect("connection state")
                .pending
                .remove(&id)?;
            let reply = match (message.get("result"), message.get("error")) {
                (Some(result), None) => Ok(Ok(result.clone())),
                (None, Some(error)) if error.is_object() => Ok(Err(error.clone())),
                _ => Err(McpError::Malformed(
                    "an answer without one result or error object".into(),
                )),
            };
            let _ = waiter.send(reply);
        }
        (None, None) => {}
    }
    None
}

/// The HTTP reader's loop: one JSON message at a time, the same correlation
/// as [`read`].
async fn read_messages(
    mut incoming: mpsc::Receiver<Result<super::http::HttpMessage, McpError>>,
    shared: Arc<Shared>,
    outgoing: OutgoingQueue,
    session: Weak<HttpSession>,
) {
    let cause = loop {
        let bytes = match incoming.recv().await {
            Some(Ok(bytes)) => bytes,
            Some(Err(error)) => break error,
            None => break McpError::ServerGone,
        };
        let Ok(message) = serde_json::from_slice::<Value>(&bytes) else {
            break McpError::Malformed("a frame from the MCP server is not JSON".into());
        };
        if let Some(cause) = deliver(&message, &shared, &outgoing, bytes.reply) {
            break cause;
        }
    };
    if let Some(session) = session.upgrade() {
        shutdown_and_end(&session, &shared, cause);
    } else {
        shared.end(cause);
    }
}

#[cfg(all(test, unix))]
#[path = "../../../tests/infrastructure/mcp/connection_end.rs"]
mod end_tests;
