//! ADR 392 J1–J26: JSON/initialization progress and replacement cleanup ownership.
use super::super::connection::{Connection, Outgoing, OutgoingQueue, Reply, OUTGOING_FRAMES};
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
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::task::Poll;
use std::time::Duration;
use tokio::sync::{
    mpsc::{self, error::TrySendError},
    Notify, Semaphore,
};
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
    controls: Mutex<VecDeque<(u16, Option<Arc<Gate>>)>>,
    initialized_expiry: Option<Arc<ManualClock>>,
    replacement_json: bool,
    call_sse: bool,
    recovery_statuses: Mutex<VecDeque<u16>>,
    initial_id: Option<String>,
    replacement_id: Option<String>,
    ordinary_response_id: Option<String>,
    recovery_exchange_failure: AtomicBool,
    control_panic: AtomicBool,
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
            controls: Mutex::new(VecDeque::new()),
            initialized_expiry: None,
            replacement_json: false,
            call_sse: false,
            recovery_statuses: Mutex::default(),
            initial_id: Some("initial".into()),
            replacement_id: Some("replacement".into()),
            ordinary_response_id: None,
            recovery_exchange_failure: AtomicBool::new(false),
            control_panic: AtomicBool::new(false),
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
                    if recovery {
                        if self.recovery_exchange_failure.swap(false, Ordering::SeqCst) {
                            return Err(HttpFailure::Unreachable);
                        }
                        if let Some(status) = self.recovery_statuses.lock().unwrap().pop_front() {
                            if status != 200 {
                                return Ok(HttpResponse {
                                    status,
                                    headers: vec![(
                                        "www-authenticate".into(),
                                        "Bearer scope=tools".into(),
                                    )],
                                    body: HttpBody::Buffered(vec![]),
                                });
                            }
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
                    if let Some(id) = if recovery {
                        &self.replacement_id
                    } else {
                        &self.initial_id
                    } {
                        response.headers.push(("mcp-session-id".into(), id.clone()));
                    }
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
                    let control = if method(&request).as_deref() == Some("notifications/cancelled")
                        || message.get("method").is_none()
                    {
                        self.controls.lock().unwrap().pop_front()
                    } else {
                        None
                    };
                    if let Some((status, gate)) = control {
                        if let Some(gate) = gate {
                            gate.enter().await;
                        }
                        assert!(
                            !self.control_panic.swap(false, Ordering::SeqCst),
                            "injected writer exchange panic"
                        );
                        return Ok(HttpResponse {
                            status,
                            headers: vec![],
                            body: HttpBody::Buffered(vec![]),
                        });
                    }
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
                if let Some(id) = &self.ordinary_response_id {
                    response.headers.push(("mcp-session-id".into(), id.clone()));
                }
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
) -> (
    Arc<HttpSession>,
    mpsc::Receiver<Result<super::super::http::HttpMessage, McpError>>,
) {
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
    let answer = peer
        .seen
        .lock()
        .unwrap()
        .iter()
        .find(|request| {
            serde_json::from_slice::<Value>(&request.body)
                .ok()
                .is_some_and(|message| message["id"] == 100)
        })
        .unwrap()
        .clone();
    assert_eq!(header(&answer, "Mcp-Session-Id"), Some("initial"));
    assert_eq!(header(&answer, "MCP-Protocol-Version"), None);
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
    let (writer, mut queue) = OutgoingQueue::new(1, 0);
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
    let (writer, mut queue) = OutgoingQueue::new(1, 0);
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
    let (writer, mut queue) = OutgoingQueue::new(1, 0);
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
    let (writer, mut queue) = OutgoingQueue::new(1, 0);
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
    let (writer, mut queue) = OutgoingQueue::new(1, 0);
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
    let answer = peer
        .seen
        .lock()
        .unwrap()
        .iter()
        .find(|request| {
            serde_json::from_slice::<Value>(&request.body)
                .ok()
                .is_some_and(|message| message["id"] == 70)
        })
        .unwrap()
        .clone();
    assert_eq!(header(&answer, "Mcp-Session-Id"), Some("replacement"));
    assert_eq!(header(&answer, "MCP-Protocol-Version"), None);
    assert_eq!(peer.count(|request| request.method == HttpMethod::Get), 1);
    assert_eq!(
        bounded(connection.call("tools/list", None))
            .await
            .unwrap_err(),
        McpError::Busy
    );
    assert_eq!(
        peer.count(|r| method(r).as_deref() == Some("notifications/initialized")),
        0
    );
    probe.event(json!({"id":0,"result":{"protocolVersion":"2025-06-18"}}));
    peer.observed(|r| method(r).as_deref() == Some("notifications/initialized"))
        .await;
    bounded(connection.call("tools/list", None))
        .await
        .unwrap()
        .unwrap();
    peer.observed(|r| {
        r.method == HttpMethod::Get && header(r, "Mcp-Session-Id") == Some("replacement")
    })
    .await;
    assert_eq!(peer.count(|r| r.method == HttpMethod::Get), 2);
    assert_eq!(
        peer.count(|r| method(r).as_deref() == Some("initialize")),
        2
    );
    assert_eq!(
        peer.count(|r| method(r).as_deref() == Some("notifications/initialized")),
        1
    );
    assert_eq!(
        peer.count(|r| method(r).as_deref() == Some("tools/list")),
        2
    );
    stop(&session).await;
    assert_eq!(peer.count(|r| r.method == HttpMethod::Delete), 1);
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
    let (writer, mut queue) = OutgoingQueue::new(1, 0);
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
    let (writer, mut queue) = OutgoingQueue::new(1, 0);
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
    let (writer, mut queue) = OutgoingQueue::new(2, 0);
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
    let (writer, mut queue) = OutgoingQueue::new(1, 0);
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
    for (status, expected) in [
        (503, McpError::Unconfirmed),
        (401, McpError::Unauthorized),
        (404, McpError::SessionExpired),
    ] {
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
        assert_eq!(
            peer.count(|request| method(request).as_deref() == Some("initialize")),
            2
        );
        assert_eq!(
            peer.count(|request| method(request).as_deref() == Some("tools/list")),
            1
        );
        stop(&session).await;
    }
}

/// Each mode drives the same actual exchange: public serialized writer and
/// direct dispatch seam (which additionally exposes the control's typed outcome).
async fn control_context_ordering(retry: bool, public_writer: bool) {
    let gate = Arc::new(Gate::default());
    let (probe, body) = Probe::body();
    let peer = Peer::new();
    *peer.replacement.lock().unwrap() = Some(body);
    peer.controls
        .lock()
        .unwrap()
        .push_back((if retry { 401 } else { 404 }, Some(gate.clone())));
    if retry {
        peer.controls.lock().unwrap().push_back((404, None));
    }
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
    let connection = if public_writer {
        Some(Connection::open_http(
            session.clone(),
            incoming,
            Arc::new(RuntimeClock::new()),
        ))
    } else {
        None
    };
    let (writer, mut queue) = OutgoingQueue::new(2, 0);
    if let Some(connection) = &connection {
        bounded(connection.call("initialize", None))
            .await
            .unwrap()
            .unwrap();
    } else {
        session.set_writer(writer, Arc::new(RuntimeClock::new()));
        initialize(&session).await;
    }
    peer.expire.store(true, Ordering::SeqCst);
    if let Some(connection) = &connection {
        assert_eq!(
            bounded(connection.call("tools/call", None))
                .await
                .unwrap_err(),
            McpError::SessionExpired
        );
    } else {
        assert!(matches!(
            session.dispatch(br#"{"id":2,"method":"tools/call"}"#).await,
            SendOutcome::FailCall {
                error: McpError::SessionExpired,
                ..
            }
        ));
    }
    probe.polled(1).await;
    let direct = if let Some(connection) = &connection {
        connection
            .notify("notifications/cancelled", Some(json!({"requestId":2})))
            .await
            .unwrap();
        None
    } else {
        Some(tokio::spawn({
            let session = session.clone();
            async move {
                session
                    .dispatch(br#"{"method":"notifications/cancelled","params":{"requestId":2}}"#)
                    .await
            }
        }))
    };
    gate.reached().await;
    assert_eq!(
        peer.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|request| method(request).as_deref() == Some("notifications/cancelled"))
            .count(),
        1
    );
    assert!(peer
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|request| method(request).as_deref() == Some("notifications/cancelled"))
        .all(|request| header(request, "Mcp-Session-Id").is_none()));
    probe.event(json!({"id":0,"result":{"protocolVersion":"2025-06-18"}}));
    peer.observed(|request| {
        request.method == HttpMethod::Get
            && header(request, "Mcp-Session-Id") == Some("replacement")
    })
    .await;
    gate.release();
    if let Some(direct) = direct {
        let outcome = bounded(direct).await.unwrap();
        if retry {
            assert!(matches!(
                outcome,
                SendOutcome::End(McpError::SessionExpired)
            ));
        } else {
            assert!(
                matches!(outcome, SendOutcome::FailCall { id: None, error: McpError::Malformed(ref text) } if text == "HTTP 404")
            );
        }
        let Outgoing::RecoveryReady {
            deadline,
            completed,
        } = bounded(queue.recv()).await.unwrap()
        else {
            panic!("queued recovery handoff")
        };
        if retry {
            drop(completed);
        } else {
            assert!(matches!(
                session.finish_recovery(deadline, completed).await,
                SendOutcome::Done
            ));
            assert!(matches!(call(&session, 3).await, SendOutcome::Done));
        }
    } else if retry {
        let connection = connection.as_ref().unwrap();
        assert_eq!(bounded(connection.ended()).await, McpError::SessionExpired);
        assert_eq!(
            bounded(connection.call("tools/list", None))
                .await
                .unwrap_err(),
            McpError::SessionExpired
        );
    } else {
        peer.observed(|request| method(request).as_deref() == Some("notifications/initialized"))
            .await;
        assert_eq!(
            bounded(connection.as_ref().unwrap().call("tools/list", None))
                .await
                .unwrap()
                .unwrap(),
            json!({"tools":[]})
        );
    }
    {
        let requests = peer.seen.lock().unwrap();
        let controls: Vec<_> = requests
            .iter()
            .filter(|request| method(request).as_deref() == Some("notifications/cancelled"))
            .collect();
        assert_eq!(controls.len(), if retry { 2 } else { 1 });
        assert!(header(controls[0], "Mcp-Session-Id").is_none());
        if retry {
            assert_eq!(header(controls[1], "Mcp-Session-Id"), Some("replacement"));
            assert_eq!(header(controls[1], "Authorization"), Some("Bearer retry"));
        }
        assert_eq!(
            requests
                .iter()
                .filter(|request| method(request).as_deref() == Some("initialize"))
                .count(),
            2
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| method(request).as_deref() == Some("tools/call"))
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| method(request).as_deref() == Some("notifications/initialized"))
                .count(),
            usize::from(!retry)
        );
    }
    assert_eq!(
        authorization.rejected.load(Ordering::SeqCst),
        usize::from(retry)
    );
    stop(&session).await;
    probe.released().await;
}

