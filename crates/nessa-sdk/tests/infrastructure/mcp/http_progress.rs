//! ADR 392 J1–J14: JSON/initialization progress and replacement cleanup ownership.
use super::super::connection::{Connection, Outgoing};
use super::super::http::{HttpSession, SendOutcome};
use super::super::{
    Bearer, HttpBody, HttpExchange, HttpFailure, HttpMethod, HttpRequest, HttpResponse, McpError,
    McpServers, NoAuthorization, RemoteAuthorization, RemoteMcpServer, RemoteMcpUrl, SessionClaims,
    INITIALIZE_TIMEOUT,
};
use super::post_body::{bounded, Probe};
use crate::infrastructure::clock::{
    manual::ManualClock, Clock, ClockInstant, ClockSleep, RuntimeClock,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tokio::sync::{mpsc, Notify, Semaphore};
use uuid::Uuid;

struct Gate {
    entered: AtomicUsize,
    changed: Notify,
    permits: Semaphore,
}
impl Default for Gate {
    fn default() -> Self {
        Self {
            entered: AtomicUsize::new(0),
            changed: Notify::new(),
            permits: Semaphore::new(0),
        }
    }
}
impl Gate {
    async fn enter(&self) {
        self.entered.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_one();
        self.permits.acquire().await.unwrap().forget();
    }
    async fn reached(&self) {
        bounded(async {
            while self.entered.load(Ordering::SeqCst) == 0 {
                self.changed.notified().await;
            }
        })
        .await;
    }
    fn release(&self) {
        self.permits.add_permits(1);
    }
}

struct Peer {
    initial: Mutex<Option<HttpBody>>,
    replacement: Mutex<Option<HttpBody>>,
    bodies: Mutex<VecDeque<HttpBody>>,
    expire: AtomicBool,
    seen: Mutex<Vec<HttpRequest>>,
    changed: Notify,
    recovery_headers: Option<Arc<Gate>>,
    initialized_headers: Option<Arc<Gate>>,
    delete: Option<Arc<Gate>>,
    initialized_status: u16,
    initialized_expiry: Option<Arc<ManualClock>>,
    replacement_json: bool,
    call_sse: bool,
}
impl Peer {
    fn new() -> Self {
        Self {
            initial: Mutex::new(None),
            replacement: Mutex::new(None),
            bodies: Mutex::new(VecDeque::new()),
            expire: AtomicBool::new(false),
            seen: Mutex::new(vec![]),
            changed: Notify::new(),
            recovery_headers: None,
            initialized_headers: None,
            delete: None,
            initialized_status: 202,
            initialized_expiry: None,
            replacement_json: false,
            call_sse: false,
        }
    }
    async fn observed(&self, predicate: impl Fn(&HttpRequest) -> bool) {
        bounded(async {
            loop {
                if self.seen.lock().unwrap().iter().any(&predicate) {
                    return;
                }
                self.changed.notified().await;
            }
        })
        .await;
    }
    fn count(&self, predicate: impl Fn(&HttpRequest) -> bool) -> usize {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|request| predicate(request))
            .count()
    }
}
fn method(request: &HttpRequest) -> Option<String> {
    serde_json::from_slice::<Value>(&request.body)
        .ok()?
        .get("method")?
        .as_str()
        .map(str::to_owned)
}
fn header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}
fn json_response(value: Value) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![("content-type".into(), "application/json".into())],
        body: HttpBody::Buffered(serde_json::to_vec(&value).unwrap()),
    }
}
#[async_trait]
impl HttpExchange for Peer {
    async fn exchange(&self, request: HttpRequest) -> Result<HttpResponse, HttpFailure> {
        self.seen.lock().unwrap().push(request.clone());
        self.changed.notify_one();
        match request.method {
            HttpMethod::Delete => {
                if let Some(gate) = &self.delete {
                    gate.enter().await;
                }
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: HttpBody::Buffered(vec![]),
                })
            }
            HttpMethod::Get => Ok(HttpResponse {
                status: 405,
                headers: vec![],
                body: HttpBody::Buffered(vec![]),
            }),
            HttpMethod::Post => {
                let message: Value = serde_json::from_slice(&request.body).unwrap();
                if method(&request).as_deref() == Some("initialize") {
                    let recovery = message["id"] == 0;
                    if recovery {
                        if let Some(gate) = &self.recovery_headers {
                            gate.enter().await;
                        }
                    }
                    let body = if recovery {
                        self.replacement.lock().unwrap().take()
                    } else {
                        self.initial.lock().unwrap().take()
                    };
                    let mut response = json_response(
                        json!({"jsonrpc":"2.0","id":message["id"],"result":{"protocolVersion":"2025-06-18"}}),
                    );
                    response.headers.push((
                        "mcp-session-id".into(),
                        if recovery { "replacement" } else { "initial" }.into(),
                    ));
                    if let Some(body) = body {
                        response.headers[0].1 = if recovery && self.replacement_json {
                            "application/json"
                        } else {
                            "text/event-stream"
                        }
                        .into();
                        response.body = body;
                    }
                    return Ok(response);
                }
                if message.get("method").is_none() || message.get("id").is_none() {
                    let recovering = header(&request, "mcp-session-id") == Some("replacement");
                    if recovering
                        && method(&request).as_deref() == Some("notifications/initialized")
                    {
                        if let Some(gate) = &self.initialized_headers {
                            gate.enter().await;
                        }
                        if let Some(clock) = &self.initialized_expiry {
                            clock.advance(INITIALIZE_TIMEOUT);
                        }
                    }
                    return Ok(HttpResponse {
                        status: if recovering {
                            self.initialized_status
                        } else {
                            202
                        },
                        headers: vec![],
                        body: HttpBody::Buffered(vec![]),
                    });
                }
                if self.expire.swap(false, Ordering::SeqCst) {
                    return Ok(HttpResponse {
                        status: 404,
                        headers: vec![],
                        body: HttpBody::Buffered(vec![]),
                    });
                }
                let mut response = json_response(
                    json!({"jsonrpc":"2.0","id":message["id"],"result":{"tools":[]}}),
                );
                if let Some(body) = self.bodies.lock().unwrap().pop_front() {
                    response.body = body;
                    if self.call_sse {
                        response.headers[0].1 = "text/event-stream".into();
                    }
                }
                Ok(response)
            }
        }
    }
}
fn transport(
    peer: Arc<Peer>,
    claims: Arc<SessionClaims>,
) -> (Arc<HttpSession>, mpsc::Receiver<Result<Vec<u8>, McpError>>) {
    HttpSession::open(
        Uuid::from_u128(1),
        RemoteMcpUrl::parse("http://127.0.0.1/mcp").unwrap(),
        peer,
        Arc::new(NoAuthorization),
        claims,
    )
}
async fn initialize(session: &HttpSession) {
    assert!(matches!(
        session.dispatch(br#"{"id":1,"method":"initialize"}"#).await,
        SendOutcome::Done
    ));
}
async fn stop(session: &HttpSession) {
    let mut done = session.finished();
    session.shutdown();
    bounded(done.wait_for(|done| *done)).await.unwrap();
}
async fn call(session: &HttpSession, id: u64) -> SendOutcome {
    session
        .dispatch(&serde_json::to_vec(&json!({"id":id,"method":"tools/list"})).unwrap())
        .await
}

#[tokio::test]
async fn j1_stalled_json_timeout_allows_cancel_and_next_call() {
    let (probe, body) = Probe::body();
    let peer = Arc::new(Peer::new());
    peer.bodies.lock().unwrap().push_back(body);
    let (session, incoming) = transport(peer.clone(), Arc::default());
    let clock = Arc::new(ManualClock::default());
    let connection = Arc::new(Connection::open_http(
        session.clone(),
        incoming,
        clock.clone(),
    ));
    let task = tokio::spawn({
        let connection = connection.clone();
        async move {
            connection
                .request("tools/call", None, Duration::from_secs(1))
                .await
        }
    });
    probe.polled(1).await;
    clock.advance(Duration::from_secs(1));
    assert_eq!(bounded(task).await.unwrap().unwrap_err(), McpError::Timeout);
    probe.released().await;
    peer.observed(|request| method(request).as_deref() == Some("notifications/cancelled"))
        .await;
    assert!(bounded(connection.call("tools/list", None))
        .await
        .unwrap()
        .is_ok());
    stop(&session).await;
    assert_eq!(
        peer.count(|request| method(request).as_deref() == Some("tools/call")),
        1
    );
}

#[tokio::test]
async fn j2_json_read_failure_and_bound_are_typed() {
    for oversized in [false, true] {
        let (probe, body) = Probe::body();
        if oversized {
            probe
                .send
                .send(Ok(Some(vec![
                    b'x';
                    super::super::framing::MAX_FRAME_BYTES + 1
                ])))
                .unwrap();
        } else {
            probe.send.send(Err(HttpFailure::Unreachable)).unwrap();
        }
        let peer = Arc::new(Peer::new());
        peer.bodies.lock().unwrap().push_back(body);
        let (session, incoming) = transport(peer, Arc::default());
        let connection =
            Connection::open_http(session.clone(), incoming, Arc::new(RuntimeClock::new()));
        assert_eq!(
            bounded(connection.call("tools/list", None))
                .await
                .unwrap_err(),
            if oversized {
                McpError::TooLarge("an HTTP response body")
            } else {
                McpError::Unconfirmed
            }
        );
        stop(&session).await;
        probe.released().await;
    }
}

#[tokio::test]
async fn j3_public_open_held_initialize_dispatches_initialized_list_and_ping() {
    let (probe, body) = Probe::body();
    probe.event(json!({"jsonrpc":"2.0","id":100,"method":"ping"}));
    probe.event(json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18"}}));
    let peer = Arc::new(Peer::new());
    *peer.initial.lock().unwrap() = Some(body);
    let servers = McpServers::new(vec![], Arc::new(RuntimeClock::new())).unwrap();
    servers.set_remote_transport(peer.clone(), Arc::new(NoAuthorization));
    let session = bounded(servers.open_remote_once(
        &RemoteMcpServer::new(Uuid::from_u128(1), "remote", "http://127.0.0.1/mcp").unwrap(),
    ))
    .await
    .unwrap();
    probe.released().await;
    bounded(session.list_tools()).await.unwrap();
    peer.observed(|request| method(request).as_deref() == Some("notifications/initialized"))
        .await;
    assert!(peer
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|request| method(request).as_deref() == Some("tools/list")
            || request.method == HttpMethod::Get)
        .all(
            |request| header(request, "Mcp-Session-Id") == Some("initial")
                && header(request, "MCP-Protocol-Version") == Some("2025-06-18")
        ));
    peer.observed(|request| {
        serde_json::from_slice::<Value>(&request.body)
            .ok()
            .is_some_and(|message| message["id"] == 100 && message.get("result").is_some())
    })
    .await;
    session.close().await;
}

#[tokio::test]
async fn j4_invalid_initialize_version_does_not_claim_or_start_get() {
    let (probe, body) = Probe::body();
    probe.event(json!({"id":1,"result":{"protocolVersion":"unsupported"}}));
    let peer = Arc::new(Peer::new());
    *peer.initial.lock().unwrap() = Some(body);
    let claims = Arc::new(SessionClaims::default());
    let (session, mut incoming) = transport(peer.clone(), claims.clone());
    initialize(&session).await;
    assert!(matches!(
        bounded(incoming.recv()).await.unwrap(),
        Err(McpError::Handshake(_))
    ));
    assert_eq!(peer.count(|request| request.method == HttpMethod::Get), 0);
    assert!(claims.claim("http://127.0.0.1/mcp", "initial", u64::MAX));
    stop(&session).await;
    probe.released().await;
    assert_eq!(
        peer.count(|request| request.method == HttpMethod::Delete),
        0
    );
}

#[tokio::test]
async fn j5_close_held_initialize_body_has_no_late_publication() {
    let (probe, body) = Probe::body();
    let peer = Arc::new(Peer::new());
    *peer.initial.lock().unwrap() = Some(body);
    let claims = Arc::new(SessionClaims::default());
    let (session, _incoming) = transport(peer.clone(), claims.clone());
    initialize(&session).await;
    probe.polled(1).await;
    stop(&session).await;
    probe.released().await;
    assert!(claims.claim("http://127.0.0.1/mcp", "initial", u64::MAX));
    assert_eq!(peer.count(|request| request.method != HttpMethod::Post), 0);
}

#[tokio::test]
async fn j6_j7_close_owns_delete_and_retains_claim_through_completion() {
    let gate = Arc::new(Gate::default());
    let mut peer = Peer::new();
    peer.delete = Some(gate.clone());
    let peer = Arc::new(peer);
    let claims = Arc::new(SessionClaims::default());
    let (session, _incoming) = transport(peer.clone(), claims.clone());
    initialize(&session).await;
    let mut done = session.finished();
    session.shutdown();
    gate.reached().await;
    session.shutdown();
    assert!(!*done.borrow());
    assert!(!claims.claim("http://127.0.0.1/mcp", "initial", u64::MAX));
    drop(session);
    assert_eq!(
        peer.count(|request| request.method == HttpMethod::Delete),
        1
    );
    gate.release();
    bounded(done.wait_for(|done| *done)).await.unwrap();
    assert!(claims.claim("http://127.0.0.1/mcp", "initial", u64::MAX));
}

#[tokio::test]
async fn j8_recovery_owned_before_headers_and_controls_remain_available() {
    let gate = Arc::new(Gate::default());
    let mut peer = Peer::new();
    peer.recovery_headers = Some(gate.clone());
    let peer = Arc::new(peer);
    let (session, incoming) = transport(peer.clone(), Arc::default());
    let connection =
        Connection::open_http(session.clone(), incoming, Arc::new(RuntimeClock::new()));
    bounded(connection.call("initialize", None))
        .await
        .unwrap()
        .unwrap();
    peer.expire.store(true, Ordering::SeqCst);
    assert_eq!(
        bounded(connection.call("tools/call", None))
            .await
            .unwrap_err(),
        McpError::SessionExpired
    );
    gate.reached().await;
    assert_eq!(
        bounded(connection.call("tools/list", None))
            .await
            .unwrap_err(),
        McpError::Busy
    );
    connection
        .notify("notifications/cancelled", Some(json!({"requestId":2})))
        .await
        .unwrap();
    peer.observed(|request| method(request).as_deref() == Some("notifications/cancelled"))
        .await;
    stop(&session).await;
    assert_eq!(
        peer.count(|request| request.method == HttpMethod::Delete),
        0
    );
    assert_eq!(
        peer.count(|request| method(request).as_deref() == Some("tools/call")),
        1
    );
}

#[tokio::test]
async fn j9_recovery_gate_covers_queued_calls_and_initialized_headers() {
    let gate = Arc::new(Gate::default());
    let mut peer = Peer::new();
    peer.initialized_headers = Some(gate.clone());
    let peer = Arc::new(peer);
    let (session, _incoming) = transport(peer.clone(), Arc::default());
    let clock = Arc::new(ManualClock::default());
    let (writer, mut queue) = mpsc::channel(1);
    session.set_writer(writer.clone(), clock.clone());
    initialize(&session).await;
    writer
        .send(Outgoing::Frame(b"queued request".to_vec()))
        .await
        .unwrap();
    peer.expire.store(true, Ordering::SeqCst);
    assert!(matches!(
        call(&session, 2).await,
        SendOutcome::FailCall {
            error: McpError::SessionExpired,
            ..
        }
    ));
    peer.observed(|request| {
        request.method == HttpMethod::Get
            && header(request, "Mcp-Session-Id") == Some("replacement")
    })
    .await;
    assert!(matches!(
        call(&session, 3).await,
        SendOutcome::FailCall {
            error: McpError::Busy,
            ..
        }
    ));
    assert!(matches!(queue.recv().await, Some(Outgoing::Frame(_))));
    let Outgoing::RecoveryReady {
        deadline,
        completed,
    } = bounded(queue.recv()).await.unwrap()
    else {
        panic!("owned recovery handoff")
    };
    let finish = tokio::spawn({
        let session = session.clone();
        async move {
            let result = session.finish_recovery(deadline, completed).await;
            result
        }
    });
    gate.reached().await;
    assert!(matches!(
        call(&session, 4).await,
        SendOutcome::FailCall {
            error: McpError::Busy,
            ..
        }
    ));
    gate.release();
    assert!(matches!(bounded(finish).await.unwrap(), SendOutcome::Done));
    assert!(matches!(call(&session, 5).await, SendOutcome::Done));
    stop(&session).await;
}

#[tokio::test]
async fn j9_expired_queued_handoff_cannot_dispatch_initialized() {
    let peer = Arc::new(Peer::new());
    let (session, mut incoming) = transport(peer.clone(), Arc::default());
    let clock = Arc::new(ManualClock::default());
    let (writer, mut queue) = mpsc::channel(1);
    session.set_writer(writer, clock.clone());
    initialize(&session).await;
    peer.expire.store(true, Ordering::SeqCst);
    assert!(matches!(
        call(&session, 2).await,
        SendOutcome::FailCall {
            error: McpError::SessionExpired,
            ..
        }
    ));
    let Outgoing::RecoveryReady {
        deadline,
        completed,
    } = bounded(queue.recv()).await.unwrap()
    else {
        panic!("owned recovery handoff")
    };
    clock.advance(INITIALIZE_TIMEOUT);
    bounded(async {
        loop {
            if let Some(Err(error)) = incoming.recv().await {
                assert_eq!(error, McpError::Timeout);
                break;
            }
        }
    })
    .await;
    assert!(matches!(
        session.finish_recovery(deadline, completed).await,
        SendOutcome::End(McpError::Timeout)
    ));
    assert_eq!(
        peer.count(|request| method(request).as_deref() == Some("notifications/initialized")),
        0
    );
    assert!(matches!(
        call(&session, 3).await,
        SendOutcome::FailCall {
            error: McpError::Busy,
            ..
        }
    ));
    stop(&session).await;
}

#[tokio::test]
async fn j9_rejected_initialized_does_not_release_admission() {
    let mut peer = Peer::new();
    peer.initialized_status = 405;
    let peer = Arc::new(peer);
    let (session, _incoming) = transport(peer.clone(), Arc::default());
    let (writer, mut queue) = mpsc::channel(1);
    session.set_writer(writer, Arc::new(RuntimeClock::new()));
    initialize(&session).await;
    peer.expire.store(true, Ordering::SeqCst);
    assert!(matches!(
        call(&session, 2).await,
        SendOutcome::FailCall {
            error: McpError::SessionExpired,
            ..
        }
    ));
    let Outgoing::RecoveryReady {
        deadline,
        completed,
    } = bounded(queue.recv()).await.unwrap()
    else {
        panic!("owned recovery handoff")
    };
    let outcome = session.finish_recovery(deadline, completed).await;
    assert!(matches!(outcome, SendOutcome::End(McpError::Malformed(_))));
    assert!(matches!(
        call(&session, 3).await,
        SendOutcome::FailCall {
            error: McpError::Busy,
            ..
        }
    ));
    stop(&session).await;
}

#[tokio::test]
async fn j9_timeout_while_initialized_headers_held_cannot_reopen() {
    let gate = Arc::new(Gate::default());
    let mut peer = Peer::new();
    peer.initialized_headers = Some(gate.clone());
    let peer = Arc::new(peer);
    let (session, incoming) = transport(peer.clone(), Arc::default());
    let clock = Arc::new(ManualClock::default());
    let connection = Connection::open_http(session.clone(), incoming, clock.clone());
    bounded(connection.call("initialize", None))
        .await
        .unwrap()
        .unwrap();
    peer.expire.store(true, Ordering::SeqCst);
    assert_eq!(
        bounded(connection.call("tools/list", None))
            .await
            .unwrap_err(),
        McpError::SessionExpired
    );
    gate.reached().await;
    clock.advance(INITIALIZE_TIMEOUT);
    assert_eq!(bounded(connection.ended()).await, McpError::Timeout);
    gate.release();
    assert!(bounded(connection.call("tools/list", None)).await.is_err());
    stop(&session).await;
}

#[tokio::test]
async fn j6_recovery_publication_then_close_owns_one_retained_delete() {
    let gate = Arc::new(Gate::default());
    let mut peer = Peer::new();
    peer.delete = Some(gate.clone());
    let peer = Arc::new(peer);
    let claims = Arc::new(SessionClaims::default());
    let (session, _incoming) = transport(peer.clone(), claims.clone());
    let (writer, mut queue) = mpsc::channel(1);
    session.set_writer(writer, Arc::new(RuntimeClock::new()));
    initialize(&session).await;
    peer.expire.store(true, Ordering::SeqCst);
    assert!(matches!(
        call(&session, 2).await,
        SendOutcome::FailCall {
            error: McpError::SessionExpired,
            ..
        }
    ));
    let Outgoing::RecoveryReady {
        deadline,
        completed,
    } = bounded(queue.recv()).await.unwrap()
    else {
        panic!("owned recovery handoff")
    };
    let mut done = session.finished();
    session.shutdown();
    gate.reached().await;
    session.shutdown();
    assert!(!*done.borrow());
    assert!(!claims.claim("http://127.0.0.1/mcp", "replacement", u64::MAX));
    assert!(matches!(
        session.finish_recovery(deadline, completed).await,
        SendOutcome::End(McpError::Closed)
    ));
    drop(session);
    assert_eq!(
        peer.count(|request| request.method == HttpMethod::Delete),
        1
    );
    gate.release();
    bounded(done.wait_for(|done| *done)).await.unwrap();
    assert!(claims.claim("http://127.0.0.1/mcp", "replacement", u64::MAX));
    assert_eq!(
        peer.count(|request| request.method == HttpMethod::Delete),
        1
    );
}

#[tokio::test]
async fn j9_saturated_queue_budget_expiry_retains_close_ownership() {
    let peer = Arc::new(Peer::new());
    let (session, mut incoming) = transport(peer.clone(), Arc::default());
    let clock = Arc::new(ManualClock::default());
    let (writer, mut queue) = mpsc::channel(1);
    session.set_writer(writer.clone(), clock.clone());
    initialize(&session).await;
    writer.send(Outgoing::Frame(vec![])).await.unwrap();
    peer.expire.store(true, Ordering::SeqCst);
    assert!(matches!(
        call(&session, 2).await,
        SendOutcome::FailCall {
            error: McpError::SessionExpired,
            ..
        }
    ));
    peer.observed(|request| {
        request.method == HttpMethod::Get
            && header(request, "Mcp-Session-Id") == Some("replacement")
    })
    .await;
    clock.advance(INITIALIZE_TIMEOUT);
    bounded(async {
        loop {
            if let Some(Err(error)) = incoming.recv().await {
                assert_eq!(error, McpError::Timeout);
                break;
            }
        }
    })
    .await;
    assert!(matches!(queue.recv().await, Some(Outgoing::Frame(_))));
    assert!(queue.try_recv().is_err());
    assert!(matches!(
        call(&session, 3).await,
        SendOutcome::FailCall {
            error: McpError::Busy,
            ..
        }
    ));
    stop(&session).await;
    assert_eq!(
        peer.count(|request| request.method == HttpMethod::Delete),
        1
    );
}

#[tokio::test]
async fn j8_stalled_recovery_json_is_owned_and_bounded() {
    let (probe, body) = Probe::body();
    let mut peer = Peer::new();
    peer.replacement_json = true;
    *peer.replacement.lock().unwrap() = Some(body);
    let peer = Arc::new(peer);
    let (session, incoming) = transport(peer.clone(), Arc::default());
    let clock = Arc::new(ManualClock::default());
    let connection = Connection::open_http(session.clone(), incoming, clock.clone());
    bounded(connection.call("initialize", None))
        .await
        .unwrap()
        .unwrap();
    peer.expire.store(true, Ordering::SeqCst);
    assert_eq!(
        bounded(connection.call("tools/list", None))
            .await
            .unwrap_err(),
        McpError::SessionExpired
    );
    probe.polled(1).await;
    assert_eq!(
        bounded(connection.call("tools/list", None))
            .await
            .unwrap_err(),
        McpError::Busy
    );
    connection
        .notify("notifications/cancelled", None)
        .await
        .unwrap();
    peer.observed(|request| method(request).as_deref() == Some("notifications/cancelled"))
        .await;
    clock.advance(INITIALIZE_TIMEOUT);
    assert_eq!(bounded(connection.ended()).await, McpError::Timeout);
    stop(&session).await;
    probe.released().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn j2_json_capacity_is_retained_through_physical_destruction() {
    use super::post_body::{DropGate, ReleaseOnDrop};
    use std::sync::Condvar;
    let gate = Arc::new(DropGate {
        entered: Notify::new(),
        released: Mutex::new(false),
        ready: Condvar::new(),
    });
    let _release = ReleaseOnDrop(gate.clone());
    let (probe, body) = Probe::body();
    *probe.drop_gate.lock().unwrap() = Some(gate.clone());
    let peer = Arc::new(Peer::new());
    peer.bodies.lock().unwrap().push_back(body);
    let mut probes = vec![];
    for _ in 1..super::super::http::MAX_POST_STREAMS {
        let (probe, body) = Probe::body();
        peer.bodies.lock().unwrap().push_back(body);
        probes.push(probe);
    }
    let (session, _incoming) = transport(peer.clone(), Arc::default());
    for id in 1..=super::super::http::MAX_POST_STREAMS as u64 {
        assert!(matches!(call(&session, id).await, SendOutcome::Done));
    }
    probe.polled(1).await;
    session.cancel_post(1);
    bounded(gate.entered.notified()).await;
    let before = peer.count(|_| true);
    assert!(matches!(
        call(&session, 999).await,
        SendOutcome::FailCall {
            error: McpError::Busy,
            ..
        }
    ));
    assert_eq!(peer.count(|_| true), before);
    gate.release();
    probe.released().await;
    bounded(async {
        loop {
            match call(&session, 1000).await {
                SendOutcome::Done => break,
                SendOutcome::FailCall {
                    error: McpError::Busy,
                    ..
                } => tokio::task::yield_now().await,
                other => panic!("unexpected {other:?}"),
            }
        }
    })
    .await;
    stop(&session).await;
    for probe in probes {
        probe.released().await;
    }
}

#[tokio::test]
async fn j7_public_open_collision_while_delete_is_held_is_not_deleted() {
    let gate = Arc::new(Gate::default());
    let mut peer = Peer::new();
    peer.delete = Some(gate.clone());
    let peer = Arc::new(peer);
    let servers = McpServers::new(vec![], Arc::new(RuntimeClock::new())).unwrap();
    servers.set_remote_transport(peer.clone(), Arc::new(NoAuthorization));
    let remote =
        RemoteMcpServer::new(Uuid::from_u128(1), "remote", "http://127.0.0.1/mcp").unwrap();
    let session = servers.open_remote_once(&remote).await.unwrap();
    let close = tokio::spawn(async move { session.close().await });
    gate.reached().await;
    assert!(matches!(
        servers.open_remote_once(&remote).await,
        Err(McpError::SessionCollision)
    ));
    assert_eq!(
        peer.count(|request| request.method == HttpMethod::Delete),
        1
    );
    gate.release();
    bounded(close).await.unwrap();
    let reopened = servers.open_remote_once(&remote).await.unwrap();
    gate.release();
    reopened.close().await;
}

#[tokio::test]
async fn j2_complete_streamed_json_settles_and_releases_body() {
    let (probe, body) = Probe::body();
    probe
        .send
        .send(Ok(Some(br#"{"id":1,"result":{"tools":[]}}"#.to_vec())))
        .unwrap();
    probe.send.send(Ok(None)).unwrap();
    let peer = Arc::new(Peer::new());
    peer.bodies.lock().unwrap().push_back(body);
    let (session, incoming) = transport(peer, Arc::default());
    let connection =
        Connection::open_http(session.clone(), incoming, Arc::new(RuntimeClock::new()));
    assert_eq!(
        bounded(connection.call("tools/list", None))
            .await
            .unwrap()
            .unwrap(),
        json!({"tools":[]})
    );
    probe.released().await;
    stop(&session).await;
}

#[tokio::test]
async fn j5_drop_initial_reader_does_not_retain_session() {
    let (probe, body) = Probe::body();
    let peer = Arc::new(Peer::new());
    *peer.initial.lock().unwrap() = Some(body);
    let (session, _incoming) = transport(peer.clone(), Arc::default());
    initialize(&session).await;
    probe.polled(1).await;
    let mut done = session.finished();
    drop(session);
    bounded(done.wait_for(|done| *done)).await.unwrap();
    probe.released().await;
    assert_eq!(peer.count(|request| request.method != HttpMethod::Post), 0);
}

#[tokio::test]
async fn j4_initialize_terminal_precedes_bad_trailing_bytes() {
    for streamed in [false, true] {
        let mut bytes = format!(
            "data: {}\n\n",
            json!({"id":1,"result":{"protocolVersion":"2025-06-18"}})
        )
        .into_bytes();
        bytes.extend_from_slice(b"data: \xff\n\n");
        let (probe, stream) = Probe::body();
        let body = if streamed {
            probe.send.send(Ok(Some(bytes))).unwrap();
            stream
        } else {
            drop(stream);
            HttpBody::Buffered(bytes)
        };
        let peer = Arc::new(Peer::new());
        *peer.initial.lock().unwrap() = Some(body);
        let (session, mut incoming) = transport(peer.clone(), Arc::default());
        initialize(&session).await;
        let answer = bounded(incoming.recv()).await.unwrap().unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&answer).unwrap()["id"], 1);
        peer.observed(|request| request.method == HttpMethod::Get)
            .await;
        stop(&session).await;
        probe.released().await;
    }
}

fn complete_body(value: Value, sse: bool, streamed: bool) -> HttpBody {
    let bytes = if sse {
        format!("data: {value}\n\n").into_bytes()
    } else {
        serde_json::to_vec(&value).unwrap()
    };
    if !streamed {
        return HttpBody::Buffered(bytes);
    }
    // The sender closes after supplying EOF; the owned chunks remain gated substitutes.
    let (probe, body) = Probe::body();
    probe.send.send(Ok(Some(bytes))).unwrap();
    probe.send.send(Ok(None)).unwrap();
    body
}

#[tokio::test]
async fn j2_complete_body_without_own_terminal_settles_deadline_less_call() {
    for sse in [false, true] {
        for streamed in [false, true] {
            for value in [
                json!({"id":77,"result":{}}),
                json!({"id":1,"method":"ping"}),
                json!({"method":"notifications/tools/list_changed"}),
                json!({"jsonrpc":"2.0"}),
            ] {
                let mut peer = Peer::new();
                peer.call_sse = sse;
                peer.bodies
                    .lock()
                    .unwrap()
                    .push_back(complete_body(value, sse, streamed));
                let peer = Arc::new(peer);
                let (session, incoming) = transport(peer, Arc::default());
                let connection =
                    Connection::open_http(session.clone(), incoming, Arc::new(RuntimeClock::new()));
                assert_eq!(
                    bounded(connection.call("tools/list", None))
                        .await
                        .unwrap_err(),
                    McpError::Unconfirmed
                );
                stop(&session).await;
            }
        }
    }
}

#[tokio::test]
async fn j2_matching_json_result_and_error_remain_accepted() {
    for sse in [false, true] {
        for streamed in [false, true] {
            for error in [false, true] {
                let expected = if error {
                    Err(json!({"code":-32601,"message":"refused"}))
                } else {
                    Ok(json!({"tools":[]}))
                };
                let value = match &expected {
                    Ok(result) => json!({"id":1,"result":result}),
                    Err(error) => json!({"id":1,"error":error}),
                };
                let mut peer = Peer::new();
                peer.call_sse = sse;
                peer.bodies
                    .lock()
                    .unwrap()
                    .push_back(complete_body(value, sse, streamed));
                let (session, incoming) = transport(Arc::new(peer), Arc::default());
                let connection =
                    Connection::open_http(session.clone(), incoming, Arc::new(RuntimeClock::new()));
                assert_eq!(
                    bounded(connection.call("tools/list", None)).await.unwrap(),
                    expected
                );
                stop(&session).await;
            }
        }
    }
}

#[tokio::test]
async fn j2_neighbor_reply_is_preserved_before_unconfirmed_completion() {
    for sse in [false, true] {
        for streamed in [false, true] {
            let (probe, body) = Probe::body();
            let mut peer = Peer::new();
            peer.call_sse = sse;
            peer.bodies.lock().unwrap().extend([
                body,
                complete_body(json!({"id":1,"result":{"tools":[]}}), sse, streamed),
            ]);
            let (session, incoming) = transport(Arc::new(peer), Arc::default());
            let connection = Arc::new(Connection::open_http(
                session.clone(),
                incoming,
                Arc::new(RuntimeClock::new()),
            ));
            let first = tokio::spawn({
                let connection = connection.clone();
                async move { connection.call("tools/list", None).await }
            });
            probe.polled(1).await;
            assert_eq!(
                bounded(connection.call("tools/list", None))
                    .await
                    .unwrap_err(),
                McpError::Unconfirmed
            );
            assert_eq!(
                bounded(first).await.unwrap().unwrap().unwrap(),
                json!({"tools":[]})
            );
            stop(&session).await;
            probe.released().await;
        }
    }
}

#[tokio::test]
async fn j8_recovery_server_ping_reply_dispatches_before_initialize_result() {
    let (probe, body) = Probe::body();
    probe.event(json!({"id":70,"method":"ping"}));
    let peer = Arc::new(Peer::new());
    *peer.replacement.lock().unwrap() = Some(body);
    let (session, incoming) = transport(peer.clone(), Arc::default());
    let connection =
        Connection::open_http(session.clone(), incoming, Arc::new(RuntimeClock::new()));
    bounded(connection.call("initialize", None))
        .await
        .unwrap()
        .unwrap();
    peer.expire.store(true, Ordering::SeqCst);
    assert_eq!(
        bounded(connection.call("tools/list", None))
            .await
            .unwrap_err(),
        McpError::SessionExpired
    );
    peer.observed(|request| {
        serde_json::from_slice::<Value>(&request.body)
            .ok()
            .is_some_and(|message| message["id"] == 70 && message.get("result").is_some())
    })
    .await;
    assert_eq!(
        bounded(connection.call("tools/list", None))
            .await
            .unwrap_err(),
        McpError::Busy
    );
    stop(&session).await;
    probe.released().await;
}

#[tokio::test]
async fn j10_recovery_neighbor_reply_precedes_invalid_initialize() {
    for sse in [false, true] {
        for streamed in [false, true] {
            let (probe, held) = Probe::body();
            let mut peer = Peer::new();
            peer.replacement_json = !sse;
            *peer.replacement.lock().unwrap() = Some(complete_body(
                json!({"id":2,"result":{"tools":[]}}),
                sse,
                streamed,
            ));
            peer.bodies.lock().unwrap().push_back(held);
            let peer = Arc::new(peer);
            let (session, incoming) = transport(peer.clone(), Arc::default());
            let connection = Arc::new(Connection::open_http(
                session.clone(),
                incoming,
                Arc::new(RuntimeClock::new()),
            ));
            bounded(connection.call("initialize", None))
                .await
                .unwrap()
                .unwrap();
            let old = tokio::spawn({
                let connection = connection.clone();
                async move { connection.call("tools/list", None).await }
            });
            probe.polled(1).await;
            peer.expire.store(true, Ordering::SeqCst);
            assert_eq!(
                bounded(connection.call("tools/list", None))
                    .await
                    .unwrap_err(),
                McpError::SessionExpired
            );
            assert_eq!(
                bounded(old).await.unwrap().unwrap().unwrap(),
                json!({"tools":[]})
            );
            assert_eq!(
                peer.count(|request| header(request, "Mcp-Session-Id") == Some("replacement")),
                0
            );
            stop(&session).await;
            probe.released().await;
        }
    }
}

#[tokio::test]
async fn j11_accepted_initialized_at_expiry_reports_timeout() {
    let clock = Arc::new(ManualClock::default());
    let mut peer = Peer::new();
    peer.initialized_expiry = Some(clock.clone());
    let peer = Arc::new(peer);
    let (session, _incoming) = transport(peer.clone(), Arc::default());
    let (writer, mut queue) = mpsc::channel(1);
    session.set_writer(writer, clock);
    initialize(&session).await;
    peer.expire.store(true, Ordering::SeqCst);
    let _ = call(&session, 2).await;
    let Outgoing::RecoveryReady {
        deadline,
        completed,
    } = bounded(queue.recv()).await.unwrap()
    else {
        panic!("recovery handoff")
    };
    assert!(matches!(
        session.finish_recovery(deadline, completed).await,
        SendOutcome::End(McpError::Timeout)
    ));
    assert!(matches!(
        call(&session, 3).await,
        SendOutcome::FailCall {
            error: McpError::Busy,
            ..
        }
    ));
    stop(&session).await;
}

#[tokio::test]
async fn j11_completion_loss_preserves_writer_failure() {
    let mut peer = Peer::new();
    peer.initialized_status = 405;
    let peer = Arc::new(peer);
    let (session, mut incoming) = transport(peer.clone(), Arc::default());
    let (writer, mut queue) = mpsc::channel(1);
    session.set_writer(writer, Arc::new(RuntimeClock::new()));
    initialize(&session).await;
    peer.expire.store(true, Ordering::SeqCst);
    let _ = call(&session, 2).await;
    let Outgoing::RecoveryReady {
        deadline,
        completed,
    } = bounded(queue.recv()).await.unwrap()
    else {
        panic!("recovery handoff")
    };
    let SendOutcome::End(first) = session.finish_recovery(deadline, completed).await else {
        panic!("rejected initialized")
    };
    assert!(matches!(first, McpError::Malformed(_)));
    let (lost, receiver) = tokio::sync::oneshot::channel();
    drop(receiver);
    let SendOutcome::End(retried) = session.finish_recovery(deadline, lost).await else {
        panic!("retained first failure")
    };
    assert_eq!(retried, first);
    let retained = bounded(async {
        loop {
            if let Some(Err(error)) = incoming.recv().await {
                break error;
            }
        }
    })
    .await;
    assert_eq!(retained, first);
    stop(&session).await;
}

#[tokio::test]
async fn j12_lost_completion_cannot_admit_queued_call() {
    let gate = Arc::new(Gate::default());
    let mut peer = Peer::new();
    peer.initialized_headers = Some(gate.clone());
    let peer = Arc::new(peer);
    let (session, _incoming) = transport(peer.clone(), Arc::default());
    let (writer, mut queue) = mpsc::channel(2);
    session.set_writer(writer.clone(), Arc::new(RuntimeClock::new()));
    initialize(&session).await;
    peer.expire.store(true, Ordering::SeqCst);
    let _ = call(&session, 2).await;
    let Outgoing::RecoveryReady {
        deadline,
        completed: startup_sender,
    } = bounded(queue.recv()).await.unwrap()
    else {
        panic!("recovery handoff")
    };
    // Keep startup alive while the writer's independently controlled delivery
    // receiver disappears after dispatch began, before its terminal commit.
    let (completed, receiver) = tokio::sync::oneshot::channel();
    let finishing = tokio::spawn({
        let session = session.clone();
        async move { session.finish_recovery(deadline, completed).await }
    });
    gate.reached().await;
    writer
        .send(Outgoing::Frame(
            br#"{"id":3,"method":"tools/list"}"#.to_vec(),
        ))
        .await
        .unwrap();
    drop(receiver);
    gate.release();
    assert!(matches!(
        bounded(finishing).await.unwrap(),
        SendOutcome::End(McpError::Unconfirmed)
    ));
    let Outgoing::Frame(frame) = queue.recv().await.unwrap() else {
        panic!("queued ordinary call")
    };
    assert!(matches!(
        session.dispatch(&frame).await,
        SendOutcome::FailCall {
            error: McpError::Busy,
            ..
        }
    ));
    assert_eq!(
        peer.count(
            |request| header(request, "Mcp-Session-Id") == Some("replacement")
                && method(request).as_deref() == Some("tools/list")
        ),
        0
    );
    drop(startup_sender);
    stop(&session).await;
}

/// This clock finishes an already-ready HTTP dispatch during the timer poll,
/// after the startup operation was polled Pending. The timer then wins that
/// select even though completion was delivered before the clock advanced.
#[derive(Default)]
struct CommitBeforeExpiryClock {
    now: Arc<ManualClock>,
    trigger: Arc<Notify>,
    commit: Arc<Mutex<Option<ExpiryCommit>>>,
}
struct ExpiryCommit {
    session: std::sync::Weak<HttpSession>,
    deadline: ClockInstant,
    completed: tokio::sync::oneshot::Sender<Result<(), McpError>>,
    observed: tokio::sync::oneshot::Sender<SendOutcome>,
}
impl Clock for CommitBeforeExpiryClock {
    fn now(&self) -> ClockInstant {
        self.now.now()
    }
    fn sleep_until(&self, _: ClockInstant) -> ClockSleep {
        let trigger = self.trigger.clone();
        let commit = self.commit.clone();
        let clock = self.now.clone();
        Box::pin(async move {
            trigger.notified().await;
            let commit = commit.lock().unwrap().take().expect("armed expiry race");
            let session = commit.session.upgrade().unwrap();
            let mut finishing =
                Box::pin(session.finish_recovery(commit.deadline, commit.completed));
            let outcome = std::future::poll_fn(|context| {
                match std::future::Future::poll(finishing.as_mut(), context) {
                    std::task::Poll::Ready(outcome) => std::task::Poll::Ready(outcome),
                    std::task::Poll::Pending => panic!("immediate peer accepts initialized"),
                }
            })
            .await;
            clock.advance_to(commit.deadline);
            let _ = commit.observed.send(outcome);
        })
    }
}

#[tokio::test]
async fn j13_completed_commit_defeats_late_timeout() {
    let peer = Arc::new(Peer::new());
    let (session, mut incoming) = transport(peer.clone(), Arc::default());
    let clock = Arc::new(CommitBeforeExpiryClock::default());
    let (writer, mut queue) = mpsc::channel(1);
    session.set_writer(writer, clock.clone());
    initialize(&session).await;
    peer.expire.store(true, Ordering::SeqCst);
    let _ = call(&session, 2).await;
    let Outgoing::RecoveryReady {
        deadline,
        completed,
    } = bounded(queue.recv()).await.unwrap()
    else {
        panic!("recovery handoff")
    };
    let (observed, observation) = tokio::sync::oneshot::channel();
    *clock.commit.lock().unwrap() = Some(ExpiryCommit {
        session: Arc::downgrade(&session),
        deadline,
        completed,
        observed,
    });
    clock.trigger.notify_one();
    assert!(matches!(
        bounded(observation).await.unwrap(),
        SendOutcome::Done
    ));
    assert_eq!(clock.now(), deadline);
    // The startup timer won after delivery. Its late failure is suppressed,
    // and queued/public admission still sees the committed success.
    assert!(matches!(call(&session, 3).await, SendOutcome::Done));
    assert_eq!(
        peer.count(
            |request| header(request, "Mcp-Session-Id") == Some("replacement")
                && method(request).as_deref() == Some("tools/list")
        ),
        1
    );
    while let Ok(message) = incoming.try_recv() {
        assert!(message.is_ok());
    }
    stop(&session).await;
}

struct RetryAuthorization {
    rejected: AtomicUsize,
}
#[async_trait]
impl RemoteAuthorization for RetryAuthorization {
    async fn bearer(&self, _: Uuid) -> Result<Option<Bearer>, McpError> {
        Ok(Some(Bearer::new("first", 0)))
    }
    async fn rejected(&self, _: Uuid, _: &str) -> Result<Option<Bearer>, McpError> {
        self.rejected.fetch_add(1, Ordering::SeqCst);
        Ok(Some(Bearer::new("retry", 1)))
    }
    async fn insufficient_scope(&self, _: Uuid, _: &str) {}
}

#[tokio::test]
async fn j14_initialized_http_failures_keep_typed_causes() {
    for (status, expected) in [(503, McpError::Unconfirmed), (401, McpError::Unauthorized)] {
        let mut peer = Peer::new();
        peer.initialized_status = status;
        let peer = Arc::new(peer);
        let authorization = Arc::new(RetryAuthorization {
            rejected: AtomicUsize::new(0),
        });
        let (session, incoming) = HttpSession::open(
            Uuid::from_u128(1),
            RemoteMcpUrl::parse("http://127.0.0.1/mcp").unwrap(),
            peer.clone(),
            authorization.clone(),
            Arc::default(),
        );
        let connection =
            Connection::open_http(session.clone(), incoming, Arc::new(RuntimeClock::new()));
        bounded(connection.call("initialize", None))
            .await
            .unwrap()
            .unwrap();
        peer.expire.store(true, Ordering::SeqCst);
        assert_eq!(
            bounded(connection.call("tools/list", None))
                .await
                .unwrap_err(),
            McpError::SessionExpired
        );
        assert_eq!(bounded(connection.ended()).await, expected);
        assert_eq!(
            bounded(connection.call("tools/list", None))
                .await
                .unwrap_err(),
            expected
        );
        assert_eq!(
            authorization.rejected.load(Ordering::SeqCst),
            usize::from(status == 401)
        );
        assert_eq!(
            peer.count(
                |request| header(request, "Mcp-Session-Id") == Some("replacement")
                    && method(request).as_deref() == Some("notifications/initialized")
            ),
            if status == 401 { 2 } else { 1 }
        );
        stop(&session).await;
    }
}
