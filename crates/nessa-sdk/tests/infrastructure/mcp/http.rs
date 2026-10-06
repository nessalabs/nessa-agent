//! Remote HTTP sessions (ADR 392 connection rows). A scripted
//! [`HttpExchange`] covers the orderings a loopback fixture cannot force;
//! one test speaks HTTP/1.1 to `scripts/mcp-test-server/http-server.mjs`.
use crate::infrastructure::clock::RuntimeClock;
use crate::infrastructure::mcp::error::McpError;
use crate::infrastructure::mcp::framing::MAX_FRAME_BYTES;
use crate::infrastructure::mcp::http_exchange::{
    HttpBody, HttpChunks, HttpExchange, HttpFailure, HttpMethod, HttpRequest, HttpResponse,
};
use crate::infrastructure::mcp::{
    Bearer, McpServers, RemoteAuthorization, RemoteMcpServer, RemoteMcpUrl, RemoteUrlProblem,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Notify};
use uuid::Uuid;

fn remote_at(url: &str) -> RemoteMcpServer {
    RemoteMcpServer::new(Uuid::from_u128(1), "remote", url).unwrap()
}

fn servers_with(peer: Arc<Peer>) -> McpServers {
    let servers = McpServers::new(Vec::new(), Arc::new(RuntimeClock::new())).unwrap();
    servers.set_remote_transport(peer.clone(), Arc::new(NoAuth));
    servers
}

struct NoAuth;
#[async_trait]
impl RemoteAuthorization for NoAuth {
    async fn bearer(&self, _: Uuid) -> Result<Option<Bearer>, McpError> {
        Ok(None)
    }
    async fn rejected(&self, _: Uuid, _: &str) -> Result<Option<Bearer>, McpError> {
        Err(McpError::Unauthorized)
    }
    async fn insufficient_scope(&self, _: Uuid, _: &str) {}
}

/// One scripted peer. `log` is what it was asked; `behavior` is how it answers.
struct Peer {
    behavior: Behavior,
    log: Mutex<Log>,
    entered: Notify,
}

struct Log {
    requests: Vec<Seen>,
    minted: Vec<String>,
    /// 404s already returned for a session-bound call.
    expired: u8,
    legacy: Option<mpsc::UnboundedSender<Option<Vec<u8>>>>,
}

struct Seen {
    method: HttpMethod,
    url: String,
    session: Option<String>,
    authorization: Option<String>,
    body: Vec<u8>,
}

#[derive(Clone, Copy)]
enum Behavior {
    /// JSON replies, a new session id, GET 405.
    Json,
    /// Initialize omits `Mcp-Session-Id`.
    NoSession,
    /// Initialize body is one SSE event.
    Sse,
    /// SSE split across chunks, including a comment line.
    SplitSse,
    /// Initial POST is this status. 400/404/405 enter legacy; others do not.
    Initial(u16),
    /// 200 with a content type that is neither JSON nor an event stream.
    BadType,
    /// The first session-bound call is 404; one recovery is accepted.
    ExpireOnce,
    /// A second 404 after recovery ends the session.
    ExpireTwice,
    /// Every initialize returns the same session id.
    Collide,
    /// Optional GET is a stream that ends at once.
    DropGet,
    /// One SSE event past the frame bound.
    Oversize,
    /// The first POST does not answer until the task is dropped.
    Block,
    /// Legacy HTTP+SSE: POST 405, GET endpoint event, replies on that stream.
    Legacy,
}

impl Peer {
    fn new(behavior: Behavior) -> Arc<Self> {
        Arc::new(Self {
            behavior,
            log: Mutex::new(Log {
                requests: Vec::new(),
                minted: Vec::new(),
                expired: 0,
                legacy: None,
            }),
            entered: Notify::new(),
        })
    }

    fn seen(&self) -> Vec<Seen> {
        self.log.lock().expect("log").requests.drain(..).collect()
    }

    fn requests(&self) -> usize {
        self.log.lock().expect("log").requests.len()
    }
}

