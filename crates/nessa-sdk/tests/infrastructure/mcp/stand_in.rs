//! What a harness sees through a stand-in ("A stand-in" table, Serving rows).
use super::fixture::Behaviour;
use super::{session, Harness};
use serde_json::json;

fn silent(method: &'static str) -> Behaviour {
    let mut behaviour = Behaviour::default();
    behaviour.silent.insert(method);
    behaviour
}

#[tokio::test]
async fn the_harness_initialize_is_answered_from_the_upstreams_and_not_forwarded() {
    let (session, _, launcher, _) = session(Behaviour::default()).await;
    let mut harness = Harness::attach(session);
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize",
                      "params": { "protocolVersion": "2025-06-18", "capabilities": { "roots": {} } } }))
        .await;
    let answer = harness.next().await.unwrap();
    assert_eq!(answer["id"], 0);
    assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(answer["result"]["serverInfo"]["name"], "fixture");
    // Subscriptions are not offered; the rest of the capability stays.
    assert_eq!(
        answer["result"]["capabilities"]["resources"],
        json!({ "listChanged": true })
    );
    harness
        .send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .await;
    assert_eq!(
        harness.next().await.unwrap(),
        json!({ "jsonrpc": "2.0", "id": 1, "result": {} })
    );
    // The upstream was initialized once, by the client, and told so once.
    let server = launcher.server(0);
    assert_eq!(server.with_method("initialize").len(), 1);
    assert_eq!(server.with_method("notifications/initialized").len(), 1);
}

#[tokio::test]
async fn requests_are_forwarded_under_the_connections_ids_and_answered_under_the_harness_ids() {
    let (session, _, launcher, _) = session(Behaviour::default()).await;
    let mut harness = Harness::attach(session);
    // A number and a string spelling the same digit are two ids.
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                      "params": { "name": "echo", "arguments": { "from": "number" } } }))
        .await;
    harness
        .send(json!({ "jsonrpc": "2.0", "id": "1", "method": "tools/call", "params": { "name": "nope" } }))
        .await;
    let mut answers = [harness.next().await.unwrap(), harness.next().await.unwrap()];
    answers.sort_by_key(|answer| answer["id"].to_string());
    assert_eq!(answers[0]["id"], "1");
    // The server's error, verbatim.
    assert_eq!(
        answers[0]["error"],
        json!({ "code": -32602, "message": "unknown tool" })
    );
    assert_eq!(answers[1]["id"], 1);
    assert_eq!(
        answers[1]["result"]["structuredContent"],
        json!({ "from": "number" })
    );
    // Upstream, two distinct ids of the connection's own, neither a harness's.
    let ids: Vec<_> = launcher
        .server(0)
        .with_method("tools/call")
        .iter()
        .map(|call| call["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.iter().all(|id| *id > 1));
    assert_ne!(ids[0], ids[1]);
}

#[tokio::test]
async fn a_cancelled_request_is_cancelled_upstream_and_never_answered() {
    let (session, _, launcher, _) = session(silent("tools/call")).await;
    let mut harness = Harness::attach(session);
    harness
        .send(json!({ "jsonrpc": "2.0", "id": "a", "method": "tools/call", "params": { "name": "echo" } }))
        .await;
    let server = launcher.server(0);
    server.arrived("tools/call", 1).await;
    let upstream = server.with_method("tools/call")[0]["id"].clone();
    // A cancellation naming another id cancels nothing.
    harness
        .send(json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": "b" } }))
        .await;
    harness
        .send(json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": "a" } }))
        .await;
    server.arrived("notifications/cancelled", 1).await;
    let cancelled = server.with_method("notifications/cancelled");
    assert_eq!(cancelled.len(), 1);
    assert_eq!(cancelled[0]["params"]["requestId"], upstream);
    // A late answer to it reaches nobody; the id is free again.
    server.send(json!({ "jsonrpc": "2.0", "id": upstream, "result": { "content": [] } }));
    harness
        .send(json!({ "jsonrpc": "2.0", "id": "a", "method": "ping" }))
        .await;
    assert_eq!(
        harness.next().await.unwrap(),
        json!({ "jsonrpc": "2.0", "id": "a", "result": {} })
    );
}

#[test]
fn an_answer_belongs_to_the_call_now_waiting_under_its_id() {
    use crate::infrastructure::mcp::stand_in::answered_by;
    use std::collections::HashMap;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut tasks = tokio::task::JoinSet::new();
        let old = tasks.spawn(std::future::pending::<()>()).id();
        let new = tasks.spawn(std::future::pending::<()>());
        let new_id = new.id();
        let mut waiting = HashMap::new();
        waiting.insert("1".to_owned(), new);
        // The call reusing the id is answered; the old one's answer is not.
        assert!(answered_by(&waiting, "1", new_id));
        assert!(!answered_by(&waiting, "1", old));
        // Nor one cancelled since.
        assert!(!answered_by(&waiting, "2", new_id));
        tasks.abort_all();
    });
}

#[tokio::test]
async fn a_harness_that_closes_mid_call_closes_its_server() {
    let (session, _, launcher, _) = session(silent("tools/call")).await;
    let mut harness = Harness::attach(session);
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "echo" } }))
        .await;
    let server = launcher.server(0);
    server.arrived("tools/call", 1).await;
    // The session closes: the server's stdin closes with the call unanswered,
    // which ends it — there is nobody left to answer.
    drop(harness);
    server.stopped().await;
}

