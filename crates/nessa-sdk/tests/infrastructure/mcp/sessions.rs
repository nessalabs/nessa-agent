//! Sessions ("A session" table): opening, closing, stopping, sessions apart
//! from each other, and the view's lookup across them.
use super::fixture::{launch, Behaviour, FixtureLauncher};
use super::{servers, session, Harness};
use crate::domain::agent_execution::tools::McpTool;
use crate::domain::mcp_apps::UiResourceUri;
use crate::infrastructure::clock::manual::ManualClock;
use crate::infrastructure::mcp::{McpError, McpServers, INITIALIZE_TIMEOUT};
use serde_json::json;
use std::{sync::Arc, time::Duration};

fn silent(method: &'static str) -> Behaviour {
    let mut behaviour = Behaviour::default();
    behaviour.silent.insert(method);
    behaviour
}

async fn eventually(what: &str, done: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !done() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}"));
}

#[tokio::test]
async fn an_initialize_with_no_answer_times_out_and_the_process_is_stopped() {
    let (servers, launcher, clock) = servers(silent("initialize"));
    let open = servers.open("fixture");
    let timed_out = clock.passing(|wait| wait.limit() == INITIALIZE_TIMEOUT, open);
    assert!(matches!(timed_out.await, Err(McpError::Timeout)));
    launcher.server(0).stopped().await;
}

#[tokio::test]
async fn a_server_that_cannot_launch_or_exits_while_opening_fails_the_opening() {
    let (servers, launcher, _) = servers(Behaviour::default());
    *launcher.refuse.lock().unwrap() = Some(McpError::Start("not found".into()));
    assert!(matches!(
        servers.open("fixture").await,
        Err(McpError::Start(reason)) if reason == "not found"
    ));
    *launcher.refuse.lock().unwrap() = None;
    let (servers, launcher, _) = super::servers(silent("initialize"));
    let open = tokio::spawn(async move { servers.open("fixture").await.map(|_| ()) });
    eventually("launched", || launcher.launches() == 1).await;
    launcher.server(0).arrived("initialize", 1).await;
    launcher.server(0).exit();
    assert_eq!(open.await.unwrap(), Err(McpError::ServerGone));
}

#[tokio::test]
async fn something_on_stdout_that_is_not_an_answer_is_a_handshake_failure() {
    let (servers, launcher, _) = servers(silent("initialize"));
    let open = tokio::spawn(async move { servers.open("fixture").await.map(|_| ()) });
    eventually("launched", || launcher.launches() == 1).await;
    launcher.server(0).arrived("initialize", 1).await;
    launcher
        .server(0)
        .send_raw(b"Starting server v1.2...\n".to_vec());
    assert!(matches!(open.await.unwrap(), Err(McpError::Handshake(_))));
}

#[tokio::test]
async fn each_opening_is_a_server_process_and_a_session_of_its_own() {
    let (servers, launcher, _) = servers(Behaviour::default());
    let first = servers.open("fixture").await.unwrap();
    let second = servers.open("fixture").await.unwrap();
    assert_eq!(launcher.launches(), 2);
    assert_eq!((first.server(), second.server()), ("fixture", "fixture"));
    // Each initialized its own server, once.
    for index in 0..2 {
        assert_eq!(launcher.server(index).with_method("initialize").len(), 1);
    }
    // One ending leaves the other.
    launcher.server(0).exit();
    assert!(first.list_tools().await.is_err());
    assert_eq!(second.list_tools().await.unwrap().len(), 2);
}

