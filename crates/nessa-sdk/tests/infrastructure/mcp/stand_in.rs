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
            // A `_meta.ui` that is not an object is hidden from the model (#424).
            json!({ "name": "ui_string", "_meta": { "ui": "app" } }),
            json!({ "name": "ui_array", "_meta": { "ui": ["model", "app"] } }),
            json!({ "name": "ui_number", "_meta": { "ui": 1 } }),
            json!({ "name": "ui_boolean", "_meta": { "ui": false } }),
            json!({ "name": "ui_null", "_meta": { "ui": null } }),
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
    for (id, name) in [
        (2, "refresh"),
        (5, "ui_string"),
        (6, "ui_array"),
        (7, "ui_number"),
        (8, "ui_boolean"),
        (9, "ui_null"),
    ] {
        harness
            .send(json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name } }))
            .await;
        let refused = harness.next().await.unwrap();
        assert_eq!(
            (refused["id"].clone(), refused["error"]["code"].clone()),
            (json!(id), json!(-32602)),
            "{name}"
        );
    }
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

#[tokio::test]
async fn a_tool_the_sessions_list_hid_is_refused_before_the_harness_lists_any() {
    let behaviour = Behaviour {
        pages: vec![vec![
            // An app's own tool with no UI resource of its own.
            json!({ "name": "helper", "_meta": { "ui": { "visibility": ["app"] } } }),
            // A visibility that cannot be read: kept from the model.
            json!({ "name": "odd", "_meta": { "ui": { "visibility": "app" } } }),
            json!({ "name": "odder", "_meta": { "ui": { "visibility": ["app", 1] } } }),
            // Unreadable even though it names the model.
            json!({ "name": "oddest", "_meta": { "ui": { "visibility": ["model", 1] } } }),
            json!({ "name": "echo" }),
        ]],
        ..Behaviour::default()
    };
    let (session, _, launcher, _) = session(behaviour).await;
    session.list_tools().await.unwrap();
    let mut harness = Harness::attach(session);
    for (id, name) in [(1, "helper"), (2, "odd"), (3, "odder"), (5, "oddest")] {
        harness
            .send(json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name } }))
            .await;
        let refused = harness.next().await.unwrap();
        assert_eq!(
            (refused["id"].clone(), refused["error"]["code"].clone()),
            (json!(id), json!(-32602)),
            "{name}"
        );
    }
    assert!(launcher.server(0).with_method("tools/call").is_empty());
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "echo" } }))
        .await;
    assert!(harness.next().await.unwrap().get("result").is_some());
}

#[tokio::test]
async fn of_two_lists_answered_out_of_order_the_one_asked_later_decides() {
    let (session, _, launcher, _) = session(silent("tools/list")).await;
    let server = launcher.server(0);
    // The session's own list stays unanswered: it hides nothing.
    server.arrived("tools/list", 1).await;
    let mut harness = Harness::attach(session);
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
        .await;
    server.arrived("tools/list", 2).await;
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }))
        .await;
    server.arrived("tools/list", 3).await;
    let asked = server.with_method("tools/list");
    let answer = |id: &serde_json::Value, visibility: &str| {
        json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": [
            { "name": "refresh", "_meta": { "ui": { "visibility": [visibility] } } } ] } })
    };
    // Asked later, answered first: the model may see it.
    server.send(answer(&asked[2]["id"], "model"));
    assert_eq!(harness.next().await.unwrap()["id"], 2);
    // Asked earlier, answered later: it would hide it, and does not decide.
    server.send(answer(&asked[1]["id"], "app"));
    assert_eq!(harness.next().await.unwrap()["id"], 1);
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "refresh" } }))
        .await;
    // Forwarded: the fixture has no such tool, and says so.
    assert_eq!(
        harness.next().await.unwrap()["error"]["message"],
        "unknown tool"
    );
}

