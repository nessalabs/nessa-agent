//! A caller's panicking waker stays inside that caller's MCP wait.
//!
//! Rows of "Caller wakers" in `docs/agent_execution/lifecycle.md`. Each test
//! polls the public wait once with a waker that panics, lets the SDK task
//! that completes it run, and then uses the connection again. Without
//! `contain_caller_wake` the reader task or the process reaper unwinds and
//! the later use does not finish.

use super::fixture::{launch, Behaviour, Control, FixtureLauncher};
use super::{conversation, owner};
use crate::domain::mcp_apps::{UiResourceUri, APP_MIME_TYPE};
use crate::infrastructure::acp::sessions::StdioMcpServer;
use crate::infrastructure::clock::manual::ManualClock;
use crate::infrastructure::clock::RuntimeClock;
use crate::infrastructure::mcp::{McpError, McpServerLaunch, McpServers, REQUEST_TIMEOUT};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    future::Future,
    panic::panic_any,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};
use tokio::{
    io::{duplex, AsyncWriteExt},
    sync::oneshot,
};

const BOUND: Duration = Duration::from_secs(3);

struct PanicWake {
    seen: Mutex<Option<oneshot::Sender<()>>>,
}
impl Wake for PanicWake {
    fn wake(self: Arc<Self>) {
        if let Some(seen) = self.seen.lock().unwrap().take() {
            let _ = seen.send(());
        }
        panic!("caller waker");
    }
}

/// Panics with a payload whose drop panics. Tokio catches a `JoinHandle`
/// wake panic and drops that payload outside the catch.
struct PanickingDrop;
impl Drop for PanickingDrop {
    fn drop(&mut self) {
        panic!("caller waker payload drop");
    }
}
struct PayloadWake {
    seen: Mutex<Option<oneshot::Sender<()>>>,
}
impl Wake for PayloadWake {
    fn wake(self: Arc<Self>) {
        if let Some(seen) = self.seen.lock().unwrap().take() {
            let _ = seen.send(());
        }
        panic_any(PanickingDrop);
    }
}

fn panic_waker() -> (Waker, oneshot::Receiver<()>) {
    let (seen, notified) = oneshot::channel();
    let waker = Waker::from(Arc::new(PanicWake {
        seen: Mutex::new(Some(seen)),
    }));
    (waker, notified)
}

fn payload_waker() -> (Waker, oneshot::Receiver<()>) {
    let (seen, notified) = oneshot::channel();
    let waker = Waker::from(Arc::new(PayloadWake {
        seen: Mutex::new(Some(seen)),
    }));
    (waker, notified)
}

fn silent(methods: &[&'static str]) -> Behaviour {
    let mut behaviour = Behaviour::default();
    for method in methods {
        behaviour.silent.insert(method);
    }
    behaviour
}

fn servers_for(behaviour: Behaviour) -> (McpServers, Arc<FixtureLauncher>, Arc<ManualClock>) {
    let launcher = FixtureLauncher::new(behaviour);
    let clock = Arc::new(ManualClock::default());
    let servers =
        McpServers::with_launcher(vec![launch("fixture")], clock.clone(), launcher.clone())
            .unwrap();
    (servers, launcher, clock)
}

fn answer(control: &Control, request: &Value, result: Value) {
    control.send(json!({ "jsonrpc": "2.0", "id": request["id"], "result": result }));
}

async fn request_arrived(control: &Control, method: &str) -> Value {
    control
        .arrived_where(move |message| message["method"] == method)
        .await
}

/// Poll `wait` until it is pending, so the caller's waker is the one the
/// SDK task will invoke.
fn park<F>(wait: Pin<&mut F>, waker: &Waker)
where
    F: Future + ?Sized,
{
    assert!(
        wait.poll(&mut Context::from_waker(waker)).is_pending(),
        "the wait must be parked on the SDK task before it completes"
    );
}

fn app_contents(uri: &UiResourceUri) -> Value {
    json!({
        "contents": [{
            "uri": uri.as_str(),
            "mimeType": APP_MIME_TYPE,
            "text": "<p>chart</p>"
        }]
    })
}

async fn reader_still_serves(servers: &McpServers) {
    let session_id = conversation();
    let echoed = tokio::time::timeout(
        BOUND,
        servers.call_tool(
            &session_id,
            "fixture",
            "echo",
            Some(json!({ "ok": true })),
            BOUND,
        ),
    )
    .await
    .expect("the reader still serves a later call");
    assert_eq!(echoed.unwrap()["structuredContent"]["ok"], true);
}

fn script() -> String {
    format!(
        "{}/tests/infrastructure/mcp/fixtures/server.py",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn real_process(args: &[&str]) -> McpServers {
    let mut arguments = vec![script()];
    arguments.extend(args.iter().copied().map(str::to_owned));
    McpServers::new(
        vec![McpServerLaunch {
            server: StdioMcpServer {
                name: "fixture".into(),
                command: PathBuf::from("/usr/bin/python3"),
                args: arguments,
            },
            working_directory: std::env::temp_dir(),
            environment: BTreeMap::from([("NESSA_FIXTURE".into(), "1".into())]),
        }],
        Arc::new(RuntimeClock::new()),
    )
    .unwrap()
}

#[tokio::test]
async fn panicking_open_waiter_leaves_the_reader_serving() {
    let (servers, launcher, _) = servers_for(silent(&["initialize"]));
    let (waker, notified) = panic_waker();
    let mut open = Box::pin(servers.open("fixture", owner()));
    park(open.as_mut(), &waker);
    let request = request_arrived(&launcher.server(0), "initialize").await;
    answer(
        &launcher.server(0),
        &request,
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "fixture", "version": "1" }
        }),
    );
    tokio::time::timeout(BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    let session = match open.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(Ok(session)) => session,
        Poll::Ready(Err(error)) => panic!("open keeps its session: {error}"),
        Poll::Pending => panic!("open keeps its session, but the wake was lost"),
    };
    reader_still_serves(&servers).await;
    drop(session);
}