/// The stateful fixture: a handle the agent's call got back resolves in a
/// later read on the same session — the agent and its app share one upstream
/// session — and not in another conversation's.
#[tokio::test]
async fn a_handle_from_the_agents_call_resolves_in_the_same_sessions_later_read_only() {
    let (servers, _, _) = servers(Behaviour::default());
    let session = servers.open("fixture").await.unwrap();
    let other = servers.open("fixture").await.unwrap();
    let mut agent = Harness::attach(session.clone());
    agent
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "remember" } }))
        .await;
    let handle = agent.next().await.unwrap()["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    let uri = UiResourceUri::new(format!("ui://fixture/handle/{handle}")).unwrap();
    let app = session.read_ui_resource(&uri).await.unwrap();
    assert_eq!(app.html(), format!("<p>{handle}</p>"));
    assert!(matches!(
        other.read_ui_resource(&uri).await,
        Err(McpError::Remote { code: -32002, .. })
    ));
}

#[tokio::test]
async fn a_session_ends_with_its_stand_in_and_closes_its_server() {
    let (session, _, launcher, _) = session(silent("resources/read")).await;
    let waiting = tokio::spawn({
        let session = session.clone();
        async move {
            let uri = UiResourceUri::new("ui://fixture/chart.html").unwrap();
            session.read_ui_resource(&uri).await
        }
    });
    launcher.server(0).arrived("resources/read", 1).await;
    let harness = Harness::attach(session.clone());
    drop(harness);
    // The server's stdin closes, and what waited ends `Closed`.
    launcher.server(0).stopped().await;
    assert_eq!(waiting.await.unwrap(), Err(McpError::Closed));
    assert_eq!(session.list_tools().await, Err(McpError::Closed));
}

#[tokio::test]
async fn closing_a_session_ends_what_waits_on_it() {
    let (session, _, launcher, _) = session(silent("resources/read")).await;
    let waiting = tokio::spawn({
        let session = session.clone();
        async move {
            let uri = UiResourceUri::new("ui://fixture/chart.html").unwrap();
            session.read_ui_resource(&uri).await
        }
    });
    launcher.server(0).arrived("resources/read", 1).await;
    session.close().await;
    assert_eq!(waiting.await.unwrap(), Err(McpError::Closed));
    launcher.server(0).stopped().await;
}

#[tokio::test]
async fn stopping_closes_every_session_and_refuses_later_ones() {
    let (servers, launcher, _) = servers(silent("resources/read"));
    let first = servers.open("fixture").await.unwrap();
    let second = servers.open("fixture").await.unwrap();
    let mut harness = Harness::attach(second);
    let waiting = tokio::spawn({
        let first = first.clone();
        async move {
            let uri = UiResourceUri::new("ui://fixture/chart.html").unwrap();
            first.read_ui_resource(&uri).await
        }
    });
    launcher.server(0).arrived("resources/read", 1).await;
    servers.stop().await;
    assert_eq!(waiting.await.unwrap(), Err(McpError::Stopped));
    assert_eq!(harness.next().await, None);
    launcher.server(0).stopped().await;
    launcher.server(1).stopped().await;
    assert!(matches!(
        servers.open("fixture").await,
        Err(McpError::Stopped)
    ));
    assert_eq!(launcher.launches(), 2);
}

#[tokio::test]
async fn stopping_while_a_session_opens_fails_that_opening() {
    let (servers, launcher, _) = servers(silent("initialize"));
    let open = tokio::spawn({
        let servers = servers.clone();
        async move { servers.open("fixture").await.map(|_| ()) }
    });
    eventually("launched", || launcher.launches() == 1).await;
    launcher.server(0).arrived("initialize", 1).await;
    servers.stop().await;
    assert_eq!(open.await.unwrap(), Err(McpError::Stopped));
    launcher.server(0).stopped().await;
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
        McpServers::with_launcher(vec![launch("a__b")], clock.clone(), launcher.clone()),
        Err(McpError::InvalidConfiguration)
    ));
    let servers = McpServers::with_launcher(vec![launch("one")], clock, launcher).unwrap();
    assert_eq!(servers.names().collect::<Vec<_>>(), ["one"]);
}

fn chart_call() -> McpTool {
    McpTool::new("fixture", "show_chart").unwrap()
}

