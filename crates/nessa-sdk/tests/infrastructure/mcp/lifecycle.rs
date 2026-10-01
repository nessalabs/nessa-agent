//! A server's generations ("A server's connection" table).
use super::fixture::{launch, Behaviour, FixtureLauncher};
use super::{servers, Harness};
use crate::domain::mcp_apps::UiResourceUri;
use crate::infrastructure::clock::manual::ManualClock;
use crate::infrastructure::mcp::{McpError, McpServers, INITIALIZE_TIMEOUT};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn uses_during_a_start_wait_for_that_start() {
    let mut behaviour = Behaviour::default();
    behaviour.silent.insert("initialize");
    let (servers, launcher, _) = servers(behaviour);
    let first = tokio::spawn({
        let servers = servers.clone();
        async move { servers.list_tools("fixture").await.map(|tools| tools.len()) }
    });
    let second = tokio::spawn({
        let servers = servers.clone();
        async move { servers.stand_in("fixture").await.map(|_| ()) }
    });
    let server = loop {
        if launcher.launches() == 1 {
            break launcher.generation(0);
        }
        tokio::task::yield_now().await;
    };
    server.arrived("initialize", 1).await;
    let id = server.with_method("initialize")[0]["id"].clone();
    server.send(json!({ "jsonrpc": "2.0", "id": id,
                        "result": { "protocolVersion": "2025-06-18", "capabilities": {} } }));
    assert_eq!(first.await.unwrap(), Ok(2));
    assert_eq!(second.await.unwrap(), Ok(()));
    // One process for both.
    assert_eq!(launcher.launches(), 1);
}

#[tokio::test]
async fn a_start_that_cannot_launch_fails_and_the_next_use_launches_again() {
    let (servers, launcher, _) = servers(Behaviour::default());
    *launcher.refuse.lock().unwrap() = Some(McpError::Start("not found".into()));
    assert_eq!(
        servers.list_tools("fixture").await,
        Err(McpError::Start("not found".into()))
    );
    assert_eq!(launcher.launches(), 0);
    *launcher.refuse.lock().unwrap() = None;
    assert!(servers.list_tools("fixture").await.is_ok());
    assert_eq!(launcher.launches(), 1);
}

#[tokio::test]
async fn an_initialize_with_no_answer_times_out_and_the_next_use_starts_again() {
    let mut behaviour = Behaviour::default();
    behaviour.silent.insert("initialize");
    let (servers, launcher, clock) = servers(behaviour);
    let start = servers.list_tools("fixture");
    let timed_out = clock.passing(|wait| wait.limit() == INITIALIZE_TIMEOUT, start);
    assert_eq!(timed_out.await, Err(McpError::Timeout));
    // Every waiter on that start gets the same answer; the next use starts anew.
    launcher.behaviour.lock().unwrap().silent.clear();
    assert!(servers.list_tools("fixture").await.is_ok());
    assert_eq!(launcher.launches(), 2);
}

#[tokio::test]
async fn a_server_that_exits_fails_what_waits_and_the_next_use_is_a_new_generation() {
    let mut behaviour = Behaviour::default();
    behaviour.silent.insert("resources/read");
    let (servers, launcher, _) = servers(behaviour);
    servers.list_tools("fixture").await.unwrap();
    let uri = UiResourceUri::new("ui://fixture/chart.html").unwrap();
    let read = tokio::spawn({
        let servers = servers.clone();
        let uri = uri.clone();
        async move { servers.read_ui_resource("fixture", &uri).await }
    });
    launcher.generation(0).arrived("resources/read", 1).await;
    launcher.generation(0).exit();
    assert_eq!(read.await.unwrap(), Err(McpError::ServerGone));
    // The tools as last listed stay until the next generation lists them.
    let call =
        crate::domain::agent_execution::tools::McpTool::new("fixture", "show_chart").unwrap();
    assert!(servers.tool_ui(&call).is_some());
    servers.list_tools("fixture").await.unwrap();
    assert_eq!(launcher.launches(), 2);
}

#[tokio::test]
async fn a_server_that_exits_during_its_start_fails_that_start() {
    let mut behaviour = Behaviour::default();
    behaviour.silent.insert("initialize");
    let (servers, launcher, _) = servers(behaviour);
    let start = tokio::spawn({
        let servers = servers.clone();
        async move { servers.list_tools("fixture").await }
    });
    let server = loop {
        if launcher.launches() == 1 {
            break launcher.generation(0);
        }
        tokio::task::yield_now().await;
    };
    server.arrived("initialize", 1).await;
    server.exit();
    assert_eq!(start.await.unwrap(), Err(McpError::ServerGone));
}

