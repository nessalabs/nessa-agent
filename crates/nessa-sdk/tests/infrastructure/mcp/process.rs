//! The client against a real server process (`fixtures/server.py`): launching
//! it as configured, its exit and the next generation, a stop that has to
//! kill, and the shared session.
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

async fn call(harness: &mut Harness, id: u64, name: &str) -> Value {
    harness
        .send(json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name } }))
        .await;
    harness.next().await.expect("an answer")
}

#[tokio::test]
async fn a_configured_server_runs_with_only_what_it_was_given() {
    let (servers, directory) = process(&[]);
    let tools = servers.list_tools("fixture").await.unwrap();
    assert_eq!(tools.len(), 4);
    let chart = McpTool::new("fixture", "show_chart").unwrap();
    assert_eq!(
        servers.tool_ui(&chart).unwrap().resource_uri().as_str(),
        "ui://fixture/chart.html"
    );
    let mut harness = Harness::attach(servers.stand_in("fixture").await.unwrap());
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
async fn the_agents_handle_resolves_for_the_app_until_the_server_exits() {
    let (servers, _) = process(&[]);
    let mut agent = Harness::attach(servers.stand_in("fixture").await.unwrap());
    let handle = call(&mut agent, 1, "remember").await["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    let uri = UiResourceUri::new(format!("ui://fixture/handle/{handle}")).unwrap();
    assert_eq!(
        servers
            .read_ui_resource("fixture", &uri)
            .await
            .unwrap()
            .html(),
        format!("<p>{handle}</p>")
    );
    // The server exits: its stand-in ends, and the next use is a new session
    // that never gave the handle out.
    agent
        .send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "exit" } }))
        .await;
    assert_eq!(agent.next().await, None);
    assert!(matches!(
        servers.read_ui_resource("fixture", &uri).await,
        Err(McpError::Remote { code: -32002, .. })
    ));
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
        servers.list_tools("missing").await,
        Err(McpError::Start(_))
    ));
}

#[tokio::test]
async fn stopping_kills_a_server_that_outlives_its_stdin() {
    let (servers, _) = process(&["--ignore-eof"]);
    let mut harness = Harness::attach(servers.stand_in("fixture").await.unwrap());
    let pid = call(&mut harness, 1, "where").await["result"]["structuredContent"]["pid"]
        .as_i64()
        .unwrap() as libc::pid_t;
    let alive = || unsafe { libc::kill(pid, 0) } == 0;
    assert!(alive());
    let started = std::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(10), servers.stop())
        .await
        .unwrap();
    assert!(
        started.elapsed() >= Duration::from_secs(2),
        "waited out the grace first"
    );
    // Killed and reaped: no such process, not even a zombie.
    assert!(!alive());
}