#[async_trait]
impl HttpExchange for Peer {
    async fn exchange(&self, request: HttpRequest) -> Result<HttpResponse, HttpFailure> {
        let session = header(&request.headers, "mcp-session-id").map(str::to_owned);
        let authorization = header(&request.headers, "authorization").map(str::to_owned);
        let method_name = json_method(&request.body);
        {
            let mut log = self.log.lock().expect("log");
            log.requests.push(Seen {
                method: request.method,
                url: request.url.clone(),
                session,
                authorization,
                body: request.body.clone(),
            });
        }
        if matches!(self.behavior, Behavior::Block) && request.method == HttpMethod::Post {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
        match request.method {
            HttpMethod::Delete => Ok(response(200, vec![], Vec::new())),
            HttpMethod::Get => self.answer_get(request).await,
            HttpMethod::Post => self.answer_post(request, method_name.as_deref()).await,
        }
    }
}

impl Peer {
    async fn answer_get(&self, request: HttpRequest) -> Result<HttpResponse, HttpFailure> {
        if matches!(self.behavior, Behavior::Legacy) {
            let (tx, rx) = mpsc::unbounded_channel();
            let endpoint = endpoint_of(&request.url);
            let _ = tx.send(Some(
                format!("event: endpoint\ndata: {endpoint}\n\n").into_bytes(),
            ));
            self.log.lock().expect("log").legacy = Some(tx);
            return Ok(HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "text/event-stream".into())],
                body: HttpBody::Stream(Box::new(ChannelChunks { rx })),
            });
        }
        if matches!(self.behavior, Behavior::DropGet) {
            return Ok(HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "text/event-stream".into())],
                body: HttpBody::Stream(Box::new(ChannelChunks {
                    rx: mpsc::unbounded_channel().1,
                })),
            });
        }
        Ok(response(405, vec![], Vec::new()))
    }

    async fn answer_post(
        &self,
        request: HttpRequest,
        method: Option<&str>,
    ) -> Result<HttpResponse, HttpFailure> {
        if let Behavior::Initial(status) = self.behavior {
            let first = self
                .log
                .lock()
                .expect("log")
                .requests
                .iter()
                .filter(|seen| seen.method == HttpMethod::Post)
                .count()
                == 1;
            if first && method == Some("initialize") {
                return Ok(response(status, vec![], b"{}".to_vec()));
            }
        }
        if matches!(self.behavior, Behavior::Legacy) {
            if request.url.ends_with("/mcp") {
                return Ok(response(405, vec![], Vec::new()));
            }
            let message = reply_message(&request.body, method);
            if let Some(tx) = &self.log.lock().expect("log").legacy {
                if let Some(message) = message {
                    let _ = tx.send(Some(
                        format!("event: message\ndata: {message}\n\n").into_bytes(),
                    ));
                }
            }
            return Ok(response(202, vec![], Vec::new()));
        }
        if method == Some("initialize") {
            return Ok(self.initialize(&request));
        }
        if method.is_none() || !body_has_id(&request.body) {
            return Ok(response(202, vec![], Vec::new()));
        }
        if matches!(self.behavior, Behavior::ExpireOnce | Behavior::ExpireTwice) {
            let mut log = self.log.lock().expect("log");
            let limit = if matches!(self.behavior, Behavior::ExpireTwice) {
                2
            } else {
                1
            };
            if log.expired < limit {
                log.expired += 1;
                return Ok(response(404, vec![], b"{}".to_vec()));
            }
        }
        let body = reply_message(&request.body, method).unwrap_or_else(|| "{}".into());
        Ok(response(
            200,
            vec![("content-type".into(), "application/json".into())],
            body.into_bytes(),
        ))
    }

    fn initialize(&self, request: &HttpRequest) -> HttpResponse {
        let id = match self.behavior {
            Behavior::Collide => "same-session".to_owned(),
            Behavior::NoSession => String::new(),
            _ => {
                let id = format!("s-{}", self.log.lock().expect("log").minted.len() + 1);
                self.log.lock().expect("log").minted.push(id.clone());
                id
            }
        };
        if matches!(self.behavior, Behavior::BadType) {
            return response(
                200,
                vec![("content-type".into(), "text/plain".into())],
                b"nope".to_vec(),
            );
        }
        let payload = reply_message(&request.body, Some("initialize")).unwrap();
        if matches!(self.behavior, Behavior::Oversize) {
            let mut data = vec![b'd', b'a', b't', b'a', b':', b' '];
            data.extend(std::iter::repeat(b'x').take(MAX_FRAME_BYTES));
            data.extend(b"\n\n");
            return HttpResponse {
                status: 200,
                headers: vec![
                    ("content-type".into(), "text/event-stream".into()),
                    ("mcp-session-id".into(), id),
                ],
                body: HttpBody::Stream(Box::new(OnceChunks::parts(vec![data]))),
            };
        }
        let mut headers = vec![("content-type".into(), "application/json".into())];
        if !id.is_empty() {
            headers.push(("mcp-session-id".into(), id));
        }
        if matches!(self.behavior, Behavior::Sse) {
            headers[0].1 = "text/event-stream".into();
            let body = format!("data: {payload}\n\n");
            return response(200, headers, body.into_bytes());
        }
        if matches!(self.behavior, Behavior::SplitSse) {
            headers[0].1 = "text/event-stream".into();
            let notice = json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"});
            let bytes = format!(": keep\n\ndata: {notice}\n\ndata: {payload}\n\n").into_bytes();
            let mid = bytes.len() / 2;
            return HttpResponse {
                status: 200,
                headers,
                body: HttpBody::Stream(Box::new(OnceChunks::parts(vec![
                    bytes[..mid].to_vec(),
                    bytes[mid..].to_vec(),
                ]))),
            };
        }
        response(200, headers, payload.into_bytes())
    }
}