#[tokio::test]
async fn a_failed_list_leaves_the_server_started() {
    let behaviour = Behaviour {
        pages: vec![vec![json!({ "name": "t" })]; 64],
        ..Behaviour::default()
    };
    let (servers, launcher, _) = servers(behaviour);
    // Its own list fails on the page bound; the server still serves.
    let stand_in = servers.stand_in("fixture").await.unwrap();
    let mut harness = Harness::attach(stand_in);
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .await;
    assert!(harness.next().await.is_some());
    assert_eq!(launcher.launches(), 1);
}

#[tokio::test]
async fn stopping_ends_every_stand_in_and_refuses_every_later_use() {
    let mut behaviour = Behaviour::default();
    behaviour.silent.insert("resources/read");
    let (servers, launcher, _) = servers(behaviour);
    let mut harness = Harness::attach(servers.stand_in("fixture").await.unwrap());
    let uri = UiResourceUri::new("ui://fixture/chart.html").unwrap();
    let read = tokio::spawn({
        let servers = servers.clone();
        let uri = uri.clone();
        async move { servers.read_ui_resource("fixture", &uri).await }
    });
    launcher.generation(0).arrived("resources/read", 1).await;
    servers.stop().await;
    assert_eq!(read.await.unwrap(), Err(McpError::Stopped));
    assert_eq!(harness.next().await, None);
    assert_eq!(servers.list_tools("fixture").await, Err(McpError::Stopped));
    assert!(matches!(
        servers.stand_in("fixture").await,
        Err(McpError::Stopped)
    ));
    assert_eq!(launcher.launches(), 1);
}

#[tokio::test]
async fn stopping_during_a_start_fails_that_start() {
    let mut behaviour = Behaviour::default();
    behaviour.silent.insert("initialize");
    let (servers, launcher, _) = servers(behaviour);
    let start = tokio::spawn({
        let servers = servers.clone();
        async move { servers.list_tools("fixture").await }
    });
    let server = loop {
        if launcher.launches() == 1 {
            break launcher.generation(0);
        }
        tokio::task::yield_now().await;
    };
    server.arrived("initialize", 1).await;
    servers.stop().await;
    assert_eq!(start.await.unwrap(), Err(McpError::Stopped));
}

#[tokio::test]
async fn start_all_starts_every_server_in_the_background() {
    let (servers, launcher, _) = servers(Behaviour::default());
    assert_eq!(servers.names().collect::<Vec<_>>(), ["fixture"]);
    servers.start_all();
    loop {
        if launcher.launches() == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    launcher.generation(0).arrived("tools/list", 1).await;
}

#[test]
fn an_invalid_or_repeated_configuration_is_refused() {
    let clock = Arc::new(ManualClock::default());
    let launcher = FixtureLauncher::new(Behaviour::default());
    let repeated = vec![launch("same"), launch("same")];
    assert!(matches!(
        McpServers::with_launcher(repeated, clock.clone(), launcher.clone()),
        Err(McpError::InvalidConfiguration)
    ));
    assert!(matches!(
        McpServers::with_launcher(vec![launch("a__b")], clock, launcher),
        Err(McpError::InvalidConfiguration)
    ));
}

/// The stateful fixture: a handle the agent's call got back resolves in the
/// app's later read on the same generation — one upstream session — and not
/// on the next generation, which is a new one.
#[tokio::test]
async fn a_handle_from_the_agents_call_resolves_in_the_apps_later_read() {
    let (servers, _, _) = servers(Behaviour::default());
    let mut agent = Harness::attach(servers.stand_in("fixture").await.unwrap());
    agent
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "remember" } }))
        .await;
    let handle = agent.next().await.unwrap()["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    let uri = UiResourceUri::new(format!("ui://fixture/handle/{handle}")).unwrap();
    let app = servers.read_ui_resource("fixture", &uri).await.unwrap();
    assert_eq!(app.html(), format!("<p>{handle}</p>"));
    // A handle never given out is unknown to it.
    let unknown = UiResourceUri::new("ui://fixture/handle/h99").unwrap();
    assert!(matches!(
        servers.read_ui_resource("fixture", &unknown).await,
        Err(McpError::Remote { code: -32002, .. })
    ));
}
