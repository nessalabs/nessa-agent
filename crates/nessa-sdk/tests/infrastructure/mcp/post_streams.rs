//! ADR 392 P1–P8: owned POST bodies under controlled chunk/header ordering.
use super::super::connection::Connection;
use super::super::http::{HttpSession, SendOutcome, SessionClaims, MAX_POST_STREAMS};
use super::super::{
    HttpBody, HttpChunks, HttpExchange, HttpFailure, HttpMethod, HttpRequest, HttpResponse,
    McpError, NoAuthorization, RemoteMcpUrl,
};
use crate::infrastructure::clock::RuntimeClock;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::future::{poll_fn, Future};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::Poll;
use std::time::Duration;
use tokio::sync::{mpsc, Notify};
use uuid::Uuid;

struct DropGate {
    entered: Notify,
    released: Mutex<bool>,
    ready: Condvar,
}

impl DropGate {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.ready.notify_all();
    }
}

struct ReleaseOnDrop(Arc<DropGate>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct Body {
    chunks: mpsc::UnboundedReceiver<Result<Option<Vec<u8>>, HttpFailure>>,
    dropped: Arc<AtomicUsize>,
    changed: Arc<Notify>,
    polled: Arc<AtomicUsize>,
    polling: Arc<Notify>,
    drop_gate: Arc<Mutex<Option<Arc<DropGate>>>>,
}

impl Drop for Body {
    fn drop(&mut self) {
        if let Some(gate) = self.drop_gate.lock().unwrap().as_ref() {
            gate.entered.notify_one();
            let mut released = gate.released.lock().unwrap();
            while !*released {
                released = gate.ready.wait(released).unwrap();
            }
        }
        self.dropped.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_one();
    }
}

#[async_trait]
impl HttpChunks for Body {
    async fn next(&mut self) -> Result<Option<Vec<u8>>, HttpFailure> {
        self.polled.fetch_add(1, Ordering::SeqCst);
        self.polling.notify_one();
        self.chunks.recv().await.unwrap_or(Ok(None))
    }
}

struct Probe {
    send: mpsc::UnboundedSender<Result<Option<Vec<u8>>, HttpFailure>>,
    dropped: Arc<AtomicUsize>,
    changed: Arc<Notify>,
    polled: Arc<AtomicUsize>,
    polling: Arc<Notify>,
    drop_gate: Arc<Mutex<Option<Arc<DropGate>>>>,
}

impl Probe {
    fn body() -> (Self, HttpBody) {
        let (send, chunks) = mpsc::unbounded_channel();
        let dropped = Arc::new(AtomicUsize::new(0));
        let changed = Arc::new(Notify::new());
        let polled = Arc::new(AtomicUsize::new(0));
        let polling = Arc::new(Notify::new());
        let drop_gate = Arc::new(Mutex::new(None));
        (
            Self {
                send,
                dropped: dropped.clone(),
                changed: changed.clone(),
                polled: polled.clone(),
                polling: polling.clone(),
                drop_gate: drop_gate.clone(),
            },
            HttpBody::Stream(Box::new(Body {
                chunks,
                dropped,
                changed,
                polled,
                polling,
                drop_gate,
            })),
        )
    }

    fn event(&self, value: Value) {
        self.send
            .send(Ok(Some(format!("data: {value}\n\n").into_bytes())))
            .unwrap();
    }

    async fn polled(&self, count: usize) {
        bounded(async {
            while self.polled.load(Ordering::SeqCst) < count {
                self.polling.notified().await;
            }
        })
        .await;
    }

    async fn released(&self) {
        bounded(async {
            while self.dropped.load(Ordering::SeqCst) == 0 {
                self.changed.notified().await;
            }
        })
        .await;
        assert_eq!(self.dropped.load(Ordering::SeqCst), 1);
    }
}

struct Peer {
    bodies: Mutex<VecDeque<HttpBody>>,
    posts: AtomicUsize,
    entered: Notify,
    header_gate: Option<Arc<Notify>>,
    stream_notifications: bool,
    gate_after: usize,
}

impl Peer {
    fn new(bodies: Vec<HttpBody>) -> Arc<Self> {
        Arc::new(Self {
            bodies: Mutex::new(bodies.into()),
            posts: AtomicUsize::new(0),
            entered: Notify::new(),
            header_gate: None,
            stream_notifications: false,
            gate_after: 1,
        })
    }
}

#[async_trait]
impl HttpExchange for Peer {
    async fn exchange(&self, request: HttpRequest) -> Result<HttpResponse, HttpFailure> {
        assert_eq!(request.method, HttpMethod::Post);
        self.posts.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        if self.posts.load(Ordering::SeqCst) >= self.gate_after {
            if let Some(gate) = &self.header_gate {
                gate.notified().await;
            }
        }
        let message: Value = serde_json::from_slice(&request.body).unwrap();
        if !self.stream_notifications
            && (message.get("method").is_none() || message.get("id").is_none())
        {
            return Ok(HttpResponse {
                status: 202,
                headers: vec![],
                body: HttpBody::Buffered(vec![]),
            });
        }
        Ok(HttpResponse {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            body: self
                .bodies
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted body"),
        })
    }
}

fn session(peer: Arc<Peer>) -> (Arc<HttpSession>, mpsc::Receiver<Result<Vec<u8>, McpError>>) {
    HttpSession::open(
        Uuid::from_u128(1),
        RemoteMcpUrl::parse("http://127.0.0.1/mcp").unwrap(),
        peer,
        Arc::new(NoAuthorization),
        Arc::new(SessionClaims::default()),
    )
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(3), future)
        .await
        .expect("controlled lifecycle completes")
}

