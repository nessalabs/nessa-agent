//! ADR 392 J28: a custom adapter panic in a session task other than the writer.
use super::super::connection::{Connection, HttpTask};
use super::super::http::{watch_http_task, HttpSession};
use super::super::{
    HttpBody, HttpChunks, HttpExchange, HttpFailure, HttpMethod, HttpRequest, HttpResponse,
    McpError, NoAuthorization, RemoteMcpUrl, SessionClaims, INITIALIZE_TIMEOUT,
};
use crate::infrastructure::clock::manual::ManualClock;
use crate::infrastructure::clock::RuntimeClock;
use async_trait::async_trait;
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{watch, Notify, Semaphore};
use uuid::Uuid;

const SECRET: &str = "bearer secret-token-not-for-logs";

struct Gate {
    entered: AtomicUsize,
    changed: Notify,
    permits: Semaphore,
}

impl Gate {
    fn new() -> Self {
        Self {
            entered: AtomicUsize::new(0),
            changed: Notify::new(),
            permits: Semaphore::new(0),
        }
    }

    async fn enter(&self) {
        self.entered.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_one();
        self.permits.acquire().await.expect("gate").forget();
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

struct PanicBody;

#[async_trait]
impl HttpChunks for PanicBody {
    async fn next(&mut self) -> Result<Option<Vec<u8>>, HttpFailure> {
        panic!("{SECRET}");
    }
}

fn empty(status: u16) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![],
        body: HttpBody::Buffered(vec![]),
    }
}

fn initialize_json(id: u64) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![
            ("content-type".into(), "application/json".into()),
            ("mcp-session-id".into(), "held".into()),
        ],
        body: HttpBody::Buffered(
            serde_json::to_vec(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "protocolVersion": "2025-06-18" }
            }))
            .expect("initialize body"),
        ),
    }
}

fn event_stream(body: HttpBody) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![("content-type".into(), "text/event-stream".into())],
        body,
    }
}

struct Seen(Mutex<Vec<HttpRequest>>);

impl Seen {
    fn push(&self, request: HttpRequest) {
        self.0.lock().expect("seen").push(request);
    }

    fn count(&self, predicate: impl Fn(&HttpRequest) -> bool) -> usize {
        self.0
            .lock()
            .expect("seen")
            .iter()
            .filter(|request| predicate(request))
            .count()
    }
}

struct ReplyExchange {
    seen: Seen,
}

#[async_trait]
impl HttpExchange for ReplyExchange {
    async fn exchange(&self, request: HttpRequest) -> Result<HttpResponse, HttpFailure> {
        let method = request_method(&request);
        let http_method = request.method;
        self.seen.push(request);
        match http_method {
            HttpMethod::Get => Ok(empty(405)),
            HttpMethod::Delete => Ok(empty(200)),
            HttpMethod::Post => match method.as_deref() {
                Some("initialize") => Ok(initialize_json(1)),
                Some("tools/list") => Ok(event_stream(HttpBody::Stream(Box::new(PanicBody)))),
                _ => Ok(empty(202)),
            },
        }
    }
}

struct LegacyExchange {
    seen: Seen,
}

#[async_trait]
impl HttpExchange for LegacyExchange {
    async fn exchange(&self, request: HttpRequest) -> Result<HttpResponse, HttpFailure> {
        let method = request.method;
        self.seen.push(request);
        match method {
            HttpMethod::Post => Ok(empty(405)),
            HttpMethod::Get => Ok(event_stream(HttpBody::Stream(Box::new(PanicBody)))),
            HttpMethod::Delete => Ok(empty(200)),
        }
    }
}

struct ServerExchange {
    seen: Seen,
    gate: Arc<Gate>,
}

#[async_trait]
impl HttpExchange for ServerExchange {
    async fn exchange(&self, request: HttpRequest) -> Result<HttpResponse, HttpFailure> {
        let method = request.method;
        self.seen.push(request);
        match method {
            HttpMethod::Post => Ok(initialize_json(1)),
            HttpMethod::Get => {
                self.gate.enter().await;
                Ok(event_stream(HttpBody::Stream(Box::new(PanicBody))))
            }
            HttpMethod::Delete => Ok(empty(200)),
        }
    }
}

struct RecoveryExchange {
    seen: Seen,
    clock: Arc<ManualClock>,
    expired: AtomicBool,
}

#[async_trait]
impl HttpExchange for RecoveryExchange {
    async fn exchange(&self, request: HttpRequest) -> Result<HttpResponse, HttpFailure> {
        let method = request_method(&request);
        let id = serde_json::from_slice::<serde_json::Value>(&request.body)
            .ok()
            .and_then(|message| message.get("id").and_then(|id| id.as_u64()));
        self.seen.push(request);
        if method.as_deref() == Some("initialize") && id == Some(0) {
            self.clock.advance(INITIALIZE_TIMEOUT);
            panic!("{SECRET}");
        }
        if method.as_deref() == Some("tools/list") && !self.expired.swap(true, Ordering::SeqCst) {
            return Ok(empty(404));
        }
        if method.as_deref() == Some("initialize") {
            return Ok(initialize_json(id.unwrap_or(1)));
        }
        Ok(empty(405))
    }
}

