//! ADR392 J23 and J27: retained admission snapshot, and the HTTP writer watch.
use super::{record_writer_panic, watch_http_writer, Connection, Shared, State, NOTICES};
use crate::infrastructure::clock::RuntimeClock;
use crate::infrastructure::mcp::http::{HttpSession, SendOutcome};
use crate::infrastructure::mcp::{
    HttpBody, HttpExchange, HttpFailure, HttpMethod, HttpRequest, HttpResponse, McpError,
    NoAuthorization, RemoteMcpUrl, SessionClaims,
};
use async_trait::async_trait;
use serde_json::json;
use std::{
    collections::HashMap,
    future::Future,
    io::Write,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{sync_channel, SyncSender},
        Arc, Condvar, Mutex,
    },
    task::{Context, Wake, Waker},
    thread,
    time::Duration,
};
use tokio::{
    io::duplex,
    runtime::Handle,
    sync::{broadcast, oneshot, watch},
    time::timeout,
};
use uuid::Uuid;

const BOUND: Duration = Duration::from_secs(3);

struct ReleasePublication(SyncSender<()>);
impl Drop for ReleasePublication {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

#[tokio::test]
async fn recorded_end_refuses_admission_while_watch_publication_is_held() {
    let (input, _server_output) = duplex(1024);
    let (output, _server_input) = duplex(1024);
    let connection = Arc::new(Connection::open(
        input,
        output,
        Arc::new(RuntimeClock::new()),
    ));
    let watched = connection.shared.ended.subscribe();
    let (held, holding) = oneshot::channel();
    let (release, resumed) = sync_channel(1);
    let release = ReleasePublication(release);
    let holder = tokio::task::spawn_blocking(move || {
        let value = watched.borrow();
        held.send(()).unwrap();
        resumed.recv().unwrap();
        drop(value);
    });
    timeout(BOUND, holding).await.unwrap().unwrap();
    let ending_connection = connection.clone();
    let ending =
        tokio::task::spawn_blocking(move || ending_connection.shared.end(McpError::Closed));
    timeout(BOUND, async {
        while connection.end_cause().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // subscribe clones/version-loads, without borrowing the held watch value.
    // The retained snapshot must reject before polling that value or ready enqueue.
    let notifying_connection = connection.clone();
    let runtime = Handle::current();
    let mut notifying = tokio::task::spawn_blocking(move || {
        runtime.block_on(notifying_connection.notify("notifications/test", None))
    });
    let notified = timeout(BOUND, &mut notifying).await;
    drop(release);
    holder.await.unwrap();
    ending.await.unwrap();
    if notified.is_err() {
        notifying.await.unwrap().unwrap_err();
    }
    connection.close(McpError::Closed);
    assert!(
        matches!(notified, Ok(Ok(Err(McpError::Closed)))),
        "retained snapshot must return before watch publication: {notified:?}"
    );
}

fn open_state() -> Arc<Shared> {
    Arc::new(Shared {
        state: Mutex::new(State {
            pending: HashMap::new(),
            ended: None,
        }),
        ended: watch::channel(None).0,
        notices: broadcast::channel(NOTICES).0,
        next_id: AtomicU64::new(1),
    })
}

fn recorded_cause(shared: &Shared) -> Option<McpError> {
    shared.state.lock().expect("connection state").ended.clone()
}

struct RecordedExchange {
    methods: Mutex<Vec<HttpMethod>>,
}

#[async_trait]
impl HttpExchange for RecordedExchange {
    async fn exchange(&self, request: HttpRequest) -> Result<HttpResponse, HttpFailure> {
        self.methods.lock().expect("methods").push(request.method);
        let initialize = serde_json::from_slice::<serde_json::Value>(&request.body)
            .ok()
            .and_then(|message| {
                message
                    .get("method")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .as_deref()
            == Some("initialize");
        if initialize {
            return Ok(HttpResponse {
                status: 200,
                headers: vec![
                    ("content-type".into(), "application/json".into()),
                    ("mcp-session-id".into(), "held".into()),
                ],
                body: HttpBody::Buffered(
                    serde_json::to_vec(&json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": { "protocolVersion": "2025-06-18" }
                    }))
                    .expect("initialize body"),
                ),
            });
        }
        Ok(HttpResponse {
            status: 202,
            headers: vec![],
            body: HttpBody::Buffered(vec![]),
        })
    }
}

fn open_session(
    exchange: Arc<RecordedExchange>,
) -> (
    Arc<HttpSession>,
    tokio::sync::mpsc::Receiver<Result<crate::infrastructure::mcp::http::HttpMessage, McpError>>,
) {
    HttpSession::open(
        Uuid::from_u128(1),
        RemoteMcpUrl::parse("http://127.0.0.1/mcp").expect("url"),
        exchange,
        Arc::new(NoAuthorization),
        Arc::new(SessionClaims::default()),
    )
}

struct Log(Arc<Mutex<Vec<u8>>>);
impl Write for Log {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn closed_cause_survives_a_panicked_writer() {
    let exchange = Arc::new(RecordedExchange {
        methods: Mutex::new(vec![]),
    });
    let (session, _incoming) = open_session(exchange.clone());
    match session.dispatch(br#"{"id":1,"method":"initialize"}"#).await {
        SendOutcome::Done => {}
        SendOutcome::FailCall { id, error } => panic!("initialize failed {id:?}: {error}"),
        SendOutcome::End(error) => panic!("initialize ended: {error}"),
    }
    let shared = open_state();
    shared.end(McpError::Closed);
    let writer = tokio::spawn(async { panic!("bearer secret-token-not-for-logs") });
    timeout(BOUND, async {
        while !writer.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("panicked writer finishes");
    let recorded = Arc::new(Mutex::new(Vec::<u8>::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::ERROR)
        .with_writer({
            let recorded = Arc::clone(&recorded);
            move || Log(Arc::clone(&recorded))
        })
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let mut finished = session.finished();
    watch_http_writer(writer, Arc::clone(&shared), Arc::clone(&session)).await;
    let log = String::from_utf8(recorded.lock().expect("log").clone()).expect("utf8 log");
    assert!(
        log.contains("custom HTTP writer panicked"),
        "writer panic was not logged: {log}"
    );
    assert!(
        !log.contains("secret-token-not-for-logs"),
        "panic payload leaked into the log: {log}"
    );
    assert_eq!(recorded_cause(&shared), Some(McpError::Closed));
    timeout(BOUND, finished.wait_for(|done| *done))
        .await
        .expect("delete finishes")
        .expect("finished watch");
    assert_eq!(
        exchange
            .methods
            .lock()
            .expect("methods")
            .iter()
            .filter(|method| **method == HttpMethod::Delete)
            .count(),
        1
    );
}

/// Sees the panic marker only after [`record_writer_panic`] has stored
/// `ServerGone`. A log that runs first leaves this false.
struct EndBeforeLog {
    shared: Arc<Shared>,
    saw_recorded_end: Arc<AtomicBool>,
}
impl tracing::Subscriber for EndBeforeLog {
    fn register_callsite(
        &self,
        _metadata: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::sometimes()
    }
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "nessa_sdk::infrastructure::mcp::connection"
            && *metadata.level() == tracing::Level::ERROR
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut message = String::new();
        event.record(&mut PanicMarker(&mut message));
        if message.contains("custom HTTP writer panicked") {
            self.saw_recorded_end.store(
                recorded_cause(&self.shared) == Some(McpError::ServerGone),
                Ordering::SeqCst,
            );
        }
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}
struct PanicMarker<'a>(&'a mut String);
impl tracing::field::Visit for PanicMarker<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        }
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" && self.0.is_empty() {
            self.0.push_str(&format!("{value:?}"));
        }
    }
}

#[tokio::test]
async fn writer_panic_log_follows_the_recorded_end() {
    let exchange = Arc::new(RecordedExchange {
        methods: Mutex::new(vec![]),
    });
    let (session, _incoming) = open_session(exchange);
    let shared = open_state();
    let saw_recorded_end = Arc::new(AtomicBool::new(false));
    let subscriber = EndBeforeLog {
        shared: Arc::clone(&shared),
        saw_recorded_end: Arc::clone(&saw_recorded_end),
    };
    let _guard = tracing::subscriber::set_default(subscriber);
    tracing::callsite::rebuild_interest_cache();
    let mut finished = session.finished();
    record_writer_panic(&session, &shared);
    assert!(
        saw_recorded_end.load(Ordering::SeqCst),
        "panic log ran before ServerGone was recorded"
    );
    assert_eq!(recorded_cause(&shared), Some(McpError::ServerGone));
    timeout(BOUND, finished.wait_for(|done| *done))
        .await
        .expect("delete finishes")
        .expect("finished watch");
}

struct HoldWake {
    entered: tokio::sync::Notify,
    released: Mutex<bool>,
    ready: Condvar,
}
impl HoldWake {
    fn release(&self) {
        *self.released.lock().expect("hold") = true;
        self.ready.notify_all();
    }
}
impl Wake for HoldWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.entered.notify_one();
        let mut released = self.released.lock().expect("hold");
        while !*released {
            released = self.ready.wait(released).expect("hold");
        }
    }
}
struct ReleaseHold(Arc<HoldWake>);
impl Drop for ReleaseHold {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[tokio::test]
async fn writer_watch_fences_before_recording_the_end() {
    let exchange = Arc::new(RecordedExchange {
        methods: Mutex::new(vec![]),
    });
    let (session, _incoming) = open_session(exchange.clone());
    let shared = open_state();
    let (sender, mut receiver) = oneshot::channel();
    shared
        .state
        .lock()
        .expect("connection state")
        .pending
        .insert(1, sender);
    let hold = Arc::new(HoldWake {
        entered: tokio::sync::Notify::new(),
        released: Mutex::new(false),
        ready: Condvar::new(),
    });
    let release = ReleaseHold(Arc::clone(&hold));
    let waker = Waker::from(Arc::clone(&hold));
    assert!(std::pin::Pin::new(&mut receiver)
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
    let writer = tokio::spawn(async { panic!("local adapter writer panicked") });
    timeout(BOUND, async {
        while !writer.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("panicked writer finishes");
    let watched_session = Arc::clone(&session);
    let watched_shared = Arc::clone(&shared);
    let running = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(watch_http_writer(writer, watched_shared, watched_session));
    });
    timeout(BOUND, hold.entered.notified())
        .await
        .expect("end wakes the pending call");
    assert_eq!(recorded_cause(&shared), Some(McpError::ServerGone));
    let dispatched = session
        .dispatch(br#"{"jsonrpc":"2.0","method":"notifications/direct"}"#)
        .await;
    assert!(
        matches!(dispatched, SendOutcome::End(McpError::Closed)),
        "shutdown must already fence admission: {dispatched:?}"
    );
    assert_eq!(
        exchange
            .methods
            .lock()
            .expect("methods")
            .iter()
            .filter(|method| **method == HttpMethod::Post)
            .count(),
        0
    );
    drop(release);
    running.join().expect("watch thread");
    drop(receiver);
}