#[tokio::test]
async fn panicking_tool_call_waiter_leaves_the_reader_serving() {
    let (servers, launcher, _) = servers_for(silent(&["tools/call"]));
    let _session = servers.open("fixture", owner()).await.unwrap();
    let session_id = conversation();
    let (waker, notified) = panic_waker();
    let mut call = Box::pin(servers.call_tool(
        &session_id,
        "fixture",
        "echo",
        Some(json!({ "n": 1 })),
        REQUEST_TIMEOUT,
    ));
    park(call.as_mut(), &waker);
    let request = request_arrived(&launcher.server(0), "tools/call").await;
    answer(
        &launcher.server(0),
        &request,
        json!({ "structuredContent": { "n": 1 } }),
    );
    tokio::time::timeout(BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        call.as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(result)) if result["structuredContent"]["n"] == 1
    ));
    let mut again = Box::pin(servers.call_tool(
        &session_id,
        "fixture",
        "echo",
        Some(json!({ "n": 2 })),
        REQUEST_TIMEOUT,
    ));
    park(again.as_mut(), Waker::noop());
    launcher.server(0).arrived("tools/call", 2).await;
    let request = launcher.server(0).with_method("tools/call").remove(1);
    answer(
        &launcher.server(0),
        &request,
        json!({ "structuredContent": { "n": 2 } }),
    );
    let followed = tokio::time::timeout(BOUND, again)
        .await
        .expect("reader survived");
    assert_eq!(followed.unwrap()["structuredContent"]["n"], 2);
}

#[tokio::test]
async fn panicking_tool_list_waiter_leaves_the_reader_serving() {
    let (servers, launcher, _) = servers_for(silent(&["tools/list"]));
    let session = servers.open("fixture", owner()).await.unwrap();
    // `open` lists tools in the background. That request is the first
    // `tools/list`; the explicit list is the second, and only its answer
    // may wake this waiter.
    launcher.server(0).arrived("tools/list", 1).await;
    let (waker, notified) = panic_waker();
    let mut listed = Box::pin(session.list_tools());
    park(listed.as_mut(), &waker);
    launcher.server(0).arrived("tools/list", 2).await;
    let request = launcher.server(0).with_method("tools/list").remove(1);
    answer(&launcher.server(0), &request, json!({ "tools": [] }));
    tokio::time::timeout(BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        listed
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(tools)) if tools.is_empty()
    ));
    reader_still_serves(&servers).await;
}

#[tokio::test]
async fn panicking_resource_read_waiter_leaves_the_reader_serving() {
    let (servers, launcher, _) = servers_for(silent(&["resources/read"]));
    let session = servers.open("fixture", owner()).await.unwrap();
    let session_id = conversation();
    let uri = UiResourceUri::new("ui://fixture/chart.html").unwrap();
    let (waker, notified) = panic_waker();
    let mut read = Box::pin(session.read_ui_resource(&uri));
    park(read.as_mut(), &waker);
    let request = request_arrived(&launcher.server(0), "resources/read").await;
    answer(&launcher.server(0), &request, app_contents(&uri));
    tokio::time::timeout(BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        read.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(_))
    ));
    let (waker, notified) = panic_waker();
    let mut app =
        Box::pin(servers.read_app_resource(&session_id, "fixture", &uri, REQUEST_TIMEOUT));
    park(app.as_mut(), &waker);
    launcher.server(0).arrived("resources/read", 2).await;
    let request = launcher.server(0).with_method("resources/read").remove(1);
    answer(&launcher.server(0), &request, app_contents(&uri));
    tokio::time::timeout(BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        app.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(_))
    ));
    reader_still_serves(&servers).await;
}

