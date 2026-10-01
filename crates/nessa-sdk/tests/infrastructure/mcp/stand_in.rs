//! What a harness sees through a stand-in ("A stand-in" table, Serving rows).
use super::fixture::Behaviour;
use super::{servers, Harness};
use serde_json::json;

#[tokio::test]
async fn the_harness_initialize_is_answered_from_the_upstreams_and_not_forwarded() {
    let (servers, launcher, _) = servers(Behaviour::default());
    let mut harness = Harness::attach(servers.stand_in("fixture").await.unwrap());
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize",
                      "params": { "protocolVersion": "2025-06-18", "capabilities": { "roots": {} } } }))
        .await;
    let answer = harness.next().await.unwrap();
    assert_eq!(answer["id"], 0);
    assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(answer["result"]["serverInfo"]["name"], "fixture");
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
    let server = launcher.generation(0);
    assert_eq!(server.with_method("initialize").len(), 1);
    assert_eq!(server.with_method("notifications/initialized").len(), 1);
}

#[tokio::test]
async fn requests_are_forwarded_under_the_connections_ids_and_answered_under_the_harness_ids() {
    let (servers, launcher, _) = servers(Behaviour::default());
    let mut first = Harness::attach(servers.stand_in("fixture").await.unwrap());
    let mut second = Harness::attach(servers.stand_in("fixture").await.unwrap());
    // Both harnesses use id 1; one also uses a string id.
    first
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                      "params": { "name": "echo", "arguments": { "from": "first" } } }))
        .await;
    second
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                      "params": { "name": "echo", "arguments": { "from": "second" } } }))
        .await;
    second
        .send(json!({ "jsonrpc": "2.0", "id": "1", "method": "tools/call", "params": { "name": "nope" } }))
        .await;
    let answer = first.next().await.unwrap();
    assert_eq!(answer["id"], 1);
    assert_eq!(
        answer["result"]["structuredContent"],
        json!({ "from": "first" })
    );
    let mut answers = [second.next().await.unwrap(), second.next().await.unwrap()];
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
        json!({ "from": "second" })
    );
    // Upstream, three distinct ids of the connection's own, none a harness's.
    let ids: Vec<_> = launcher
        .generation(0)
        .with_method("tools/call")
        .iter()
        .map(|call| call["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids.len(), 3);
    assert!(ids.iter().all(|id| *id > 1));
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[1], ids[2]);
}

#[tokio::test]
async fn a_cancelled_request_is_cancelled_upstream_and_never_answered() {
    let mut behaviour = Behaviour::default();
    behaviour.silent.insert("tools/call");
    let (servers, launcher, _) = servers(behaviour);
    let mut harness = Harness::attach(servers.stand_in("fixture").await.unwrap());
    harness
        .send(json!({ "jsonrpc": "2.0", "id": "a", "method": "tools/call", "params": { "name": "echo" } }))
        .await;
    let server = launcher.generation(0);
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
    // Nothing is answered for it; the next request is.
    harness
        .send(json!({ "jsonrpc": "2.0", "id": "c", "method": "ping" }))
        .await;
    assert_eq!(harness.next().await.unwrap()["id"], "c");
}

#[tokio::test]
async fn a_harness_that_closes_cancels_what_it_was_waiting_for() {
    let mut behaviour = Behaviour::default();
    behaviour.silent.insert("tools/call");
    let (servers, launcher, _) = servers(behaviour);
    let mut harness = Harness::attach(servers.stand_in("fixture").await.unwrap());
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "echo" } }))
        .await;
    let server = launcher.generation(0);
    server.arrived("tools/call", 1).await;
    drop(harness);
    server.arrived("notifications/cancelled", 1).await;
    assert_eq!(
        server.with_method("notifications/cancelled")[0]["params"]["requestId"],
        server.with_method("tools/call")[0]["id"]
    );
}

#[tokio::test]
async fn a_duplicate_waiting_id_is_refused_without_forwarding() {
    let mut behaviour = Behaviour::default();
    behaviour.silent.insert("tools/call");
    let (servers, launcher, _) = servers(behaviour);
    let mut harness = Harness::attach(servers.stand_in("fixture").await.unwrap());
    let call =
        json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": "echo" } });
    harness.send(call.clone()).await;
    harness.send(call).await;
    let refused = harness.next().await.unwrap();
    assert_eq!(
        (refused["id"].clone(), refused["error"]["code"].clone()),
        (json!(7), json!(-32600))
    );
    assert_eq!(launcher.generation(0).with_method("tools/call").len(), 1);
}

#[tokio::test]
async fn the_servers_change_notices_reach_every_stand_in() {
    let (servers, launcher, _) = servers(Behaviour::default());
    let mut first = Harness::attach(servers.stand_in("fixture").await.unwrap());
    let mut second = Harness::attach(servers.stand_in("fixture").await.unwrap());
    // Both are serving once each answers a ping.
    for harness in [&mut first, &mut second] {
        harness
            .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
            .await;
        harness.next().await.unwrap();
    }
    let changed = json!({ "jsonrpc": "2.0", "method": "notifications/resources/list_changed" });
    launcher.generation(0).send(changed.clone());
    // Progress and logging are not passed on; only the change notice arrives.
    launcher
        .generation(0)
        .send(json!({ "jsonrpc": "2.0", "method": "notifications/message", "params": {} }));
    assert_eq!(first.next().await.unwrap(), changed);
    assert_eq!(second.next().await.unwrap(), changed);
}

#[tokio::test]
async fn a_stand_in_ends_with_its_generation() {
    let (servers, launcher, _) = servers(Behaviour::default());
    let mut harness = Harness::attach(servers.stand_in("fixture").await.unwrap());
    launcher.generation(0).exit();
    assert_eq!(harness.next().await, None);
    // A new stand-in is a new generation.
    let mut next = Harness::attach(servers.stand_in("fixture").await.unwrap());
    next.send(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .await;
    assert_eq!(next.next().await.unwrap()["result"], json!({}));
    assert_eq!(launcher.launches(), 2);
}

#[tokio::test]
async fn a_frame_from_the_harness_that_is_not_json_ends_the_stand_in_only() {
    let (servers, launcher, _) = servers(Behaviour::default());
    let mut harness = Harness::attach(servers.stand_in("fixture").await.unwrap());
    harness.send_raw(b"not json\n").await;
    assert_eq!(harness.next().await, None);
    let mut other = Harness::attach(servers.stand_in("fixture").await.unwrap());
    other
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .await;
    assert!(other.next().await.is_some());
    assert_eq!(launcher.launches(), 1);
}

#[tokio::test]
async fn an_answer_too_large_for_a_frame_is_an_error_for_its_id() {
    use crate::infrastructure::mcp::framing::MAX_FRAME_BYTES;
    let (servers, _, _) = servers(Behaviour::default());
    let mut harness = Harness::attach(servers.stand_in("fixture").await.unwrap());
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