fn reply_message(body: &[u8], method: Option<&str>) -> Option<String> {
    let message: Value = serde_json::from_slice(body).ok()?;
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    if id.is_null() {
        return None;
    }
    let result = if method == Some("initialize") {
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "serverInfo": {"name": "t", "version": "0"}})
    } else if method == Some("tools/list") {
        json!({"tools": []})
    } else {
        json!({})
    };
    Some(serde_json::to_string(&json!({"jsonrpc":"2.0","id": id, "result": result})).unwrap())
}

fn body_has_id(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|message| message.get("id").cloned())
        .is_some_and(|id| !id.is_null())
}

fn json_method(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()?
        .get("method")?
        .as_str()
        .map(str::to_owned)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find_map(|(header, value)| header.eq_ignore_ascii_case(name).then_some(value.as_str()))
}

fn response(status: u16, headers: Vec<(String, String)>, body: Vec<u8>) -> HttpResponse {
    HttpResponse {
        status,
        headers,
        body: HttpBody::Buffered(body),
    }
}

fn endpoint_of(mcp: &str) -> String {
    let mut url = mcp.to_owned();
    if let Some(stripped) = url.strip_suffix("/mcp") {
        url = format!("{stripped}/messages?sessionId=legacy");
    }
    url
}

struct ChannelChunks {
    rx: mpsc::UnboundedReceiver<Option<Vec<u8>>>,
}
#[async_trait]
impl HttpChunks for ChannelChunks {
    async fn next(&mut self) -> Result<Option<Vec<u8>>, HttpFailure> {
        Ok(self.rx.recv().await.flatten())
    }
}

struct OnceChunks(VecDeque<Vec<u8>>);
impl OnceChunks {
    fn parts(parts: Vec<Vec<u8>>) -> Self {
        Self(parts.into())
    }
}
#[async_trait]
impl HttpChunks for OnceChunks {
    async fn next(&mut self) -> Result<Option<Vec<u8>>, HttpFailure> {
        Ok(self.0.pop_front())
    }
}

fn must_err(result: Result<crate::infrastructure::mcp::McpSession, McpError>) -> McpError {
    match result {
        Ok(_) => panic!("the remote session opened"),
        Err(error) => error,
    }
}