fn request_method(request: &HttpRequest) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(&request.body)
        .ok()?
        .get("method")?
        .as_str()
        .map(str::to_owned)
}

fn open(
    exchange: Arc<dyn HttpExchange>,
    clock: Arc<dyn crate::infrastructure::clock::Clock>,
) -> (Arc<Connection>, Arc<HttpSession>) {
    let (session, incoming) = HttpSession::open(
        Uuid::from_u128(1),
        RemoteMcpUrl::parse("https://mcp.example:8443/secret-path?access_token=bearer-secret")
            .expect("url"),
        exchange,
        Arc::new(NoAuthorization),
        Arc::new(SessionClaims::default()),
    );
    let connection = Arc::new(Connection::open_http(session.clone(), incoming, clock));
    (connection, session)
}

struct Capture<'a>(&'a mut String);

impl tracing::field::Visit for Capture<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.push_str(field.name());
        self.0.push('=');
        self.0.push_str(value);
        self.0.push('\n');
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(field.name());
        self.0.push('=');
        self.0.push_str(&format!("{value:?}"));
        self.0.push('\n');
    }
}

struct PanicLog {
    connection: Option<Arc<Connection>>,
    marker: &'static str,
    text: Arc<Mutex<String>>,
    end_before_log: Arc<AtomicBool>,
}

impl tracing::Subscriber for PanicLog {
    fn register_callsite(
        &self,
        _metadata: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::sometimes()
    }

    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() == tracing::Level::ERROR
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut message = String::new();
        event.record(&mut Capture(&mut message));
        if message.contains(self.marker) {
            if let Some(connection) = &self.connection {
                self.end_before_log.store(
                    connection.end_cause() == Some(McpError::ServerGone),
                    Ordering::SeqCst,
                );
            }
        }
        self.text.lock().expect("log").push_str(&message);
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

struct Watching {
    marker: &'static str,
    text: Arc<Mutex<String>>,
    end_before_log: Arc<AtomicBool>,
    check_order: bool,
    _guard: tracing::subscriber::DefaultGuard,
}

fn watch_log(connection: Arc<Connection>, marker: &'static str) -> Watching {
    let text = Arc::new(Mutex::new(String::new()));
    let end_before_log = Arc::new(AtomicBool::new(false));
    let subscriber = PanicLog {
        connection: Some(connection),
        marker,
        text: Arc::clone(&text),
        end_before_log: Arc::clone(&end_before_log),
    };
    let guard = tracing::subscriber::set_default(subscriber);
    tracing::callsite::rebuild_interest_cache();
    Watching {
        marker,
        text,
        end_before_log,
        check_order: true,
        _guard: guard,
    }
}

/// The marker for a session that has no connection end owner.
fn watch_marker(marker: &'static str) -> Watching {
    let text = Arc::new(Mutex::new(String::new()));
    let end_before_log = Arc::new(AtomicBool::new(false));
    let subscriber = PanicLog {
        connection: None,
        marker,
        text: Arc::clone(&text),
        end_before_log: Arc::clone(&end_before_log),
    };
    let guard = tracing::subscriber::set_default(subscriber);
    tracing::callsite::rebuild_interest_cache();
    Watching {
        marker,
        text,
        end_before_log,
        check_order: false,
        _guard: guard,
    }
}

impl Watching {
    fn assert_secret_stays_out(&self) {
        let text = self.text.lock().expect("log").clone();
        assert!(
            text.contains(self.marker),
            "panic marker was not logged: {text}"
        );
        if self.check_order {
            assert!(
                self.end_before_log.load(Ordering::SeqCst),
                "panic log ran before ServerGone was recorded: {text}"
            );
        }
        assert!(
            !text.contains(SECRET)
                && !text.contains("secret-user")
                && !text.contains("secret-path")
                && !text.contains("bearer-secret"),
            "request contents leaked into the log: {text}"
        );
    }
}

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(3), future)
        .await
        .expect("session task panic completes")
}

#[tokio::test]
async fn reply_reader_panic_ends_server_gone() {
    let seen = Seen(Mutex::new(vec![]));
    let exchange = Arc::new(ReplyExchange { seen });
    let (connection, session) = open(exchange.clone(), Arc::new(RuntimeClock::new()));
    bounded(connection.call("initialize", None))
        .await
        .expect("initialize")
        .expect("initialize result");
    let mut finished = session.finished();
    let log = watch_log(Arc::clone(&connection), "custom HTTP reply reader panicked");
    assert_eq!(
        bounded(connection.call("tools/list", None))
            .await
            .expect_err("reply reader panic"),
        McpError::ServerGone
    );
    assert_eq!(connection.end_cause(), Some(McpError::ServerGone));
    log.assert_secret_stays_out();
    bounded(finished.wait_for(|done| *done))
        .await
        .expect("finished");
    assert_eq!(
        exchange
            .seen
            .count(|request| request.method == HttpMethod::Delete),
        1
    );
}