#[tokio::test]
async fn panicking_open_waiter_does_not_poison_stop() {
    let (servers, launcher, _) = servers_for(silent(&["initialize"]));
    let (waker, _notified) = panic_waker();
    let mut open = Box::pin(servers.open("fixture", owner()));
    assert!(open
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
    request_arrived(&launcher.server(0), "initialize").await;
    tokio::time::timeout(BOUND, servers.stop())
        .await
        .expect("stop returns after the opening waker panics");
    assert!(matches!(
        open.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err(McpError::Stopped))
    ));
    assert!(matches!(
        servers.open("fixture", owner()).await,
        Err(McpError::Stopped)
    ));
}

#[tokio::test]
async fn panicking_serve_waiter_leaves_the_reader_serving() {
    let (servers, launcher, _) = servers_for(silent(&["tools/call"]));
    let session = servers.open("fixture", owner()).await.unwrap();
    let session_id = conversation();
    tokio::time::timeout(BOUND, async {
        loop {
            if servers
                .listed_tool(&session_id, "fixture", "report")
                .unwrap()
                .is_some()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the background tool list finished before serve");
    let kept = session.clone();
    let (harness, served) = duplex(64 * 1024);
    let (input, output) = tokio::io::split(served);
    let (_harness_read, mut harness_write) = tokio::io::split(harness);
    let mut serve = Box::pin(session.serve(input, output));
    let noop = Waker::noop();
    park(serve.as_mut(), noop);
    let frame = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": { "name": "echo", "arguments": { "n": 1 } }
    }))
    .unwrap();
    harness_write.write_all(&frame).await.unwrap();
    harness_write.write_all(b"\n").await.unwrap();
    let control = launcher.server(0);
    tokio::time::timeout(BOUND, async {
        loop {
            park(serve.as_mut(), noop);
            if !control.with_method("tools/call").is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("serve forwarded the harness call");
    let (waker, notified) = panic_waker();
    park(serve.as_mut(), &waker);
    let request = control.with_method("tools/call").remove(0);
    answer(
        &control,
        &request,
        json!({ "structuredContent": { "n": 1 } }),
    );
    tokio::time::timeout(BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    drop(kept);
    let mut again = Box::pin(servers.call_tool(
        &session_id,
        "fixture",
        "echo",
        Some(json!({ "n": 2 })),
        REQUEST_TIMEOUT,
    ));
    park(again.as_mut(), Waker::noop());
    launcher.server(0).arrived("tools/call", 2).await;
    let request = launcher.server(0).with_method("tools/call").remove(1);
    answer(
        &launcher.server(0),
        &request,
        json!({ "structuredContent": { "n": 2 } }),
    );
    let followed = tokio::time::timeout(BOUND, again)
        .await
        .expect("reader survived serve");
    assert_eq!(followed.unwrap()["structuredContent"]["n"], 2);
}

#[tokio::test]
async fn panicking_close_waiter_does_not_stop_the_process_reaper() {
    let servers = real_process(&["--ignore-eof"]);
    let first = servers.open("fixture", owner()).await.unwrap();
    let second = servers.open("fixture", owner()).await.unwrap();
    let (waker, notified) = panic_waker();
    let mut close = Box::pin(first.close());
    park(close.as_mut(), &waker);
    tokio::time::timeout(Duration::from_secs(5), notified)
        .await
        .expect("the process reaper woke the close")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(8), second.close())
        .await
        .expect("a later close still finishes");
    let _ = tokio::time::timeout(Duration::from_secs(8), close).await;
}

const CHILD_ENV: &str = "NESSA_ISSUE442_CHILD";

fn run_in_child_process(child: &str) {
    let name = format!("{}::{child}", module_path!().split_once("::").unwrap().1);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &name, "--ignored", "--test-threads=1"])
        .env(CHILD_ENV, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{child} must not abort or fail: {:?}\n{stdout}\n{stderr}",
        output.status
    );
    assert!(
        stdout.contains("1 passed"),
        "the child ran {name}: {stdout}"
    );
}

/// Tokio catches a panicking `JoinHandle` waker but drops the payload outside
/// that catch. A payload whose drop panics then aborts the process on a
/// multi-thread runtime.
#[test]
fn panicking_payload_stop_waiter_does_not_abort_the_runtime() {
    run_in_child_process("double_fault_stop_waiter_child");
}

#[test]
#[ignore = "run in a child process by panicking_payload_stop_waiter_does_not_abort_the_runtime"]
fn double_fault_stop_waiter_child() {
    assert!(std::env::var_os(CHILD_ENV).is_some());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let servers = real_process(&["--ignore-eof"]);
        let _session = servers.open("fixture", owner()).await.unwrap();
        let (waker, notified) = payload_waker();
        let mut stop = Box::pin(servers.stop());
        park(stop.as_mut(), &waker);
        tokio::time::timeout(Duration::from_secs(8), notified)
            .await
            .expect("stop's close task woke its waiter")
            .unwrap();
        tokio::time::timeout(Duration::from_secs(8), stop)
            .await
            .expect("stop returns after its waiter panics");
        assert!(matches!(
            servers.open("fixture", owner()).await,
            Err(McpError::Stopped)
        ));
    });
}