async fn open(peer: Arc<Peer>) -> Result<crate::infrastructure::mcp::McpSession, McpError> {
    servers_with(peer)
        .open_remote_once(&remote_at("http://127.0.0.1/mcp"))
        .await
}

#[tokio::test]
async fn c1_json_initialize_lists_tools_and_deletes_once() {
    let peer = Peer::new(Behavior::Json);
    let session = open(peer.clone()).await.unwrap();
    assert!(session.list_tools().await.unwrap().is_empty());
    session.close().await;
    session.close().await;
    let deletes = peer
        .seen()
        .into_iter()
        .filter(|seen| seen.method == HttpMethod::Delete)
        .count();
    assert_eq!(deletes, 1);
}

#[tokio::test]
async fn c1_sse_and_split_events_initialize() {
    open(Peer::new(Behavior::Sse)).await.unwrap().close().await;
    open(Peer::new(Behavior::SplitSse))
        .await
        .unwrap()
        .close()
        .await;
}

#[tokio::test]
async fn c1_omitted_session_id_records_delete_not_applicable() {
    let peer = Peer::new(Behavior::NoSession);
    let session = open(peer.clone()).await.unwrap();
    session.close().await;
    assert!(peer
        .seen()
        .iter()
        .all(|seen| seen.method != HttpMethod::Delete));
}

#[tokio::test]
async fn c2_legacy_follows_only_400_404_405() {
    for status in [400_u16, 404, 405] {
        let peer = Peer::new(Behavior::Initial(status));
        // The scripted legacy path is the dedicated behavior; an initial
        // status alone has no endpoint stream, so the opening fails after
        // the fallback GET — and that GET happened.
        let _ = open(peer.clone()).await;
        let methods: Vec<_> = peer.seen().into_iter().map(|seen| seen.method).collect();
        assert!(
            methods.contains(&HttpMethod::Get),
            "status {status} did not fall back: {methods:?}"
        );
    }
    for status in [401_u16, 500] {
        let peer = Peer::new(Behavior::Initial(status));
        let error = must_err(open(peer.clone()).await);
        assert!(
            matches!(
                error,
                McpError::Unauthorized | McpError::Unreachable | McpError::Handshake(_)
            ),
            "{status}: {error}"
        );
        assert!(peer
            .seen()
            .iter()
            .all(|seen| seen.method != HttpMethod::Get));
    }
    let peer = Peer::new(Behavior::BadType);
    let error = must_err(open(peer.clone()).await);
    assert!(matches!(
        error,
        McpError::Malformed(_) | McpError::Handshake(_)
    ));
    assert!(peer
        .seen()
        .iter()
        .all(|seen| seen.method != HttpMethod::Get));
}

#[tokio::test]
async fn c2_legacy_endpoint_initializes_and_lists() {
    let peer = Peer::new(Behavior::Legacy);
    let session = open(peer.clone()).await.unwrap();
    assert!(session.list_tools().await.unwrap().is_empty());
    let posts: Vec<_> = peer
        .seen()
        .into_iter()
        .filter(|seen| seen.method == HttpMethod::Post)
        .map(|seen| seen.url)
        .collect();
    assert!(posts.iter().any(|url| url.ends_with("/mcp")));
    assert!(posts.iter().any(|url| url.contains("/messages?")));
    session.close().await;
    assert!(peer
        .seen()
        .iter()
        .all(|seen| seen.method != HttpMethod::Delete));
}

#[tokio::test]
async fn c3_get_405_leaves_the_post_session_up() {
    let peer = Peer::new(Behavior::Json);
    let session = open(peer.clone()).await.unwrap();
    assert!(session.list_tools().await.is_ok());
    let gets = peer
        .seen()
        .iter()
        .filter(|seen| seen.method == HttpMethod::Get)
        .count();
    assert_eq!(gets, 1);
}