async fn dispatch(session: &HttpSession, id: u64) -> SendOutcome {
    session
        .dispatch(
            &serde_json::to_vec(&json!({"jsonrpc":"2.0", "id":id, "method":"tools/list"})).unwrap(),
        )
        .await
}

async fn finish(session: &HttpSession) {
    let mut done = session.finished();
    session.shutdown();
    bounded(done.wait_for(|done| *done)).await.unwrap();
}

#[tokio::test]
async fn post_result_releases_held_body() {
    let mut bodies = Vec::new();
    let mut probes = Vec::new();
    for id in 1..=MAX_POST_STREAMS as u64 + 5 {
        let (probe, body) = Probe::body();
        probe.event(json!({"id":id,"result":{}}));
        probes.push(probe);
        bodies.push(body);
    }
    let (session, mut incoming) = session(Peer::new(bodies));
    for (index, probe) in probes.iter().enumerate() {
        assert!(matches!(
            dispatch(&session, index as u64 + 1).await,
            SendOutcome::Done
        ));
        let message = bounded(incoming.recv()).await.unwrap().unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&message).unwrap()["id"],
            index + 1
        );
        probe.released().await;
    }
    finish(&session).await;
}

#[tokio::test]
async fn post_error_releases_held_body() {
    let (probe, body) = Probe::body();
    probe.event(json!({"id":1,"error":{"code":-1,"message":"failed"}}));
    let (session, mut incoming) = session(Peer::new(vec![body]));
    assert!(matches!(dispatch(&session, 1).await, SendOutcome::Done));
    assert!(
        serde_json::from_slice::<Value>(&bounded(incoming.recv()).await.unwrap().unwrap())
            .unwrap()
            .get("error")
            .is_some()
    );
    probe.released().await;
    finish(&session).await;
}

#[tokio::test]
async fn post_ping_before_result_remains_live() {
    let (probe, body) = Probe::body();
    // A server request can reuse the caller's numeric id. It is not a result.
    probe.event(json!({"jsonrpc":"2.0","id":1,"method":"ping"}));
    let peer = Peer::new(vec![body]);
    let (session, incoming) = session(peer.clone());
    let connection =
        Connection::open_http(session.clone(), incoming, Arc::new(RuntimeClock::new()));
    let mut call = Box::pin(connection.call("tools/list", None));
    bounded(async {
        loop {
            tokio::select! {
                result = &mut call => panic!("request settled before ping reply: {result:?}"),
                _ = peer.entered.notified() => if peer.posts.load(Ordering::SeqCst) == 2 { break; }
            }
        }
    })
    .await;
    assert_eq!(probe.dropped.load(Ordering::SeqCst), 0);
    probe.event(json!({"id":1,"result":{}}));
    assert_eq!(bounded(call).await.unwrap().unwrap(), json!({}));
    probe.released().await;
    finish(&session).await;
}

#[tokio::test]
async fn post_other_response_does_not_retire_reader() {
    let (probe, body) = Probe::body();
    for event in [
        json!({"id":2,"result":{}}),
        json!({"id":1,"method":"ping","result":{}}),
        json!({"id":1,"result":{},"error":{}}),
        json!({"id":1}),
    ] {
        probe.event(event);
    }
    let (session, mut incoming) = session(Peer::new(vec![body]));
    dispatch(&session, 1).await;
    for _ in 0..4 {
        bounded(incoming.recv()).await.unwrap().unwrap();
        assert_eq!(probe.dropped.load(Ordering::SeqCst), 0);
    }
    probe.event(json!({"id":1,"result":{}}));
    bounded(incoming.recv()).await.unwrap().unwrap();
    probe.released().await;
    finish(&session).await;
}