#[tokio::test]
async fn a_stand_in_that_falls_behind_the_change_notices_gets_all_three() {
    let (session, _, launcher, _) = session(Behaviour::default()).await;
    // Less room than one notice: the stand-in waits on the harness.
    let mut harness = Harness::attach_with_buffer(session, 16);
    // Serving, and so listening for notices, once it answers a ping.
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .await;
    assert_eq!(harness.next().await.unwrap()["id"], 1);
    let server = launcher.server(0);
    for _ in 0..40 {
        server.send(json!({ "jsonrpc": "2.0", "method": "notifications/resources/list_changed" }));
    }
    // Once the client has answered a request the server sent after them, its
    // reader has passed every notice on: they have outrun the stand-in.
    server.send(json!({ "jsonrpc": "2.0", "id": "barrier", "method": "ping" }));
    server
        .arrived_where(|message| message["id"] == "barrier" && message.get("method").is_none())
        .await;
    let mut seen = std::collections::BTreeSet::new();
    while seen.len() < 3 {
        let notice = harness.next().await.expect("the stand-in keeps serving");
        seen.insert(notice["method"].as_str().unwrap().to_owned());
    }
    assert!(seen.contains("notifications/tools/list_changed"));
    assert!(seen.contains("notifications/prompts/list_changed"));
}

#[test]
fn the_visibility_record_fails_closed_before_any_list_and_past_its_bound() {
    use crate::infrastructure::mcp::stand_in::{Visibility, MAX_REMEMBERED_TOOLS};
    let names = |prefix: &str, count: usize| -> Vec<(String, bool)> {
        (0..count)
            .map(|n| (format!("{prefix}{n}"), false))
            .collect()
    };
    // Before a list has said anything, nothing is known: hidden.
    let fresh = Visibility::default();
    assert!(fresh.hidden("echo"));
    // Within the bound, a name no list gave is the server's to decide.
    let within = Visibility::default();
    let order = within.ask();
    within.listed(
        order,
        [("helper".to_owned(), true), ("echo".to_owned(), false)],
    );
    assert!(within.hidden("helper") && !within.hidden("echo") && !within.hidden("unlisted"));
    // A name one list gives twice is hidden if either says so.
    let order = within.ask();
    within.listed(
        order,
        [("twice".to_owned(), true), ("twice".to_owned(), false)],
    );
    assert!(within.hidden("twice"));
    // Lists past the bound together: the latest list's names stand, the
    // earlier one's are forgotten, and anything not remembered is hidden.
    let past = Visibility::default();
    let earlier = past.ask();
    let later = past.ask();
    past.listed(earlier, names("a", MAX_REMEMBERED_TOOLS - 100));
    past.listed(later, names("b", 200));
    assert!(!past.hidden("b1"));
    assert!(past.hidden("a1"), "forgotten past the bound, so hidden");
    assert!(past.hidden("unlisted"));
    // The same lists, the earlier answered late: the later list's names
    // still stand, and the late one's are what is forgotten.
    let late = Visibility::default();
    let earlier = late.ask();
    let later = late.ask();
    late.listed(later, names("b", 200));
    late.listed(earlier, names("a", MAX_REMEMBERED_TOOLS - 100));
    assert!(!late.hidden("b1"), "a late answer undid a later list");
    assert!(late.hidden("a1") && late.hidden("unlisted"));
    // One list past the bound alone: all is forgotten.
    let flood = Visibility::default();
    let order = flood.ask();
    flood.listed(order, names("c", MAX_REMEMBERED_TOOLS + 1));
    assert!(flood.hidden("c1") && flood.hidden("unlisted"));
}

#[tokio::test]
async fn a_call_before_any_list_has_been_read_is_refused() {
    let (session, _, launcher, _) = session(silent("tools/list")).await;
    let mut harness = Harness::attach(session);
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "echo" } }))
        .await;
    let refused = harness.next().await.unwrap();
    assert_eq!(
        (refused["id"].clone(), refused["error"]["code"].clone()),
        (json!(1), json!(-32602))
    );
    assert!(launcher.server(0).with_method("tools/call").is_empty());
    // A list answered without a tools array says nothing: still refused.
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }))
        .await;
    let server = launcher.server(0);
    // The session's own list, asked at open, and the harness's: both so.
    server.arrived("tools/list", 2).await;
    for asked in server.with_method("tools/list") {
        server.send(json!({ "jsonrpc": "2.0", "id": asked["id"], "result": {} }));
    }
    assert_eq!(harness.next().await.unwrap()["id"], json!(2));
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "echo" } }))
        .await;
    assert_eq!(
        harness.next().await.unwrap()["error"]["code"],
        json!(-32602)
    );
    assert!(server.with_method("tools/call").is_empty());
}