#[tokio::test]
async fn legacy_stream_panic_ends_server_gone() {
    let exchange = Arc::new(LegacyExchange {
        seen: Seen(Mutex::new(vec![])),
    });
    let (connection, session) = open(exchange.clone(), Arc::new(RuntimeClock::new()));
    let mut finished = session.finished();
    let log = watch_log(
        Arc::clone(&connection),
        "custom HTTP legacy stream panicked",
    );
    let error = bounded(connection.call("initialize", None))
        .await
        .expect_err("legacy stream panic");
    assert_eq!(error, McpError::ServerGone);
    assert_eq!(connection.end_cause(), Some(McpError::ServerGone));
    log.assert_secret_stays_out();
    bounded(finished.wait_for(|done| *done))
        .await
        .expect("finished");
    assert_eq!(
        exchange
            .seen
            .count(|request| request.method == HttpMethod::Delete),
        0
    );
}

#[tokio::test]
async fn server_stream_panic_ends_server_gone() {
    let gate = Arc::new(Gate::new());
    let exchange = Arc::new(ServerExchange {
        seen: Seen(Mutex::new(vec![])),
        gate: Arc::clone(&gate),
    });
    let (connection, session) = open(exchange.clone(), Arc::new(RuntimeClock::new()));
    bounded(connection.call("initialize", None))
        .await
        .expect("initialize")
        .expect("initialize result");
    gate.reached().await;
    let mut finished = session.finished();
    let log = watch_log(
        Arc::clone(&connection),
        "custom HTTP server stream panicked",
    );
    gate.release();
    assert_eq!(bounded(connection.ended()).await, McpError::ServerGone);
    assert_eq!(
        bounded(connection.call("tools/list", None))
            .await
            .expect_err("server stream panic"),
        McpError::ServerGone
    );
    log.assert_secret_stays_out();
    bounded(finished.wait_for(|done| *done))
        .await
        .expect("finished");
    assert_eq!(
        exchange
            .seen
            .count(|request| request.method == HttpMethod::Delete),
        1
    );
}

#[tokio::test]
async fn recovery_panic_ends_server_gone() {
    let clock = Arc::new(ManualClock::default());
    let exchange = Arc::new(RecoveryExchange {
        seen: Seen(Mutex::new(vec![])),
        clock: Arc::clone(&clock),
        expired: AtomicBool::new(false),
    });
    let (connection, session) = open(exchange.clone(), clock);
    bounded(connection.call("initialize", None))
        .await
        .expect("initialize")
        .expect("initialize result");
    let mut finished = session.finished();
    let log = watch_log(Arc::clone(&connection), "custom HTTP recovery panicked");
    assert_eq!(
        bounded(connection.call("tools/list", None))
            .await
            .expect_err("expired session"),
        McpError::SessionExpired
    );
    assert_eq!(bounded(connection.ended()).await, McpError::ServerGone);
    assert_eq!(
        bounded(connection.call("tools/list", None))
            .await
            .expect_err("recovery panic"),
        McpError::ServerGone
    );
    assert_ne!(connection.end_cause(), Some(McpError::Timeout));
    assert_ne!(connection.end_cause(), Some(McpError::Unconfirmed));
    log.assert_secret_stays_out();
    bounded(finished.wait_for(|done| *done))
        .await
        .expect("finished");
    assert_eq!(
        exchange
            .seen
            .count(|request| request.method == HttpMethod::Delete),
        0
    );
}

#[tokio::test]
async fn session_task_watch_records_escaped_panic() {
    let (connection, session) = open(
        Arc::new(ReplyExchange {
            seen: Seen(Mutex::new(vec![])),
        }),
        Arc::new(RuntimeClock::new()),
    );
    let task = tokio::spawn(async { panic!("{SECRET}") });
    bounded(async {
        while !task.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let log = watch_log(
        Arc::clone(&connection),
        "custom HTTP server stream panicked",
    );
    let (settled, _observed) = watch::channel(None);
    watch_http_task(
        task,
        Arc::downgrade(&session),
        settled,
        HttpTask::ServerStream,
    )
    .await;
    assert_eq!(connection.end_cause(), Some(McpError::ServerGone));
    log.assert_secret_stays_out();
}

#[tokio::test]
async fn unset_owner_still_logs_the_panic_marker() {
    let (session, _incoming) = HttpSession::open(
        Uuid::from_u128(2),
        RemoteMcpUrl::parse("https://mcp.example:8443/secret-path?access_token=bearer-secret")
            .expect("url"),
        Arc::new(ReplyExchange {
            seen: Seen(Mutex::new(vec![])),
        }),
        Arc::new(NoAuthorization),
        Arc::new(SessionClaims::default()),
    );
    let task = tokio::spawn(async { panic!("{SECRET}") });
    bounded(async {
        while !task.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let log = watch_marker("custom HTTP server stream panicked");
    let (settled, _observed) = watch::channel(None);
    watch_http_task(
        task,
        Arc::downgrade(&session),
        settled,
        HttpTask::ServerStream,
    )
    .await;
    log.assert_secret_stays_out();
}