#[tokio::test]
async fn post_eof_and_failure_release_body() {
    for failed in [false, true] {
        let (probe, body) = Probe::body();
        probe
            .send
            .send(if failed {
                Err(HttpFailure::Unreachable)
            } else {
                Ok(None)
            })
            .unwrap();
        let (session, mut incoming) = session(Peer::new(vec![body]));
        dispatch(&session, 1).await;
        if failed {
            assert_eq!(
                bounded(incoming.recv()).await.unwrap(),
                Err(McpError::Unconfirmed)
            );
        }
        probe.released().await;
        finish(&session).await;
    }
}

#[tokio::test]
async fn post_caller_cancellation_releases_body() {
    let (probe, body) = Probe::body();
    // Observe a notice to establish reader admission before caller loss.
    probe.event(json!({"method":"notifications/tools/list_changed"}));
    let (session, incoming) = session(Peer::new(vec![body]));
    let connection =
        Connection::open_http(session.clone(), incoming, Arc::new(RuntimeClock::new()));
    let mut notices = connection.notices();
    let mut call = Box::pin(connection.call("tools/list", None));
    bounded(async {
        tokio::select! {
            result = &mut call => panic!("held call settled: {result:?}"),
            _ = notices.recv() => {}
        }
    })
    .await;
    drop(call);
    probe.released().await;
    finish(&session).await;
}