#[tokio::test]
async fn c5_session_404_fails_the_call_and_does_not_replay_it() {
    let peer = Peer::new(Behavior::ExpireOnce);
    let session = open(peer.clone()).await.unwrap();
    let error = session.list_tools().await.unwrap_err();
    assert_eq!(error, McpError::SessionExpired);
    assert!(session.list_tools().await.is_ok());
    let bodies: Vec<_> = peer
        .seen()
        .into_iter()
        .filter(|seen| seen.method == HttpMethod::Post)
        .map(|seen| String::from_utf8_lossy(&seen.body).into_owned())
        .collect();
    let lists = bodies
        .iter()
        .filter(|body| body.contains("tools/list"))
        .count();
    assert_eq!(lists, 2, "the expired call is not replayed: {bodies:?}");
}

#[tokio::test]
async fn c5_a_second_404_ends_the_session() {
    let peer = Peer::new(Behavior::ExpireTwice);
    let session = open(peer).await.unwrap();
    assert_eq!(
        session.list_tools().await.unwrap_err(),
        McpError::SessionExpired
    );
    assert_eq!(
        session.list_tools().await.unwrap_err(),
        McpError::SessionExpired
    );
}

#[tokio::test]
async fn c6_a_dropped_get_stream_is_unconfirmed() {
    let peer = Peer::new(Behavior::DropGet);
    match open(peer).await {
        Ok(session) => {
            let error = session.list_tools().await.unwrap_err();
            assert!(
                matches!(
                    error,
                    McpError::Unconfirmed | McpError::ServerGone | McpError::Closed
                ),
                "{error}"
            );
        }
        Err(error) => assert!(
            matches!(
                error,
                McpError::Unconfirmed | McpError::ServerGone | McpError::Handshake(_)
            ),
            "{error}"
        ),
    }
}

#[tokio::test]
async fn c7_a_colliding_session_id_is_not_deleted() {
    let peer = Peer::new(Behavior::Collide);
    let servers = servers_with(peer.clone());
    let first = servers
        .open_remote_once(&remote_at("http://127.0.0.1/mcp"))
        .await
        .unwrap();
    let error = must_err(
        servers
            .open_remote_once(&remote_at("http://127.0.0.1/mcp"))
            .await,
    );
    assert!(
        matches!(error, McpError::SessionCollision | McpError::Handshake(_)),
        "{error}"
    );
    let deletes_while_held = peer
        .seen()
        .into_iter()
        .filter(|seen| seen.method == HttpMethod::Delete)
        .count();
    assert_eq!(deletes_while_held, 0);
    first.close().await;
    let deletes = peer
        .seen()
        .into_iter()
        .filter(|seen| seen.method == HttpMethod::Delete)
        .count();
    assert_eq!(deletes, 1);
}

#[tokio::test]
async fn c8_close_during_initialize_does_not_delete() {
    let peer = Peer::new(Behavior::Block);
    let servers = servers_with(peer.clone());
    let opening = tokio::spawn({
        let servers = servers.clone();
        async move {
            servers
                .open_remote_once(&remote_at("http://127.0.0.1/mcp"))
                .await
        }
    });
    peer.entered.notified().await;
    servers.stop().await;
    let error = must_err(opening.await.unwrap());
    assert_eq!(error, McpError::Stopped);
    assert!(peer
        .seen()
        .iter()
        .all(|seen| seen.method != HttpMethod::Delete));
}

#[tokio::test]
async fn c10_an_oversize_sse_event_is_refused() {
    let error = must_err(open(Peer::new(Behavior::Oversize)).await);
    assert!(
        matches!(error, McpError::TooLarge(_) | McpError::Handshake(_)),
        "{error}"
    );
}

#[tokio::test]
async fn c11_two_openings_keep_distinct_session_ids() {
    let peer = Peer::new(Behavior::Json);
    let servers = servers_with(peer.clone());
    let first = servers
        .open_remote_once(&remote_at("http://127.0.0.1/mcp"))
        .await
        .unwrap();
    let second = servers
        .open_remote_once(&remote_at("http://127.0.0.1/mcp"))
        .await
        .unwrap();
    let ids: Vec<_> = peer
        .seen()
        .into_iter()
        .filter_map(|seen| seen.session)
        .collect();
    assert!(ids.iter().any(|id| id == "s-1"));
    assert!(ids.iter().any(|id| id == "s-2"));
    first.close().await;
    second.close().await;
}