#[tokio::test]
async fn the_view_finds_a_tools_ui_once_a_session_has_listed_it() {
    let (servers, launcher, _) = servers(silent("tools/list"));
    let session = servers.open("fixture").await.unwrap();
    // Not listed yet: no UI.
    launcher.server(0).arrived("tools/list", 1).await;
    assert_eq!(servers.tool_ui(&chart_call()), None);
    // Listed: the UI, for the call that names it; none for one that does not.
    let asked = launcher.server(0).with_method("tools/list")[0]["id"].clone();
    launcher.server(0).send(
        json!({ "jsonrpc": "2.0", "id": asked, "result": { "tools": [
        { "name": "show_chart", "_meta": { "ui": { "resourceUri": "ui://fixture/chart.html" } } },
        { "name": "report" } ] } }),
    );
    eventually("listed", || servers.tool_ui(&chart_call()).is_some()).await;
    assert_eq!(
        servers
            .tool_ui(&chart_call())
            .unwrap()
            .resource_uri()
            .as_str(),
        "ui://fixture/chart.html"
    );
    assert_eq!(
        servers.tool_ui(&McpTool::new("fixture", "report").unwrap()),
        None
    );
    assert_eq!(
        servers.tool_ui(&McpTool::new("other", "show_chart").unwrap()),
        None
    );
    // A closed session's list no longer counts.
    session.close().await;
    assert_eq!(servers.tool_ui(&chart_call()), None);
}

#[tokio::test]
async fn sessions_that_disagree_about_a_tools_ui_show_none() {
    let (servers, launcher, _) = servers(Behaviour::default());
    let first = servers.open("fixture").await.unwrap();
    let second = servers.open("fixture").await.unwrap();
    first.list_tools().await.unwrap();
    second.list_tools().await.unwrap();
    // Agreeing, they show it.
    assert!(servers.tool_ui(&chart_call()).is_some());
    // The second now declares another UI for the same tool.
    launcher
        .server(1)
        .set_pages(vec![vec![json!({ "name": "show_chart",
        "_meta": { "ui": { "resourceUri": "ui://fixture/other.html" } } })]]);
    second.list_tools().await.unwrap();
    assert_eq!(servers.tool_ui(&chart_call()), None);
    // Or none at all: still a disagreement.
    launcher
        .server(1)
        .set_pages(vec![vec![json!({ "name": "show_chart" })]]);
    second.list_tools().await.unwrap();
    assert_eq!(servers.tool_ui(&chart_call()), None);
    // Once the disagreeing session closes, the other's stands.
    second.close().await;
    assert!(servers.tool_ui(&chart_call()).is_some());
}

#[tokio::test]
async fn of_two_lists_finishing_out_of_order_the_one_asked_later_is_kept() {
    let (session, servers, launcher, _) = session(silent("tools/list")).await;
    let server = launcher.server(0);
    // The background list, asked first, stays unanswered.
    server.arrived("tools/list", 1).await;
    let earlier = tokio::spawn({
        let session = session.clone();
        async move { session.list_tools().await }
    });
    server.arrived("tools/list", 2).await;
    let later = tokio::spawn({
        let session = session.clone();
        async move { session.list_tools().await }
    });
    server.arrived("tools/list", 3).await;
    let ids: Vec<_> = server
        .with_method("tools/list")
        .iter()
        .map(|asked| asked["id"].clone())
        .collect();
    let answer = |id: &serde_json::Value, uri: &str| {
        json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": [
            { "name": "show_chart", "_meta": { "ui": { "resourceUri": uri } } } ] } })
    };
    server.send(answer(&ids[2], "ui://fixture/later.html"));
    later.await.unwrap().unwrap();
    server.send(answer(&ids[1], "ui://fixture/earlier.html"));
    earlier.await.unwrap().unwrap();
    assert_eq!(
        servers
            .tool_ui(&chart_call())
            .unwrap()
            .resource_uri()
            .as_str(),
        "ui://fixture/later.html"
    );
}