#[tokio::test]
async fn post_close_joins_held_and_blocked_readers() {
    let (held, held_body) = Probe::body();
    let (blocked, blocked_body) = Probe::body();
    // Saturate inbound delivery without relying on a reader polling handshake.
    for _ in 0..65 {
        blocked.event(json!({"method":"notifications/tools/list_changed"}));
    }
    let (session, _incoming) = session(Peer::new(vec![held_body, blocked_body]));
    dispatch(&session, 1).await;
    dispatch(&session, 2).await;
    held.polled(1).await;
    blocked.polled(65).await;
    session.shutdown();
    finish(&session).await;
    assert_eq!(held.dropped.load(Ordering::SeqCst), 1);
    assert_eq!(blocked.dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn post_drop_releases_body() {
    let (probe, body) = Probe::body();
    let (session, _incoming) = session(Peer::new(vec![body]));
    dispatch(&session, 1).await;
    let mut finished = session.finished();
    drop(session);
    bounded(finished.wait_for(|done| *done)).await.unwrap();
    assert_eq!(probe.dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn post_late_body_after_close_is_dropped() {
    let (probe, body) = Probe::body();
    let gate = Arc::new(Notify::new());
    let peer = Arc::new(Peer {
        bodies: Mutex::new(vec![body].into()),
        posts: AtomicUsize::new(0),
        entered: Notify::new(),
        header_gate: Some(gate.clone()),
        stream_notifications: false,
        gate_after: 1,
    });
    let (session, _incoming) = session(peer.clone());
    let mut send = Box::pin(dispatch(&session, 1));
    bounded(async {
        tokio::select! {
            _ = &mut send => panic!("header gate bypassed"),
            _ = peer.entered.notified() => {}
        }
    })
    .await;
    finish(&session).await;
    gate.notify_one();
    assert!(matches!(
        bounded(send).await,
        SendOutcome::End(McpError::Closed)
    ));
    assert_eq!(probe.dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn post_reader_bound_refuses_before_exchange_and_recovers() {
    let mut probes = Vec::new();
    let mut bodies = Vec::new();
    for _ in 0..MAX_POST_STREAMS + 1 {
        let (probe, body) = Probe::body();
        probes.push(probe);
        bodies.push(body);
    }
    let peer = Peer::new(bodies);
    let (session, _incoming) = session(peer.clone());
    for id in 1..=MAX_POST_STREAMS as u64 {
        assert!(matches!(dispatch(&session, id).await, SendOutcome::Done));
    }
    assert!(matches!(
        dispatch(&session, 999).await,
        SendOutcome::FailCall {
            id: Some(999),
            error: McpError::Busy
        }
    ));
    assert_eq!(peer.posts.load(Ordering::SeqCst), MAX_POST_STREAMS);
    let ping_reply = session
        .dispatch(&serde_json::to_vec(&json!({"id":100,"result":{}})).unwrap())
        .await;
    assert!(matches!(ping_reply, SendOutcome::Done));
    session.cancel_post(1);
    probes[0].released().await;
    assert!(matches!(dispatch(&session, 999).await, SendOutcome::Done));
    finish(&session).await;
    for probe in probes {
        assert_eq!(probe.dropped.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn post_notification_stream_at_capacity_is_unconfirmed() {
    let mut probes = Vec::new();
    let mut bodies = Vec::new();
    for _ in 0..MAX_POST_STREAMS + 1 {
        let (probe, body) = Probe::body();
        probes.push(probe);
        bodies.push(body);
    }
    let peer = Arc::new(Peer {
        bodies: Mutex::new(bodies.into()),
        posts: AtomicUsize::new(0),
        entered: Notify::new(),
        header_gate: None,
        stream_notifications: true,
        gate_after: 1,
    });
    let (session, _incoming) = session(peer);
    for id in 1..=MAX_POST_STREAMS as u64 {
        dispatch(&session, id).await;
    }
    let outcome = session
        .dispatch(
            &serde_json::to_vec(&json!({"method":"notifications/tools/list_changed"})).unwrap(),
        )
        .await;
    assert!(matches!(outcome, SendOutcome::End(McpError::Unconfirmed)));
    assert_eq!(probes[MAX_POST_STREAMS].dropped.load(Ordering::SeqCst), 1);
    finish(&session).await;
}

#[tokio::test]
async fn post_cancellation_with_full_outgoing_queue_releases_active_body() {
    let (probe, body) = Probe::body();
    probe.event(json!({"method":"notifications/tools/list_changed"}));
    let gate = Arc::new(Notify::new());
    let peer = Arc::new(Peer {
        bodies: Mutex::new(vec![body].into()),
        posts: AtomicUsize::new(0),
        entered: Notify::new(),
        header_gate: Some(gate),
        stream_notifications: false,
        gate_after: 2,
    });
    let (session, incoming) = session(peer.clone());
    let connection =
        Connection::open_http(session.clone(), incoming, Arc::new(RuntimeClock::new()));
    let mut notices = connection.notices();
    let mut call = Box::pin(connection.call("tools/list", None));
    bounded(async { tokio::select! { result = &mut call => panic!("held call settled: {result:?}"), _ = notices.recv() => {} } }).await;
    connection.notify("notifications/test", None).await.unwrap();
    bounded(async {
        while peer.posts.load(Ordering::SeqCst) < 2 {
            peer.entered.notified().await;
        }
    })
    .await;
    // Fill until the next notification is demonstrably waiting for queue space.
    tokio::task::yield_now().await;
    let mut queued = Box::pin(async {
        for _ in 0..65 {
            connection.notify("notifications/test", None).await.unwrap();
        }
    });
    assert!(poll_fn(|cx| Poll::Ready(queued.as_mut().poll(cx).is_pending())).await);
    drop(call);
    probe.released().await;
    drop(queued);
    connection.close(McpError::Closed);
    finish(&session).await;
}

#[tokio::test]
async fn post_cancellation_during_headers_releases_late_body() {
    let (probe, body) = Probe::body();
    let gate = Arc::new(Notify::new());
    let peer = Arc::new(Peer {
        bodies: Mutex::new(vec![body].into()),
        posts: AtomicUsize::new(0),
        entered: Notify::new(),
        header_gate: Some(gate.clone()),
        stream_notifications: false,
        gate_after: 1,
    });
    let (session, incoming) = session(peer.clone());
    let connection =
        Connection::open_http(session.clone(), incoming, Arc::new(RuntimeClock::new()));
    let mut call = Box::pin(connection.call("tools/list", None));
    bounded(async { tokio::select! { _ = &mut call => panic!("header gate bypassed"), _ = peer.entered.notified() => {} } }).await;
    drop(call);
    gate.notify_one();
    // The cancellation is consumed by dispatch before its own gated POST.
    probe.released().await;
    connection.close(McpError::Closed);
    finish(&session).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_finished_waits_for_reader_destruction() {
    let (probe, body) = Probe::body();
    let gate = Arc::new(DropGate {
        entered: Notify::new(),
        released: Mutex::new(false),
        ready: Condvar::new(),
    });
    let _release_on_drop = ReleaseOnDrop(gate.clone());
    *probe.drop_gate.lock().unwrap() = Some(gate.clone());
    let (session, _incoming) = session(Peer::new(vec![body]));
    dispatch(&session, 1).await;
    probe.polled(1).await;
    let mut finished = session.finished();
    session.shutdown();
    bounded(gate.entered.notified()).await;
    // The controlled destructor keeps the reader physically live. Give the
    // close worker a bounded opportunity to publish premature completion.
    let early = tokio::time::timeout(Duration::from_millis(50), finished.wait_for(|done| *done))
        .await
        .is_err();
    gate.release();
    assert!(early, "finished was published before reader destruction");
    bounded(finished.wait_for(|done| *done)).await.unwrap();
    assert_eq!(probe.dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn post_drop_outside_runtime_context_joins_body() {
    let (probe, body) = Probe::body();
    let (session, _incoming) = session(Peer::new(vec![body]));
    dispatch(&session, 1).await;
    probe.polled(1).await;
    let mut finished = session.finished();
    std::thread::spawn(move || drop(session)).join().unwrap();
    bounded(finished.wait_for(|done| *done)).await.unwrap();
    assert_eq!(probe.dropped.load(Ordering::SeqCst), 1);
}