#[tokio::test]
async fn j15_control_response_uses_its_actual_request_context() {
    for public_writer in [false, true] {
        control_context_ordering(false, public_writer).await;
    }
}

#[tokio::test]
async fn j16_refreshed_control_retry_uses_its_own_bound_context() {
    for public_writer in [false, true] {
        control_context_ordering(true, public_writer).await;
    }
}

struct MatrixAuthorization {
    retry: bool,
    rejected: AtomicUsize,
    scope: Mutex<Vec<String>>,
}
#[async_trait]
impl RemoteAuthorization for MatrixAuthorization {
    async fn bearer(&self, _: Uuid) -> Result<Option<Bearer>, McpError> {
        Ok(Some(Bearer::new("first", 0)))
    }
    async fn rejected(&self, _: Uuid, challenge: &str) -> Result<Option<Bearer>, McpError> {
        assert_eq!(challenge, "Bearer scope=tools");
        self.rejected.fetch_add(1, Ordering::SeqCst);
        Ok(self.retry.then(|| Bearer::new("retry", 1)))
    }
    async fn insufficient_scope(&self, _: Uuid, challenge: &str) {
        self.scope.lock().unwrap().push(challenge.to_owned());
    }
}

#[tokio::test]
async fn j20_private_initialize_preserves_shared_authorization_and_status_policy() {
    for (statuses, retry, expected) in [
        (vec![401], false, McpError::Unauthorized),
        (vec![401, 401], true, McpError::Unauthorized),
        (vec![403], true, McpError::InsufficientScope),
        (vec![401, 403], true, McpError::InsufficientScope),
        (vec![503], true, McpError::Unreachable),
        (vec![401, 503], true, McpError::Unreachable),
        (vec![400], true, McpError::Malformed("HTTP 400".into())),
        (vec![404], true, McpError::Malformed("HTTP 404".into())),
        (vec![405], true, McpError::Malformed("HTTP 405".into())),
        (vec![202], true, McpError::SessionExpired),
    ] {
        let peer = Arc::new(Peer::new());
        *peer.recovery_statuses.lock().unwrap() = statuses.clone().into();
        let authorization = Arc::new(MatrixAuthorization {
            retry,
            rejected: AtomicUsize::new(0),
            scope: Mutex::default(),
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
        assert_eq!(
            bounded(connection.ended()).await,
            expected,
            "statuses={statuses:?}"
        );
        assert_eq!(
            bounded(connection.call("tools/list", None))
                .await
                .unwrap_err(),
            expected
        );
        assert_eq!(
            authorization.rejected.load(Ordering::SeqCst),
            usize::from(statuses[0] == 401)
        );
        assert_eq!(
            *authorization.scope.lock().unwrap(),
            if statuses.contains(&403) {
                vec!["Bearer scope=tools".to_owned()]
            } else {
                vec![]
            }
        );
        assert_eq!(
            peer.count(|r| method(r).as_deref() == Some("initialize")),
            1 + statuses.len()
        );
        assert_eq!(peer.count(|r| r.method == HttpMethod::Get), 1);
        assert_eq!(
            peer.count(|r| method(r).as_deref() == Some("notifications/initialized")),
            0
        );
        assert_eq!(
            peer.count(|r| method(r).as_deref() == Some("tools/list")),
            1
        );
        for request in peer
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter(|r| method(r).as_deref() == Some("initialize"))
        {
            assert_eq!(header(request, "Mcp-Session-Id"), None);
            assert_eq!(header(request, "MCP-Protocol-Version"), None);
        }
        stop(&session).await;
    }
}

#[tokio::test]
async fn j17_initial_peer_reply_keeps_response_binding_before_validation() {
    for (session_id, request_id, peer_method) in [
        (Some("initial".to_owned()), json!(70), "ping"),
        (None, json!("peer-string-id"), "unsupported/method"),
        (Some("x".repeat(1024)), json!(71), "ping"),
    ] {
        let (probe, body) = Probe::body();
        probe.event(json!({"id":request_id,"method":peer_method}));
        let mut peer = Peer::new();
        peer.initial_id = session_id.clone();
        *peer.initial.lock().unwrap() = Some(body);
        let peer = Arc::new(peer);
        let (session, incoming) = transport(peer.clone(), Arc::default());
        let connection = Arc::new(Connection::open_http(
            session.clone(),
            incoming,
            Arc::new(RuntimeClock::new()),
        ));
        let opening = tokio::spawn({
            let connection = connection.clone();
            async move { connection.call("initialize", None).await }
        });
        peer.observed(|r| {
            serde_json::from_slice::<Value>(&r.body)
                .ok()
                .is_some_and(|m| m["id"] == request_id && m.get("method").is_none())
        })
        .await;
        let answer = peer
            .seen
            .lock()
            .unwrap()
            .iter()
            .find(|r| {
                serde_json::from_slice::<Value>(&r.body)
                    .ok()
                    .is_some_and(|m| m["id"] == request_id)
            })
            .unwrap()
            .clone();
        assert_eq!(header(&answer, "Mcp-Session-Id"), session_id.as_deref());
        assert_eq!(header(&answer, "MCP-Protocol-Version"), None);
        assert_eq!(peer.count(|r| r.method == HttpMethod::Get), 0);
        assert_eq!(
            peer.count(|r| method(r).as_deref() == Some("notifications/initialized")),
            0
        );
        probe.event(json!({"id":1,"result":{"protocolVersion":"2025-06-18"}}));
        bounded(opening).await.unwrap().unwrap().unwrap();
        if session_id.is_some() {
            peer.observed(|r| r.method == HttpMethod::Get).await;
        }
        assert!(bounded(connection.call("tools/list", None)).await.is_ok());
        stop(&session).await;
        assert_eq!(
            peer.count(|r| r.method == HttpMethod::Delete),
            usize::from(session_id.is_some())
        );
        probe.released().await;
    }
}

#[tokio::test]
async fn j18_early_peer_claim_refuses_collision_and_oversize_before_answer() {
    for collision in [false, true] {
        let (probe, body) = Probe::body();
        probe.event(json!({"id":70,"method":"ping"}));
        let mut peer = Peer::new();
        peer.initial_id = Some(if collision {
            "occupied".into()
        } else {
            "x".repeat(1025)
        });
        *peer.initial.lock().unwrap() = Some(body);
        let peer = Arc::new(peer);
        let claims = Arc::new(SessionClaims::default());
        if collision {
            assert!(claims.claim("http://127.0.0.1/mcp", "occupied", u64::MAX));
        }
        let (session, incoming) = transport(peer.clone(), claims.clone());
        let connection =
            Connection::open_http(session.clone(), incoming, Arc::new(RuntimeClock::new()));
        assert_eq!(
            bounded(connection.call("initialize", None))
                .await
                .unwrap_err(),
            if collision {
                McpError::SessionCollision
            } else {
                McpError::TooLarge("Mcp-Session-Id")
            }
        );
        assert_eq!(
            peer.count(|r| serde_json::from_slice::<Value>(&r.body)
                .ok()
                .is_some_and(|m| m["id"] == 70)),
            0
        );
        assert_eq!(peer.count(|r| r.method == HttpMethod::Get), 0);
        stop(&session).await;
        assert_eq!(peer.count(|r| r.method == HttpMethod::Delete), 0);
        if collision {
            assert!(!claims.claim("http://127.0.0.1/mcp", "occupied", 0));
        }
        probe.released().await;
    }
}

#[tokio::test]
async fn j19_peer_answer_404_ends_without_recovery_or_replay() {
    for bound in [true, false] {
        let (probe, body) = Probe::body();
        probe.event(json!({"id":70,"method":"ping"}));
        let mut peer = Peer::new();
        if !bound {
            peer.initial_id = None;
        }
        let peer = Arc::new(peer);
        let expected = if bound {
            McpError::SessionExpired
        } else {
            McpError::Malformed("HTTP 404".into())
        };
        *peer.initial.lock().unwrap() = Some(body);
        peer.controls.lock().unwrap().push_back((404, None));
        let (session, incoming) = transport(peer.clone(), Arc::default());
        let connection =
            Connection::open_http(session.clone(), incoming, Arc::new(RuntimeClock::new()));
        assert_eq!(
            bounded(connection.call("initialize", None))
                .await
                .unwrap_err(),
            expected
        );
        assert_eq!(bounded(connection.ended()).await, expected);
        assert!(matches!(
            session
                .dispatch(br#"{"method":"notifications/cancelled"}"#)
                .await,
            SendOutcome::End(McpError::Closed)
        ));
        assert_eq!(connection.end_cause(), Some(expected));
        assert_eq!(
            peer.count(|r| method(r).as_deref() == Some("initialize")),
            1
        );
        assert_eq!(peer.count(|r| r.method == HttpMethod::Get), 0);
        stop(&session).await;
        assert_eq!(
            peer.count(|r| r.method == HttpMethod::Delete),
            usize::from(bound)
        );
        probe.released().await;
    }
}

#[tokio::test]
async fn j18_provisional_claim_is_retained_through_close_delete() {
    let (probe, body) = Probe::body();
    probe.event(json!({"id":70,"method":"ping"}));
    let gate = Arc::new(Gate::default());
    let mut peer = Peer::new();
    peer.delete = Some(gate.clone());
    *peer.initial.lock().unwrap() = Some(body);
    let peer = Arc::new(peer);
    let claims = Arc::new(SessionClaims::default());
    let (session, incoming) = transport(peer.clone(), claims.clone());
    let connection = Arc::new(Connection::open_http(
        session.clone(),
        incoming,
        Arc::new(RuntimeClock::new()),
    ));
    let opening = tokio::spawn({
        let connection = connection.clone();
        async move { connection.call("initialize", None).await }
    });
    peer.observed(|r| {
        serde_json::from_slice::<Value>(&r.body)
            .ok()
            .is_some_and(|m| m["id"] == 70)
    })
    .await;
    probe.event(json!({"id":1,"result":{"protocolVersion":"invalid"}}));
    assert!(matches!(
        bounded(opening).await.unwrap(),
        Err(McpError::Handshake(_))
    ));
    let mut done = session.finished();
    connection.close(McpError::Closed);
    gate.reached().await;
    session.shutdown();
    assert!(!*done.borrow());
    assert!(!claims.claim("http://127.0.0.1/mcp", "initial", u64::MAX));
    assert_eq!(peer.count(|r| r.method == HttpMethod::Delete), 1);
    assert_eq!(peer.count(|r| r.method == HttpMethod::Get), 0);
    gate.release();
    bounded(done.wait_for(|done| *done)).await.unwrap();
    assert!(claims.claim("http://127.0.0.1/mcp", "initial", u64::MAX));
    probe.released().await;
}

#[tokio::test]
async fn j18_public_open_error_closes_provisional_binding() {
    let (probe, body) = Probe::body();
    probe.event(json!({"id":70,"method":"ping"}));
    let gate = Arc::new(Gate::default());
    let mut peer = Peer::new();
    peer.delete = Some(gate.clone());
    *peer.initial.lock().unwrap() = Some(body);
    let peer = Arc::new(peer);
    let servers = McpServers::new(vec![], Arc::new(RuntimeClock::new())).unwrap();
    servers.set_remote_transport(peer.clone(), Arc::new(NoAuthorization));
    let opening = tokio::spawn(async move {
        servers
            .open_remote_once(
                &RemoteMcpServer::new(Uuid::from_u128(1), "remote", "http://127.0.0.1/mcp")
                    .unwrap(),
            )
            .await
    });
    peer.observed(|r| {
        serde_json::from_slice::<Value>(&r.body)
            .ok()
            .is_some_and(|m| m["id"] == 70)
    })
    .await;
    probe.event(json!({"id":1,"result":{"protocolVersion":"invalid"}}));
    assert!(matches!(
        bounded(opening).await.unwrap(),
        Err(McpError::Handshake(_))
    ));
    gate.reached().await;
    assert_eq!(peer.count(|r| r.method == HttpMethod::Delete), 1);
    assert_eq!(peer.count(|r| r.method == HttpMethod::Get), 0);
    gate.release();
    probe.released().await;
}

#[tokio::test]
async fn j19_queued_reply_keeps_unnegotiated_version_and_is_refused_after_close() {
    let (probe, body) = Probe::body();
    probe.event(json!({"id":70,"method":"ping"}));
    let peer = Arc::new(Peer::new());
    *peer.initial.lock().unwrap() = Some(body);
    let (session, mut incoming) = transport(peer.clone(), Arc::default());
    initialize(&session).await;
    let request = bounded(incoming.recv()).await.unwrap().unwrap();
    let context = request.reply.unwrap();
    probe.event(json!({"id":1,"result":{"protocolVersion":"2025-06-18"}}));
    bounded(incoming.recv()).await.unwrap().unwrap();
    let answer = br#"{"id":70,"result":{}}"#;
    assert!(matches!(
        session.dispatch_reply(answer, context.clone()).await,
        SendOutcome::Done
    ));
    let sent = peer
        .seen
        .lock()
        .unwrap()
        .iter()
        .find(|r| {
            serde_json::from_slice::<Value>(&r.body)
                .ok()
                .is_some_and(|m| m["id"] == 70)
        })
        .unwrap()
        .clone();
    assert_eq!(header(&sent, "Mcp-Session-Id"), Some("initial"));
    assert_eq!(header(&sent, "MCP-Protocol-Version"), None);
    stop(&session).await;
    assert!(matches!(
        session.dispatch_reply(answer, context).await,
        SendOutcome::End(McpError::Closed)
    ));
    assert_eq!(
        peer.count(|r| serde_json::from_slice::<Value>(&r.body)
            .ok()
            .is_some_and(|m| m["id"] == 70)),
        1
    );
    probe.released().await;
}

#[tokio::test]
async fn j20_private_initialize_healthy_and_retry_success_controls() {
    for retry in [false, true] {
        let peer = Arc::new(Peer::new());
        *peer.recovery_statuses.lock().unwrap() =
            if retry { vec![401, 200] } else { vec![200] }.into();
        let authorization = Arc::new(MatrixAuthorization {
            retry,
            rejected: AtomicUsize::new(0),
            scope: Mutex::default(),
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
        peer.observed(|r| method(r).as_deref() == Some("notifications/initialized"))
            .await;
        bounded(connection.call("tools/list", None))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            authorization.rejected.load(Ordering::SeqCst),
            usize::from(retry)
        );
        assert_eq!(
            peer.count(|r| method(r).as_deref() == Some("initialize")),
            if retry { 3 } else { 2 }
        );
        assert_eq!(
            peer.count(|r| method(r).as_deref() == Some("tools/list")),
            2
        );
        if retry {
            let requests = peer.seen.lock().unwrap();
            let refreshed = requests
                .iter()
                .rfind(|r| method(r).as_deref() == Some("initialize"))
                .unwrap();
            assert_eq!(header(refreshed, "Authorization"), Some("Bearer retry"));
            assert_eq!(header(refreshed, "Mcp-Session-Id"), None);
            assert_eq!(header(refreshed, "MCP-Protocol-Version"), None);
        }
        stop(&session).await;
    }
}

#[tokio::test]
async fn j19_old_reply_cannot_adopt_reused_or_stateless_replacement_binding() {
    for old_stateless in [false, true] {
        let (probe, body) = Probe::body();
        probe.event(json!({"id":70,"method":"ping"}));
        let mut peer = Peer::new();
        peer.call_sse = true;
        peer.replacement_id = if old_stateless {
            None
        } else {
            Some("initial".into())
        };
        peer.bodies.lock().unwrap().push_back(body);
        let peer = Arc::new(peer);
        let (session, mut incoming) = transport(peer.clone(), Arc::default());
        let (writer, mut queue) = OutgoingQueue::new(8, 0);
        session.set_writer(writer, Arc::new(RuntimeClock::new()));
        if !old_stateless {
            initialize(&session).await;
            bounded(incoming.recv()).await.unwrap().unwrap();
        }
        assert!(matches!(call(&session, 10).await, SendOutcome::Done));
        let request = bounded(incoming.recv()).await.unwrap().unwrap();
        let context = request.reply.unwrap();
        if old_stateless {
            initialize(&session).await;
            bounded(incoming.recv()).await.unwrap().unwrap();
        }
        peer.expire.store(true, Ordering::SeqCst);
        assert!(matches!(
            call(&session, 11).await,
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
            panic!("missing handoff")
        };
        assert!(matches!(
            session.finish_recovery(deadline, completed).await,
            SendOutcome::Done
        ));
        assert!(matches!(
            session
                .dispatch_reply(br#"{"id":70,"result":{}}"#, context)
                .await,
            SendOutcome::End(McpError::SessionExpired)
        ));
        assert_eq!(
            peer.count(|r| serde_json::from_slice::<Value>(&r.body)
                .ok()
                .is_some_and(|m| m["id"] == 70)),
            0
        );
        assert!(matches!(call(&session, 12).await, SendOutcome::Done));
        stop(&session).await;
        probe.released().await;
    }
}

#[tokio::test]
async fn j17_ordinary_body_reply_keeps_request_binding_and_ignores_returned_identity() {
    for returned_id in ["other-session".to_owned(), "x".repeat(1025)] {
        let (probe, body) = Probe::body();
        probe.event(json!({"id":70,"method":"ping"}));
        let mut peer = Peer::new();
        peer.ordinary_response_id = Some(returned_id);
        peer.call_sse = true;
        peer.bodies.lock().unwrap().push_back(body);
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
        let call = tokio::spawn({
            let connection = connection.clone();
            async move { connection.call("tools/list", None).await }
        });
        peer.observed(|r| {
            serde_json::from_slice::<Value>(&r.body)
                .ok()
                .is_some_and(|m| m["id"] == 70)
        })
        .await;
        let answer = peer
            .seen
            .lock()
            .unwrap()
            .iter()
            .find(|r| {
                serde_json::from_slice::<Value>(&r.body)
                    .ok()
                    .is_some_and(|m| m["id"] == 70)
            })
            .unwrap()
            .clone();
        assert_eq!(header(&answer, "Mcp-Session-Id"), Some("initial"));
        assert_eq!(header(&answer, "MCP-Protocol-Version"), Some("2025-06-18"));
        probe.event(json!({"id":2,"result":{"tools":[]}}));
        bounded(call).await.unwrap().unwrap().unwrap();
        stop(&session).await;
        probe.released().await;
    }
}

#[tokio::test]
async fn j20_private_initialize_exchange_failure_is_unreachable() {
    let peer = Arc::new(Peer::new());
    peer.recovery_exchange_failure.store(true, Ordering::SeqCst);
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
    assert_eq!(bounded(connection.ended()).await, McpError::Unreachable);
    assert_eq!(
        bounded(connection.call("tools/list", None))
            .await
            .unwrap_err(),
        McpError::Unreachable
    );
    assert_eq!(
        peer.count(|r| method(r).as_deref() == Some("initialize")),
        2
    );
    assert_eq!(peer.count(|r| r.method == HttpMethod::Get), 1);
    assert_eq!(
        peer.count(|r| method(r).as_deref() == Some("notifications/initialized")),
        0
    );
    stop(&session).await;
}

struct ReplyGateAuthorization {
    bearer_calls: AtomicUsize,
    gate: Arc<Gate>,
}
#[async_trait]
impl RemoteAuthorization for ReplyGateAuthorization {
    async fn bearer(&self, _: Uuid) -> Result<Option<Bearer>, McpError> {
        if self.bearer_calls.fetch_add(1, Ordering::SeqCst) == 1 {
            self.gate.enter().await;
        }
        Ok(None)
    }
    async fn rejected(&self, _: Uuid, _: &str) -> Result<Option<Bearer>, McpError> {
        Ok(None)
    }
    async fn insufficient_scope(&self, _: Uuid, _: &str) {}
}

#[tokio::test]
async fn j19_close_during_reply_authorization_refuses_post_effect() {
    let (probe, body) = Probe::body();
    probe.event(json!({"id":70,"method":"ping"}));
    let peer = Arc::new(Peer::new());
    *peer.initial.lock().unwrap() = Some(body);
    let gate = Arc::new(Gate::default());
    let authorization = Arc::new(ReplyGateAuthorization {
        bearer_calls: AtomicUsize::new(0),
        gate: gate.clone(),
    });
    let (session, mut incoming) = HttpSession::open(
        Uuid::from_u128(1),
        RemoteMcpUrl::parse("http://127.0.0.1/mcp").unwrap(),
        peer.clone(),
        authorization,
        Arc::default(),
    );
    initialize(&session).await;
    let context = bounded(incoming.recv())
        .await
        .unwrap()
        .unwrap()
        .reply
        .unwrap();
    let sending = tokio::spawn({
        let session = session.clone();
        async move {
            session
                .dispatch_reply(br#"{"id":70,"result":{}}"#, context)
                .await
        }
    });
    gate.reached().await;
    session.shutdown();
    gate.release();
    assert!(matches!(
        bounded(sending).await.unwrap(),
        SendOutcome::End(McpError::Closed)
    ));
    assert_eq!(
        peer.count(|r| serde_json::from_slice::<Value>(&r.body)
            .ok()
            .is_some_and(|m| m["id"] == 70)),
        0
    );
    stop(&session).await;
    assert_eq!(peer.count(|r| r.method == HttpMethod::Delete), 1);
    probe.released().await;
}

struct ReplyRetryAuthorization {
    gate: Arc<Gate>,
}
#[async_trait]
impl RemoteAuthorization for ReplyRetryAuthorization {
    async fn bearer(&self, _: Uuid) -> Result<Option<Bearer>, McpError> {
        Ok(None)
    }
    async fn rejected(&self, _: Uuid, _: &str) -> Result<Option<Bearer>, McpError> {
        self.gate.enter().await;
        Ok(Some(Bearer::new("retry", 1)))
    }
    async fn insufficient_scope(&self, _: Uuid, _: &str) {}
}

#[tokio::test]
async fn j19_reply_retry_cannot_adopt_identity_published_during_authorization() {
    let (probe, body) = Probe::body();
    probe.event(json!({"id":70,"method":"ping"}));
    let mut peer = Peer::new();
    peer.call_sse = true;
    let peer = Arc::new(peer);
    peer.bodies.lock().unwrap().push_back(body);
    peer.controls.lock().unwrap().push_back((401, None));
    let gate = Arc::new(Gate::default());
    let (session, mut incoming) = HttpSession::open(
        Uuid::from_u128(1),
        RemoteMcpUrl::parse("http://127.0.0.1/mcp").unwrap(),
        peer.clone(),
        Arc::new(ReplyRetryAuthorization { gate: gate.clone() }),
        Arc::default(),
    );
    let (writer, mut queue) = OutgoingQueue::new(8, 0);
    session.set_writer(writer, Arc::new(RuntimeClock::new()));
    initialize(&session).await;
    bounded(incoming.recv()).await.unwrap().unwrap();
    assert!(matches!(call(&session, 10).await, SendOutcome::Done));
    let context = bounded(incoming.recv())
        .await
        .unwrap()
        .unwrap()
        .reply
        .unwrap();
    let sending = tokio::spawn({
        let session = session.clone();
        async move {
            session
                .dispatch_reply(br#"{"id":70,"result":{}}"#, context)
                .await
        }
    });
    gate.reached().await;
    peer.expire.store(true, Ordering::SeqCst);
    assert!(matches!(
        call(&session, 11).await,
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
        panic!("missing handoff")
    };
    assert!(matches!(
        session.finish_recovery(deadline, completed).await,
        SendOutcome::Done
    ));
    gate.release();
    assert!(matches!(
        bounded(sending).await.unwrap(),
        SendOutcome::End(McpError::SessionExpired)
    ));
    assert_eq!(
        peer.count(|r| serde_json::from_slice::<Value>(&r.body)
            .ok()
            .is_some_and(|m| m["id"] == 70)),
        1
    );
    let first = peer
        .seen
        .lock()
        .unwrap()
        .iter()
        .find(|r| {
            serde_json::from_slice::<Value>(&r.body)
                .ok()
                .is_some_and(|m| m["id"] == 70)
        })
        .unwrap()
        .clone();
    assert_eq!(header(&first, "Mcp-Session-Id"), Some("initial"));
    assert_eq!(
        peer.count(|r| method(r).as_deref() == Some("initialize")),
        2
    );
    stop(&session).await;
    probe.released().await;
}

#[tokio::test]
async fn j17_public_open_waits_for_bound_peer_answer_before_validation() {
    for session_id in [Some("initial".to_owned()), None] {
        let (probe, body) = Probe::body();
        probe.event(json!({"id":"peer","method":"unsupported/method"}));
        let mut peer = Peer::new();
        peer.initial_id = session_id.clone();
        *peer.initial.lock().unwrap() = Some(body);
        let peer = Arc::new(peer);
        let servers = Arc::new(McpServers::new(vec![], Arc::new(RuntimeClock::new())).unwrap());
        servers.set_remote_transport(peer.clone(), Arc::new(NoAuthorization));
        let opening = tokio::spawn({
            let servers = servers.clone();
            async move {
                servers
                    .open_remote_once(
                        &RemoteMcpServer::new(Uuid::from_u128(1), "remote", "http://127.0.0.1/mcp")
                            .unwrap(),
                    )
                    .await
            }
        });
        peer.observed(|r| {
            serde_json::from_slice::<Value>(&r.body)
                .ok()
                .is_some_and(|m| m["id"] == "peer")
        })
        .await;
        let answer = peer
            .seen
            .lock()
            .unwrap()
            .iter()
            .find(|r| {
                serde_json::from_slice::<Value>(&r.body)
                    .ok()
                    .is_some_and(|m| m["id"] == "peer")
            })
            .unwrap()
            .clone();
        assert_eq!(header(&answer, "Mcp-Session-Id"), session_id.as_deref());
        assert_eq!(header(&answer, "MCP-Protocol-Version"), None);
        assert_eq!(
            serde_json::from_slice::<Value>(&answer.body).unwrap()["error"]["code"],
            -32601
        );
        assert_eq!(peer.count(|r| r.method == HttpMethod::Get), 0);
        assert_eq!(
            peer.count(|r| method(r).as_deref() == Some("notifications/initialized")),
            0
        );
        probe.event(json!({"id":1,"result":{"protocolVersion":"2025-06-18"}}));
        let session = bounded(opening).await.unwrap().unwrap();
        bounded(session.list_tools()).await.unwrap();
        if let Some(id) = session_id.as_deref() {
            peer.observed(|r| {
                r.method == HttpMethod::Get && header(r, "Mcp-Session-Id") == Some(id)
            })
            .await;
        }
        assert_eq!(
            peer.count(|r| r.method == HttpMethod::Get),
            usize::from(session_id.is_some())
        );
        assert_eq!(
            peer.count(|r| method(r).as_deref() == Some("notifications/initialized")),
            1
        );
        session.close().await;
        assert_eq!(
            peer.count(|r| r.method == HttpMethod::Delete),
            usize::from(session_id.is_some())
        );
        probe.released().await;
    }
}

#[tokio::test]
async fn j17_ordinary_terminal_ignores_non_authoritative_oversized_header() {
    let mut peer = Peer::new();
    peer.ordinary_response_id = Some("x".repeat(1025));
    let peer = Arc::new(peer);
    let (session, incoming) = transport(peer.clone(), Arc::default());
    let connection =
        Connection::open_http(session.clone(), incoming, Arc::new(RuntimeClock::new()));
    bounded(connection.call("initialize", None))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        bounded(connection.call("tools/list", None))
            .await
            .unwrap()
            .unwrap(),
        json!({"tools":[]})
    );
    stop(&session).await;
}

async fn preterminal_recovery(stale_request: bool) {
    let (old, old_body) = Probe::body();
    let (replacement, replacement_body) = Probe::body();
    let mut peer = Peer::new();
    peer.call_sse = true;
    peer.bodies.lock().unwrap().push_back(old_body);
    *peer.replacement.lock().unwrap() = Some(replacement_body);
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
    let waiting = tokio::spawn({
        let connection = connection.clone();
        async move { connection.call("tools/list", None).await }
    });
    old.polled(1).await;
    peer.expire.store(true, Ordering::SeqCst);
    assert!(matches!(
        bounded(connection.call("tools/list", None)).await,
        Err(McpError::SessionExpired)
    ));
    replacement.polled(1).await;
    if stale_request {
        old.event(json!({"id":70,"method":"ping"}));
        assert_eq!(bounded(connection.ended()).await, McpError::SessionExpired);
        assert!(matches!(
            bounded(waiting).await.unwrap(),
            Err(McpError::SessionExpired)
        ));
    } else {
        old.event(json!({"id":2,"result":{"tools":[]}}));
        bounded(waiting).await.unwrap().unwrap().unwrap();
    }
    // Retirement is a finite barrier after the late terminal is consumed,
    // rather than a delay used to infer absence of replacement effects.
    let _ = replacement.send.send(Ok(Some(
        format!(
            "data: {}\n\n",
            json!({"id":0,"result":{"protocolVersion":"2025-06-18"}})
        )
        .into_bytes(),
    )));
    replacement.released().await;
    if stale_request {
        let outcome = session
            .dispatch(br#"{"method":"notifications/cancelled"}"#)
            .await;
        assert!(
            matches!(
                outcome,
                SendOutcome::End(McpError::SessionExpired | McpError::Closed)
            ),
            "ended connection's HTTP owner admitted a late notification: {outcome:?}"
        );
        assert_eq!(connection.end_cause(), Some(McpError::SessionExpired));
        assert_eq!(
            peer.count(|r| r.method == HttpMethod::Get
                && header(r, "Mcp-Session-Id") == Some("replacement")),
            0
        );
        assert_eq!(
            peer.count(
                |r| method(r).as_deref() == Some("notifications/initialized")
                    && header(r, "Mcp-Session-Id") == Some("replacement")
            ),
            0
        );
    } else {
        peer.observed(|r| {
            method(r).as_deref() == Some("notifications/initialized")
                && header(r, "Mcp-Session-Id") == Some("replacement")
        })
        .await;
        bounded(connection.call("tools/list", None))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(connection.end_cause(), None);
        assert_eq!(
            peer.count(|r| r.method == HttpMethod::Get
                && header(r, "Mcp-Session-Id") == Some("replacement")),
            1
        );
    }
    connection.close(McpError::Closed);
    stop(&session).await;
    old.released().await;
    assert_eq!(
        peer.count(|r| method(r).as_deref() == Some("initialize")),
        2
    );
}

#[tokio::test]
async fn j19_preterminal_stale_request_cannot_revive_ended_connection() {
    preterminal_recovery(true).await;
}

#[tokio::test]
async fn j19_preterminal_ordinary_terminal_preserves_recovery_control() {
    preterminal_recovery(false).await;
}

#[tokio::test]
async fn j19_reader_failure_fences_http_owner_before_ended() {
    let peer = Arc::new(Peer::new());
    let (session, _incoming) = transport(peer.clone(), Arc::default());
    initialize(&session).await;
    let (inbound, incoming) = mpsc::channel(1);
    let connection =
        Connection::open_http(session.clone(), incoming, Arc::new(RuntimeClock::new()));
    inbound.send(Err(McpError::Unreachable)).await.unwrap();
    assert_eq!(bounded(connection.ended()).await, McpError::Unreachable);
    assert!(matches!(
        call(&session, 2).await,
        SendOutcome::End(McpError::Closed)
    ));
    assert_eq!(connection.end_cause(), Some(McpError::Unreachable));
    stop(&session).await;
    assert_eq!(peer.count(|r| r.method == HttpMethod::Delete), 1);
    assert_eq!(
        peer.count(|r| method(r).as_deref() == Some("tools/list")),
        0
    );
}

// Poll to an actual queue or reply wait, rather than Tokio's cooperative yield.
async fn pending<F: Future>(future: Pin<&mut F>) {
    let mut future = std::pin::pin!(tokio::task::unconstrained(future));
    assert!(poll_fn(|context| Poll::Ready(future.as_mut().poll(context).is_pending())).await);
}

fn held_calls(
    connection: &Connection,
) -> Vec<Pin<Box<impl Future<Output = Result<Reply, McpError>> + '_>>> {
    (0..OUTGOING_FRAMES)
        .map(|_| Box::pin(connection.call("tools/list", None)))
        .collect()
}

async fn held_recovery(
    status: u16,
) -> (
    Connection,
    Arc<HttpSession>,
    Arc<Peer>,
    Probe,
    Arc<Gate>,
    Arc<ManualClock>,
) {
    let (probe, body) = Probe::body();
    let gate = Arc::new(Gate::default());
    let peer = Arc::new(Peer::new());
    *peer.replacement.lock().unwrap() = Some(body);
    peer.controls
        .lock()
        .unwrap()
        .push_back((status, Some(gate.clone())));
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
    connection
        .notify("notifications/cancelled", Some(json!({"requestId":9999})))
        .await
        .unwrap();
    gate.reached().await;
    (connection, session, peer, probe, gate, clock)
}

async fn acknowledge_peer_request(connection: &Connection, probe: &Probe, id: u64, method: &str) {
    let mut notices = connection.notices();
    probe.event(json!({"id":id,"method":method}));
    probe.event(json!({"method":"notifications/tools/list_changed"}));
    bounded(async {
        tokio::select! {
            notice = notices.recv() => { notice.unwrap(); }
            cause = connection.ended() => panic!("peer request admission ended connection: {cause:?}"),
        }
    }).await;
}

fn assert_bound_answer(peer: &Peer, id: u64, method: &str) {
    let seen = peer.seen.lock().unwrap();
    let answer = seen
        .iter()
        .find(|request| {
            serde_json::from_slice::<Value>(&request.body)
                .ok()
                .is_some_and(|message| message["id"] == id)
        })
        .unwrap();
    assert_eq!(header(answer, "Mcp-Session-Id"), Some("replacement"));
    assert_eq!(header(answer, "MCP-Protocol-Version"), None);
    let message: Value = serde_json::from_slice(&answer.body).unwrap();
    if method == "ping" {
        assert_eq!(message["result"], json!({}));
    } else {
        assert_eq!(message["error"]["code"], -32601);
    }
}

#[tokio::test]
async fn j21_saturated_ordinary_queue_preserves_early_recovery_answers() {
    for method in ["ping", "sampling/createMessage"] {
        let (connection, session, peer, probe, gate, _) = held_recovery(202).await;
        let mut calls = held_calls(&connection);
        for call in &mut calls {
            pending(call.as_mut()).await;
        }
        let mut excess = Box::pin(connection.call("tools/list", None));
        pending(excess.as_mut()).await;
        drop(excess);
        acknowledge_peer_request(&connection, &probe, 70, method).await;
        assert_eq!(connection.end_cause(), None);
        gate.release();
        for call in calls {
            assert_eq!(bounded(call).await.unwrap_err(), McpError::Busy);
        }
        peer.observed(|request| {
            serde_json::from_slice::<Value>(&request.body)
                .ok()
                .is_some_and(|message| message["id"] == 70)
        })
        .await;
        assert_bound_answer(&peer, 70, method);
        assert_eq!(
            peer.count(|request| method_of(request, "notifications/initialized")),
            0
        );
        assert_eq!(peer.count(|request| method_of(request, "tools/list")), 1);
        probe.event(json!({"id":0,"result":{"protocolVersion":"2025-06-18"}}));
        peer.observed(|request| method_of(request, "notifications/initialized"))
            .await;
        bounded(connection.call("tools/list", None))
            .await
            .unwrap()
            .unwrap();
        stop(&session).await;
        probe.released().await;
        assert_eq!(
            peer.count(|request| request.method == HttpMethod::Delete),
            1
        );
    }
}

fn method_of(request: &HttpRequest, expected: &str) -> bool {
    method(request).as_deref() == Some(expected)
}

#[tokio::test]
async fn j22_matching_initialize_preserves_queued_reply_order() {
    let (connection, session, peer, probe, gate, _) = held_recovery(202).await;
    let mut calls = held_calls(&connection);
    for call in &mut calls {
        pending(call.as_mut()).await;
    }
    acknowledge_peer_request(&connection, &probe, 70, "ping").await;
    probe.event(json!({"id":0,"result":{"protocolVersion":"2025-06-18"}}));
    peer.observed(|request| {
        request.method == HttpMethod::Get
            && header(request, "Mcp-Session-Id") == Some("replacement")
    })
    .await;
    probe.released().await;
    assert_eq!(
        peer.count(|request| method_of(request, "notifications/initialized")),
        0
    );
    gate.release();
    for call in calls {
        assert_eq!(bounded(call).await.unwrap_err(), McpError::Busy);
    }
    peer.observed(|request| method_of(request, "notifications/initialized"))
        .await;
    assert_bound_answer(&peer, 70, "ping");
    {
        let seen = peer.seen.lock().unwrap();
        let answer = seen
            .iter()
            .position(|request| {
                serde_json::from_slice::<Value>(&request.body)
                    .ok()
                    .is_some_and(|message| message["id"] == 70)
            })
            .unwrap();
        let initialized = seen
            .iter()
            .position(|request| method_of(request, "notifications/initialized"))
            .unwrap();
        assert!(answer < initialized);
    }
    bounded(connection.call("tools/list", None))
        .await
        .unwrap()
        .unwrap();
    stop(&session).await;
}

#[tokio::test]
async fn j23_control_overflow_retains_first_cause_and_one_cleanup() {
    let (connection, session, peer, probe, gate, _) = held_recovery(202).await;
    let mut calls = held_calls(&connection);
    for call in &mut calls {
        pending(call.as_mut()).await;
    }
    acknowledge_peer_request(&connection, &probe, 70, "ping").await;
    probe.event(json!({"id":71,"method":"ping"}));
    let cause = McpError::TooLarge("queued MCP control frames");
    assert_eq!(bounded(connection.ended()).await, cause);
    connection.close(McpError::Closed);
    gate.release();
    for call in calls {
        assert_eq!(bounded(call).await.unwrap_err(), cause);
    }
    stop(&session).await;
    probe.released().await;
    assert_eq!(connection.end_cause(), Some(cause));
    assert_eq!(
        peer.count(|request| request.method == HttpMethod::Delete),
        1
    );
    assert_eq!(
        peer.count(|request| serde_json::from_slice::<Value>(&request.body)
            .ok()
            .is_some_and(|message| message["id"] == 70 || message["id"] == 71)),
        0
    );
}

#[tokio::test]
async fn j23_closed_control_queue_is_explicit() {
    // A custom exchange can unwind the writer before it publishes an end.
    // Prove a subsequent peer request turns actual receiver loss into ServerGone.
    let (connection, session, peer, probe, gate, _) = held_recovery(202).await;
    peer.control_panic.store(true, Ordering::SeqCst);
    gate.release();
    bounded(async {
        loop {
            if let Err(error) = connection.notify("notifications/test", None).await {
                assert_eq!(error, McpError::ServerGone);
                break;
            }
        }
    })
    .await;
    assert_eq!(connection.end_cause(), None);
    probe.event(json!({"id":70,"method":"ping"}));
    assert_eq!(bounded(connection.ended()).await, McpError::ServerGone);
    connection.close(McpError::Closed);
    stop(&session).await;
    assert_eq!(connection.end_cause(), Some(McpError::ServerGone));
    assert_eq!(
        peer.count(|request| request.method == HttpMethod::Delete),
        1
    );
    assert_eq!(
        peer.count(|request| serde_json::from_slice::<Value>(&request.body)
            .ok()
            .is_some_and(|message| message["id"] == 70)),
        0
    );

    let (queue, frames) = OutgoingQueue::new(1, 1);
    drop(frames);
    let (completed, _) = tokio::sync::oneshot::channel();
    assert!(matches!(
        queue.try_send(Outgoing::RecoveryReady {
            deadline: ManualClock::default().now(),
            completed
        }),
        Err(TrySendError::Closed(_))
    ));
}

#[tokio::test]
async fn j24_ordinary_capacity_releases_on_dequeue_and_cancellation() {
    let (queue, mut frames) = OutgoingQueue::new(OUTGOING_FRAMES, 1);
    for id in 0..OUTGOING_FRAMES {
        queue
            .send(Outgoing::Frame(
                serde_json::to_vec(&json!({"id":id,"method":"tools/list"})).unwrap(),
            ))
            .await
            .unwrap();
    }
    let mut waiting = Box::pin(queue.send(Outgoing::Frame(b"waiting".to_vec())));
    pending(waiting.as_mut()).await;
    drop(waiting);
    let mut next = Box::pin(queue.send(Outgoing::Frame(b"next".to_vec())));
    pending(next.as_mut()).await;
    let held = frames.recv().await.unwrap();
    bounded(next).await.unwrap();
    assert!(matches!(held, Outgoing::Frame(_)));
    let (completed, _) = tokio::sync::oneshot::channel();
    queue
        .try_send(Outgoing::RecoveryReady {
            deadline: ManualClock::default().now(),
            completed,
        })
        .unwrap();
    let (completed, _) = tokio::sync::oneshot::channel();
    assert!(matches!(
        queue.try_send(Outgoing::RecoveryReady {
            deadline: ManualClock::default().now(),
            completed
        }),
        Err(TrySendError::Full(_))
    ));
    let mut closed = Box::pin(queue.send(Outgoing::Frame(b"closed".to_vec())));
    pending(closed.as_mut()).await;
    drop(frames);
    assert!(bounded(closed).await.is_err());

    // A sender can own an ordinary permit while waiting behind controls.
    // Dropping that send must release the permit as well as its encoded frame.
    let (queue, mut frames) = OutgoingQueue::new(1, 1);
    for _ in 0..2 {
        let (completed, _) = tokio::sync::oneshot::channel();
        queue
            .try_send(Outgoing::RecoveryReady {
                deadline: ManualClock::default().now(),
                completed,
            })
            .unwrap();
    }
    let mut waiting = Box::pin(queue.send(Outgoing::Frame(b"waiting behind controls".to_vec())));
    pending(waiting.as_mut()).await;
    drop(waiting);
    let held_control = frames.recv().await.unwrap();
    queue
        .try_send(Outgoing::Frame(b"reuses cancelled permit".to_vec()))
        .unwrap();
    assert!(matches!(held_control, Outgoing::RecoveryReady { .. }));
}

#[tokio::test]
async fn j25_deadline_refuses_queued_saturated_reply() {
    let (connection, session, peer, probe, gate, clock) = held_recovery(202).await;
    let mut calls = held_calls(&connection);
    for call in &mut calls {
        pending(call.as_mut()).await;
    }
    acknowledge_peer_request(&connection, &probe, 70, "ping").await;
    clock.advance(INITIALIZE_TIMEOUT);
    assert_eq!(bounded(connection.ended()).await, McpError::Timeout);
    gate.release();
    for call in calls {
        assert_eq!(bounded(call).await.unwrap_err(), McpError::Timeout);
    }
    stop(&session).await;
    assert_eq!(
        peer.count(|request| serde_json::from_slice::<Value>(&request.body)
            .ok()
            .is_some_and(|message| message["id"] == 70)),
        0
    );
    assert_eq!(
        peer.count(|request| method_of(request, "notifications/initialized")),
        0
    );
    assert_eq!(
        peer.count(|request| request.method == HttpMethod::Delete),
        1
    );
    assert_eq!(connection.end_cause(), Some(McpError::Timeout));
}

#[tokio::test]
async fn j26_close_or_writer_failure_refuses_queued_saturated_reply() {
    for status in [202, 401] {
        let (connection, session, peer, probe, gate, _) = held_recovery(status).await;
        let mut calls = held_calls(&connection);
        for call in &mut calls {
            pending(call.as_mut()).await;
        }
        acknowledge_peer_request(&connection, &probe, 70, "ping").await;
        let expected = if status == 202 {
            McpError::Closed
        } else {
            McpError::Unauthorized
        };
        if status == 202 {
            connection.close(expected.clone());
        }
        gate.release();
        assert_eq!(bounded(connection.ended()).await, expected);
        connection.close(McpError::Stopped);
        for call in calls {
            assert_eq!(bounded(call).await.unwrap_err(), expected);
        }
        stop(&session).await;
        probe.released().await;
        assert_eq!(connection.end_cause(), Some(expected));
        assert_eq!(
            peer.count(|request| serde_json::from_slice::<Value>(&request.body)
                .ok()
                .is_some_and(|message| message["id"] == 70)),
            0
        );
        assert_eq!(
            peer.count(|request| method_of(request, "notifications/initialized")),
            0
        );
        assert_eq!(
            peer.count(|request| request.method == HttpMethod::Delete),
            1
        );
    }
}
