//! An MCP App's calls on its conversation's own session (#348): the tool as
//! that session listed it, with its hints; a call and a resource read over
//! that session's connection, and no other's; and their failures.
use super::fixture::{Behaviour, CHART};
use super::servers;
use crate::domain::agent_execution::sessions::SessionId;
use crate::domain::mcp_apps::UiResourceUri;
use crate::infrastructure::mcp::{McpError, McpOwner};
use serde_json::json;
use std::time::Duration;

/// The budgets these tests give a call and a read (the gateway gives the
/// protocol's). Neither is the SDK's own `REQUEST_TIMEOUT`, so a wait on that
/// instead of the caller's budget is told apart.
const CALL: Duration = Duration::from_secs(60);
const READ: Duration = Duration::from_secs(7);

fn conversation(name: &str) -> SessionId {
    SessionId::new(name).unwrap()
}

#[tokio::test]
async fn an_apps_call_reaches_its_conversations_own_session_and_no_other() {
    let (servers, launcher, _) = servers(Behaviour::default());
    let mine = servers
        .open("fixture", McpOwner::new(conversation("a")))
        .await
        .unwrap();
    let theirs = servers
        .open("fixture", McpOwner::new(conversation("b")))
        .await
        .unwrap();
    mine.list_tools().await.unwrap();
    theirs.list_tools().await.unwrap();
    let result = servers
        .call_tool(
            &conversation("a"),
            "fixture",
            "echo",
            Some(json!({ "x": 1 })),
            CALL,
        )
        .await
        .unwrap();
    assert_eq!(result["structuredContent"], json!({ "x": 1 }));
    assert_eq!(launcher.server(0).with_method("tools/call").len(), 1);
    assert!(launcher.server(1).with_method("tools/call").is_empty());
    // A call with no arguments sends none.
    servers
        .call_tool(&conversation("a"), "fixture", "echo", None, CALL)
        .await
        .unwrap();
    let sent = launcher.server(0).with_method("tools/call");
    assert!(sent[1]["params"].get("arguments").is_none());
}

#[tokio::test]
async fn without_a_session_of_its_own_an_app_reaches_nothing() {
    let (servers, launcher, _) = servers(Behaviour::default());
    let other = servers
        .open("fixture", McpOwner::new(conversation("b")))
        .await
        .unwrap();
    other.list_tools().await.unwrap();
    let none = conversation("a");
    assert_eq!(
        servers.listed_tool(&none, "fixture", "echo"),
        Err(McpError::NoSession)
    );
    assert_eq!(
        servers
            .call_tool(&none, "fixture", "echo", None, CALL)
            .await,
        Err(McpError::NoSession)
    );
    assert_eq!(
        servers
            .read_app_resource(&none, "fixture", &UiResourceUri::new(CHART).unwrap(), READ)
            .await,
        Err(McpError::NoSession)
    );
    assert_eq!(
        servers.listed_tool(&conversation("b"), "other", "echo"),
        Err(McpError::NoSession)
    );
    assert!(launcher.server(0).with_method("tools/call").is_empty());
}

#[tokio::test]
async fn a_listed_tool_is_found_by_its_exact_name_with_what_it_said_of_its_effects() {
    let (servers, _, _) = servers(Behaviour {
        pages: vec![vec![
            json!({ "name": "reads", "annotations": { "readOnlyHint": true } }),
            json!({ "name": "keeps", "annotations": { "destructiveHint": false } }),
            json!({ "name": "silent" }),
            // Not booleans: as if unsaid.
            json!({ "name": "odd", "annotations": { "readOnlyHint": "yes", "destructiveHint": 0 } }),
        ]],
        ..Behaviour::default()
    });
    let session = servers
        .open("fixture", McpOwner::new(conversation("a")))
        .await
        .unwrap();
    session.list_tools().await.unwrap();
    let a = conversation("a");
    let destructive = |name: &str| {
        servers
            .listed_tool(&a, "fixture", name)
            .unwrap()
            .map(|tool| tool.hints().destructive())
    };
    assert_eq!(destructive("reads"), Some(false));
    assert_eq!(destructive("keeps"), Some(false));
    assert_eq!(destructive("silent"), Some(true));
    assert_eq!(destructive("odd"), Some(true));
    // Exactly as listed: no other spelling, nothing unlisted.
    assert_eq!(destructive("Reads"), None);
    assert_eq!(destructive("missing"), None);
}

