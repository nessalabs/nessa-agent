//! The client against real server processes (`fixtures/server.py`):
//! launching one as configured, a process per session and each apart from
//! the others, closing one with everything it started, and the shared
//! session.
use super::Harness;
use crate::domain::agent_execution::tools::McpTool;
use crate::domain::mcp_apps::UiResourceUri;
use crate::infrastructure::acp::sessions::StdioMcpServer;
use crate::infrastructure::clock::RuntimeClock;
use crate::infrastructure::mcp::{McpError, McpServerLaunch, McpServers};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

fn script() -> String {
    format!(
        "{}/tests/infrastructure/mcp/fixtures/server.py",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn process(args: &[&str]) -> (McpServers, PathBuf) {
    let directory = std::env::temp_dir();
    let mut arguments = vec![script()];
    arguments.extend(args.iter().map(|arg| (*arg).to_owned()));
    let servers = McpServers::new(
        vec![McpServerLaunch {
            server: StdioMcpServer {
                name: "fixture".into(),
                command: PathBuf::from("/usr/bin/python3"),
                args: arguments,
            },
            working_directory: directory.clone(),
            environment: BTreeMap::from([("NESSA_FIXTURE".into(), "1".into())]),
        }],
        Arc::new(RuntimeClock::new()),
    )
    .unwrap();
    (servers, directory)
}

/// Read until the stand-in closes. A call still waiting when the server ended
/// may be answered first, with the server's end as its error, and nothing
/// else may arrive.
async fn closes(harness: &mut Harness, waiting: u64) {
    while let Some(frame) = harness.next().await {
        assert_eq!(frame["id"], waiting, "{frame}");
        assert_eq!(frame["error"]["message"], "the MCP server ended");
    }
}

/// A session on the fixture whose tools have been listed, as a harness lists
/// before it calls.
async fn listed(servers: &McpServers) -> crate::infrastructure::mcp::McpSession {
    let session = servers.open("fixture").await.unwrap();
    session.list_tools().await.unwrap();
    session
}

async fn call(harness: &mut Harness, id: u64, name: &str) -> Value {
    harness
        .send(json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name } }))
        .await;
    harness.next().await.expect("an answer")
}

fn alive(pid: i64) -> bool {
    // SAFETY: signal 0 only asks whether the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Until `pid` is gone, within five seconds.
async fn gone(pid: i64) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while alive(pid) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("process {pid} is still running"));
}

#[tokio::test]
async fn a_configured_server_runs_with_only_what_it_was_given() {
    let (servers, directory) = process(&[]);
    let session = servers.open("fixture").await.unwrap();
    let tools = session.list_tools().await.unwrap();
    assert_eq!(tools.len(), 5);
    let chart = McpTool::new("fixture", "show_chart").unwrap();
    assert_eq!(
        servers.tool_ui(&chart).unwrap().resource_uri().as_str(),
        "ui://fixture/chart.html"
    );
    let mut harness = Harness::attach(session);
    let place = call(&mut harness, 1, "where").await;
    let cwd = PathBuf::from(
        place["result"]["structuredContent"]["cwd"]
            .as_str()
            .unwrap(),
    );
    assert_eq!(
        cwd.canonicalize().unwrap(),
        directory.canonicalize().unwrap()
    );
    // Its environment is exactly what it was given (Python may add its own
    // locale variable on some platforms; nothing of this process's is there).
    let env: Vec<String> =
        serde_json::from_value(place["result"]["structuredContent"]["env"].clone()).unwrap();
    assert!(env.contains(&"NESSA_FIXTURE".to_owned()));
    assert!(
        !env.contains(&"PATH".to_owned()) && !env.contains(&"HOME".to_owned()),
        "{env:?}"
    );
    servers.stop().await;
}