#[tokio::test]
async fn a_401_is_retried_once_with_the_replacement_bearer() {
    let peer = Peer::new(Behavior::Json);
    let auth = Arc::new(RetryAuth::default());
    let servers = McpServers::new(Vec::new(), Arc::new(RuntimeClock::new())).unwrap();
    servers.set_remote_transport(peer.clone(), auth.clone());
    // The peer answers 200. Wrap it so the first POST is 401.
    let wrapped = Arc::new(UnauthorizedOnce(peer.clone()));
    servers.set_remote_transport(wrapped.clone(), auth.clone());
    servers
        .open_remote_once(&remote_at("http://127.0.0.1/mcp"))
        .await
        .unwrap()
        .close()
        .await;
    assert_eq!(auth.rejected.load(std::sync::atomic::Ordering::SeqCst), 1);
    let authorizations: Vec<_> = wrapped
        .0
        .seen()
        .into_iter()
        .filter_map(|seen| seen.authorization)
        .collect();
    assert!(authorizations.iter().any(|header| header == "Bearer old"));
    assert!(authorizations.iter().any(|header| header == "Bearer new"));
}

struct UnauthorizedOnce(Arc<Peer>);
#[async_trait]
impl HttpExchange for UnauthorizedOnce {
    async fn exchange(&self, request: HttpRequest) -> Result<HttpResponse, HttpFailure> {
        let initialize = json_method(&request.body).as_deref() == Some("initialize");
        let first = self.0.requests() == 0;
        let response = self.0.exchange(request).await?;
        if initialize && first {
            return Ok(response_status(401, "Bearer"));
        }
        Ok(response)
    }
}

fn response_status(status: u16, challenge: &str) -> HttpResponse {
    response(
        status,
        vec![("www-authenticate".into(), challenge.into())],
        Vec::new(),
    )
}

#[derive(Default)]
struct RetryAuth {
    rejected: std::sync::atomic::AtomicUsize,
}
#[async_trait]
impl RemoteAuthorization for RetryAuth {
    async fn bearer(&self, _: Uuid) -> Result<Option<Bearer>, McpError> {
        Ok(Some(Bearer::new("old", 1)))
    }
    async fn rejected(&self, _: Uuid, _: &str) -> Result<Option<Bearer>, McpError> {
        self.rejected
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Some(Bearer::new("new", 2)))
    }
    async fn insufficient_scope(&self, _: Uuid, _: &str) {}
}

#[test]
fn a_remote_url_is_https_or_loopback_http() {
    assert!(RemoteMcpUrl::parse("https://mcp.example/mcp").is_ok());
    assert!(RemoteMcpUrl::parse("http://127.0.0.1:9/mcp").is_ok());
    assert!(RemoteMcpUrl::parse("http://localhost/mcp").is_ok());
    assert!(RemoteMcpUrl::parse("http://[::1]/mcp").is_ok());
    assert_eq!(
        RemoteMcpUrl::parse("http://10.0.0.1/mcp"),
        Err(RemoteUrlProblem::NotLoopback)
    );
    assert_eq!(
        RemoteMcpUrl::parse("http://user@127.0.0.1/mcp"),
        Err(RemoteUrlProblem::Userinfo)
    );
    assert_eq!(
        RemoteMcpUrl::parse("https://mcp.example/mcp#x"),
        Err(RemoteUrlProblem::Fragment)
    );
    let url = RemoteMcpUrl::parse("https://mcp.example/mcp").unwrap();
    assert!(url.same_origin("https://mcp.example/messages").is_some());
    assert!(url.same_origin("https://other.example/messages").is_none());
}

#[tokio::test]
async fn a_loopback_fixture_connects_over_streamable_http_and_legacy_sse() {
    let json = FixtureProcess::spawn(false);
    connect_fixture(&json.url()).await;
    let legacy = FixtureProcess::spawn(true);
    connect_fixture(&legacy.url()).await;
}