#[tokio::test]
async fn a_duplicate_waiting_id_is_refused_without_forwarding() {
    let (session, _, launcher, _) = session(silent("tools/call")).await;
    let mut harness = Harness::attach(session);
    let call =
        json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": "echo" } });
    harness.send(call.clone()).await;
    harness.send(call).await;
    let refused = harness.next().await.unwrap();
    assert_eq!(
        (refused["id"].clone(), refused["error"]["code"].clone()),
        (json!(7), json!(-32600))
    );
    assert_eq!(launcher.server(0).with_method("tools/call").len(), 1);
}

#[tokio::test]
async fn tools_the_model_may_not_see_are_left_out_of_its_lists_and_refused_if_called() {
    let behaviour = Behaviour {
        pages: vec![vec![
            json!({ "name": "show_chart", "_meta": { "ui": { "resourceUri": "ui://f/c", "visibility": ["model", "app"] } } }),
            json!({ "name": "refresh", "_meta": { "ui": { "resourceUri": "ui://f/c", "visibility": ["app"] } } }),
            json!({ "name": "echo" }),
        ]],
        ..Behaviour::default()
    };
    let (session, _, launcher, _) = session(behaviour).await;
    let server = launcher.server(0);
    let mut harness = Harness::attach(session);
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
        .await;
    let listed = harness.next().await.unwrap();
    let names: Vec<_> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].clone())
        .collect();
    assert_eq!(names, [json!("show_chart"), json!("echo")]);
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "refresh" } }))
        .await;
    let refused = harness.next().await.unwrap();
    assert_eq!(
        (refused["id"].clone(), refused["error"]["code"].clone()),
        (json!(2), json!(-32602))
    );
    assert_eq!(server.with_method("tools/call").len(), 0);
    // Listed again as the model's, it may be called again.
    server.set_pages(vec![vec![json!({ "name": "refresh" })]]);
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }))
        .await;
    harness.next().await.unwrap();
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "refresh" } }))
        .await;
    // Forwarded: the fixture has no such tool, and says so.
    assert_eq!(
        harness.next().await.unwrap()["error"]["message"],
        "unknown tool"
    );
}

#[tokio::test]
async fn resource_subscriptions_are_refused_and_not_forwarded() {
    let (session, _, launcher, _) = session(Behaviour::default()).await;
    let mut harness = Harness::attach(session);
    for (id, method) in [(1, "resources/subscribe"), (2, "resources/unsubscribe")] {
        harness
            .send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": { "uri": "x" } }))
            .await;
        let refused = harness.next().await.unwrap();
        assert_eq!(
            (refused["id"].clone(), refused["error"]["code"].clone()),
            (json!(id), json!(-32601))
        );
    }
    let server = launcher.server(0);
    assert!(server.with_method("resources/subscribe").is_empty());
    assert!(server.with_method("resources/unsubscribe").is_empty());
}

#[tokio::test]
async fn the_servers_change_notices_reach_its_stand_in() {
    let (session, _, launcher, _) = session(Behaviour::default()).await;
    let mut harness = Harness::attach(session);
    // Serving once it answers a ping.
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .await;
    harness.next().await.unwrap();
    let changed = json!({ "jsonrpc": "2.0", "method": "notifications/resources/list_changed" });
    // Progress and logging are not passed on; only the change notice arrives.
    launcher
        .server(0)
        .send(json!({ "jsonrpc": "2.0", "method": "notifications/message", "params": {} }));
    launcher.server(0).send(changed.clone());
    assert_eq!(harness.next().await.unwrap(), changed);
}

#[tokio::test]
async fn a_stand_in_ends_with_its_session() {
    let (session, _, launcher, _) = session(Behaviour::default()).await;
    let mut harness = Harness::attach(session);
    launcher.server(0).exit();
    assert_eq!(harness.next().await, None);
}

#[tokio::test]
async fn a_frame_from_the_harness_that_is_not_json_ends_the_stand_in_and_its_session() {
    let (session, _, launcher, _) = session(Behaviour::default()).await;
    let mut harness = Harness::attach(session);
    harness.send_raw(b"not json\n").await;
    assert_eq!(harness.next().await, None);
    launcher.server(0).stopped().await;
}

#[tokio::test]
async fn an_answer_too_large_for_a_frame_is_an_error_for_its_id() {
    use crate::infrastructure::mcp::framing::MAX_FRAME_BYTES;
    let (session, _, _, _) = session(Behaviour::default()).await;
    let mut harness = Harness::attach(session);
    // The upstream's answer fits a frame under its short id; under the
    // harness's long one it would not.
    let id = "i".repeat(400);
    harness
        .send(json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
                      "params": { "name": "big", "arguments": { "bytes": MAX_FRAME_BYTES - 200 } } }))
        .await;
    let answer = harness.next().await.unwrap();
    assert_eq!(answer["id"], json!(id));
    assert_eq!(answer["error"]["code"], -32603);
}