#[tokio::test]
async fn two_sessions_are_two_processes_and_neither_waits_on_or_outlives_the_other() {
    let (servers, _) = process(&[]);
    let mut first = Harness::attach(listed(&servers).await);
    let mut second = Harness::attach(listed(&servers).await);
    let pid = |answer: Value| {
        answer["result"]["structuredContent"]["pid"]
            .as_i64()
            .unwrap()
    };
    let first_pid = pid(call(&mut first, 1, "where").await);
    let second_pid = pid(call(&mut second, 1, "where").await);
    assert_ne!(first_pid, second_pid);
    // A long call in one does not hold up the other.
    first
        .send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "sleep" } }))
        .await;
    let started = std::time::Instant::now();
    assert_eq!(
        pid(call(&mut second, 2, "where").await),
        second_pid,
        "answered while the other sleeps"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    // One server exiting ends its stand-in, and leaves the other serving.
    second
        .send(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "exit" } }))
        .await;
    closes(&mut second, 3).await;
    gone(second_pid).await;
    assert!(alive(first_pid));
    servers.stop().await;
    gone(first_pid).await;
}

#[tokio::test]
async fn the_agents_handle_resolves_in_its_own_session_and_dies_with_it() {
    let (servers, _) = process(&[]);
    let session = listed(&servers).await;
    let mut agent = Harness::attach(session.clone());
    let handle = call(&mut agent, 1, "remember").await["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    let uri = UiResourceUri::new(format!("ui://fixture/handle/{handle}")).unwrap();
    assert_eq!(
        session.read_ui_resource(&uri).await.unwrap().html(),
        format!("<p>{handle}</p>")
    );
    // Another conversation's session never gave it out.
    let other = servers.open("fixture").await.unwrap();
    assert!(matches!(
        other.read_ui_resource(&uri).await,
        Err(McpError::Remote { code: -32002, .. })
    ));
    // The server exits: its stand-in ends, and so does the session.
    agent
        .send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "exit" } }))
        .await;
    closes(&mut agent, 2).await;
    assert!(session.read_ui_resource(&uri).await.is_err());
    servers.stop().await;
}

#[tokio::test]
async fn a_server_that_cannot_be_launched_is_a_start_failure() {
    let servers = McpServers::new(
        vec![McpServerLaunch {
            server: StdioMcpServer {
                name: "missing".into(),
                command: PathBuf::from("/nonexistent/server"),
                args: vec![],
            },
            working_directory: std::env::temp_dir(),
            environment: BTreeMap::new(),
        }],
        Arc::new(RuntimeClock::new()),
    )
    .unwrap();
    assert!(matches!(
        servers.open("missing").await,
        Err(McpError::Start(_))
    ));
}

#[tokio::test]
async fn a_stand_in_that_ends_stops_its_server_and_everything_it_started() {
    let (servers, _) = process(&["--ignore-eof", "--child"]);
    let mut harness = Harness::attach(listed(&servers).await);
    let place = call(&mut harness, 1, "where").await;
    let pid = place["result"]["structuredContent"]["pid"]
        .as_i64()
        .unwrap();
    let child = place["result"]["structuredContent"]["child"]
        .as_i64()
        .unwrap();
    assert!(alive(pid) && alive(child));
    let started = std::time::Instant::now();
    // The harness goes away; the server ignores its closed stdin, so it is
    // killed after the grace, and its own child with it.
    drop(harness);
    gone(pid).await;
    gone(child).await;
    assert!(
        started.elapsed() >= Duration::from_secs(2),
        "the grace came first"
    );
}

#[tokio::test]
async fn a_closed_session_has_no_server_process_left_once_close_returns() {
    let (servers, _) = process(&["--ignore-eof"]);
    let session = listed(&servers).await;
    let pid = i64::from(session.process_id().unwrap());
    // It ignores its closed stdin: killed after the grace, and reaped before
    // close returns, so nothing of it is left — not even an exit status.
    session.close().await;
    assert!(!alive(pid), "process {pid} is left after close returned");
}

#[tokio::test]
async fn a_session_dropped_without_closing_kills_its_process_group() {
    let (servers, _) = process(&["--ignore-eof", "--child"]);
    let session = servers.open("fixture").await.unwrap();
    let pid = i64::from(session.process_id().unwrap());
    // Its child, found as the one process whose parent it is.
    let children = std::process::Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .unwrap();
    let child: i64 = String::from_utf8(children.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // Nothing closes it: its last handles simply go.
    drop(session);
    drop(servers);
    gone(pid).await;
    gone(child).await;
}