async fn connect_fixture(url: &str) {
    let transport = Arc::new(Loopback);
    let servers = McpServers::new(Vec::new(), Arc::new(RuntimeClock::new())).unwrap();
    servers.set_remote_transport(transport, Arc::new(NoAuth));
    let session = servers.open_remote_once(&remote_at(url)).await.unwrap();
    session.list_tools().await.expect("tools/list");
    session.close().await;
}

struct FixtureProcess {
    child: Child,
    url: String,
}

impl FixtureProcess {
    fn spawn(sse: bool) -> Self {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/mcp-test-server/http-server.mjs");
        let mut command = Command::new("node");
        command
            .arg(&script)
            .arg("0")
            .stderr(Stdio::piped())
            .stdout(Stdio::null());
        if sse {
            command.arg("--sse");
        }
        let mut child = command.spawn().expect("node http fixture");
        let stderr = child.stderr.take().unwrap();
        let port = read_port(stderr);
        Self {
            child,
            url: format!("http://127.0.0.1:{port}/mcp"),
        }
    }

    fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for FixtureProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_port(stderr: std::process::ChildStderr) -> u16 {
    use std::io::{BufRead, BufReader};
    let mut line = String::new();
    BufReader::new(stderr)
        .read_line(&mut line)
        .expect("fixture port");
    line.split(':')
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .and_then(|port| port.trim().parse().ok())
        .expect("fixture port")
}

struct Loopback;

#[async_trait]
impl HttpExchange for Loopback {
    async fn exchange(&self, request: HttpRequest) -> Result<HttpResponse, HttpFailure> {
        let url = url::Url::parse(&request.url).map_err(|_| HttpFailure::Unreachable)?;
        let host = url.host_str().ok_or(HttpFailure::Unreachable)?;
        let port = url
            .port_or_known_default()
            .ok_or(HttpFailure::Unreachable)?;
        let mut stream = TcpStream::connect((host, port))
            .await
            .map_err(|_| HttpFailure::Unreachable)?;
        let path = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_owned(),
        };
        let mut head = format!(
            "{} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\nContent-Length: {}\r\n",
            request.method.as_str(),
            request.body.len()
        );
        for (name, value) in &request.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        stream
            .write_all(head.as_bytes())
            .await
            .map_err(|_| HttpFailure::Unreachable)?;
        stream
            .write_all(&request.body)
            .await
            .map_err(|_| HttpFailure::Unreachable)?;
        let (status, headers, rest) = read_head(&mut stream).await?;
        let event_stream = headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("content-type") && value.contains("text/event-stream")
        });
        let chunked = headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
        });
        if event_stream {
            return Ok(HttpResponse {
                status,
                headers,
                body: HttpBody::Stream(Box::new(SocketChunks {
                    pending: rest,
                    stream,
                    chunked,
                })),
            });
        }
        let body = if chunked {
            read_chunked(&mut stream, rest).await?
        } else {
            let mut body = rest;
            stream
                .read_to_end(&mut body)
                .await
                .map_err(|_| HttpFailure::Unreachable)?;
            body
        };
        Ok(HttpResponse {
            status,
            headers,
            body: HttpBody::Buffered(body),
        })
    }
}

async fn read_chunked(stream: &mut TcpStream, mut raw: Vec<u8>) -> Result<Vec<u8>, HttpFailure> {
    let mut buf = [0_u8; 1024];
    loop {
        if let Some((body, _)) = take_chunked(&raw) {
            return Ok(body);
        }
        let read = stream
            .read(&mut buf)
            .await
            .map_err(|_| HttpFailure::Unreachable)?;
        if read == 0 {
            return Err(HttpFailure::Unreachable);
        }
        raw.extend_from_slice(&buf[..read]);
    }
}