#[tokio::test]
async fn a_calls_failures_are_the_servers_error_or_its_silence() {
    let mut behaviour = Behaviour::default();
    behaviour.silent.insert("tools/call");
    let (servers, launcher, clock) = servers(behaviour);
    let session = servers
        .open("fixture", McpOwner::new(conversation("a")))
        .await
        .unwrap();
    session.list_tools().await.unwrap();
    // No answer within the budget: timed out, and cancelled upstream.
    let a = conversation("a");
    let call = servers.call_tool(&a, "fixture", "echo", None, CALL);
    let timed_out = clock.passing(|wait| wait.limit() == CALL, call);
    assert_eq!(timed_out.await, Err(McpError::Timeout));
    launcher
        .server(0)
        .arrived("notifications/cancelled", 1)
        .await;
}

#[tokio::test]
async fn a_read_unanswered_within_the_callers_budget_times_out() {
    let mut behaviour = Behaviour::default();
    behaviour.silent.insert("resources/read");
    let (servers, _, clock) = servers(behaviour);
    let session = servers
        .open("fixture", McpOwner::new(conversation("a")))
        .await
        .unwrap();
    session.list_tools().await.unwrap();
    let a = conversation("a");
    let uri = UiResourceUri::new(CHART).unwrap();
    let read = servers.read_app_resource(&a, "fixture", &uri, READ);
    // Only a wait of the caller's budget is let pass; a read waiting on any
    // other never ends, which the guard reports.
    let timed_out = clock.passing(|wait| wait.limit() == READ, read);
    let ended = tokio::time::timeout(Duration::from_secs(5), timed_out)
        .await
        .expect("the read waits the caller's budget");
    assert_eq!(ended.map(|_| ()), Err(McpError::Timeout));
}

#[tokio::test]
async fn a_servers_refusal_and_an_answer_that_is_no_result_are_typed() {
    let (servers, _, _) = servers(Behaviour::default());
    let session = servers
        .open("fixture", McpOwner::new(conversation("a")))
        .await
        .unwrap();
    session.list_tools().await.unwrap();
    assert!(matches!(
        servers
            .call_tool(&conversation("a"), "fixture", "no_such_tool", None, CALL)
            .await,
        Err(McpError::Remote { code: -32602, .. })
    ));
    // A result that is not an object is no CallToolResult.
    let mut behaviour = Behaviour::default();
    behaviour.silent.insert("tools/call");
    let (servers, launcher, _) = super::servers(behaviour);
    let session = servers
        .open("fixture", McpOwner::new(conversation("a")))
        .await
        .unwrap();
    session.list_tools().await.unwrap();
    let a = conversation("a");
    let call = tokio::spawn({
        let servers = servers.clone();
        async move { servers.call_tool(&a, "fixture", "echo", None, CALL).await }
    });
    let server = launcher.server(0);
    server.arrived("tools/call", 1).await;
    let id = server.with_method("tools/call")[0]["id"].clone();
    server.send(json!({ "jsonrpc": "2.0", "id": id, "result": 5 }));
    assert!(matches!(call.await.unwrap(), Err(McpError::Malformed(_))));
}

#[tokio::test]
async fn an_apps_resource_is_read_over_its_conversations_own_session() {
    let (servers, launcher, _) = servers(Behaviour::default());
    let session = servers
        .open("fixture", McpOwner::new(conversation("a")))
        .await
        .unwrap();
    session.list_tools().await.unwrap();
    let resource = servers
        .read_app_resource(
            &conversation("a"),
            "fixture",
            &UiResourceUri::new(CHART).unwrap(),
            READ,
        )
        .await
        .unwrap();
    assert_eq!(resource.uri().as_str(), CHART);
    assert_eq!(launcher.server(0).with_method("resources/read").len(), 1);
}