/// Decode one complete chunked body. `None` when `raw` does not yet contain
/// the terminating chunk.
fn take_chunked(raw: &[u8]) -> Option<(Vec<u8>, usize)> {
    let mut out = Vec::new();
    let mut index = 0;
    loop {
        let line_end = raw[index..].windows(2).position(|mark| mark == b"\r\n")?;
        let line = std::str::from_utf8(&raw[index..index + line_end]).ok()?;
        let size = usize::from_str_radix(line.split(';').next()?.trim(), 16).ok()?;
        index += line_end + 2;
        if size == 0 {
            let trailer = raw[index..].windows(2).position(|mark| mark == b"\r\n")?;
            return Some((out, index + trailer + 2));
        }
        if raw.len() < index + size + 2 {
            return None;
        }
        out.extend_from_slice(&raw[index..index + size]);
        index += size;
        if &raw[index..index + 2] != b"\r\n" {
            return None;
        }
        index += 2;
    }
}

async fn read_head(
    stream: &mut TcpStream,
) -> Result<(u16, Vec<(String, String)>, Vec<u8>), HttpFailure> {
    let mut raw = Vec::new();
    let mut buf = [0_u8; 1024];
    let split = loop {
        let read = stream
            .read(&mut buf)
            .await
            .map_err(|_| HttpFailure::Unreachable)?;
        if read == 0 {
            return Err(HttpFailure::Unreachable);
        }
        raw.extend_from_slice(&buf[..read]);
        if let Some(split) = raw.windows(4).position(|mark| mark == b"\r\n\r\n") {
            break split;
        }
        if raw.len() > 64 * 1024 {
            return Err(HttpFailure::Unreachable);
        }
    };
    let head = std::str::from_utf8(&raw[..split]).map_err(|_| HttpFailure::Unreachable)?;
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|status| status.parse().ok())
        .ok_or(HttpFailure::Unreachable)?;
    let mut headers = Vec::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        headers.push((name.trim().to_owned(), value.trim().to_owned()));
    }
    Ok((status, headers, raw[split + 4..].to_vec()))
}

struct SocketChunks {
    pending: Vec<u8>,
    stream: TcpStream,
    chunked: bool,
}

#[async_trait]
impl HttpChunks for SocketChunks {
    async fn next(&mut self) -> Result<Option<Vec<u8>>, HttpFailure> {
        if !self.chunked {
            if !self.pending.is_empty() {
                return Ok(Some(std::mem::take(&mut self.pending)));
            }
            let mut buf = [0_u8; 1024];
            let read = self
                .stream
                .read(&mut buf)
                .await
                .map_err(|_| HttpFailure::Unreachable)?;
            return if read == 0 {
                Ok(None)
            } else {
                Ok(Some(buf[..read].to_vec()))
            };
        }
        let mut buf = [0_u8; 1024];
        loop {
            if let Some((body, consumed)) = take_one_chunk(&self.pending) {
                self.pending.drain(..consumed);
                return Ok(body);
            }
            let read = self
                .stream
                .read(&mut buf)
                .await
                .map_err(|_| HttpFailure::Unreachable)?;
            if read == 0 {
                return Ok(None);
            }
            self.pending.extend_from_slice(&buf[..read]);
        }
    }
}

/// One chunk's payload, and how many bytes of `raw` it consumed. `None` as
/// the payload is the terminating chunk.
fn take_one_chunk(raw: &[u8]) -> Option<(Option<Vec<u8>>, usize)> {
    let line_end = raw.windows(2).position(|mark| mark == b"\r\n")?;
    let line = std::str::from_utf8(&raw[..line_end]).ok()?;
    let size = usize::from_str_radix(line.split(';').next()?.trim(), 16).ok()?;
    let mut index = line_end + 2;
    if size == 0 {
        return Some((None, index));
    }
    if raw.len() < index + size + 2 {
        return None;
    }
    let payload = raw[index..index + size].to_vec();
    index += size + 2;
    Some((Some(payload), index))
}

#[tokio::test]
async fn a_bearer_debug_omits_the_token() {
    let text = format!("{:?}", Bearer::new("secret-token", 3));
    assert!(!text.contains("secret-token"));
    assert!(text.contains('3'));
}
