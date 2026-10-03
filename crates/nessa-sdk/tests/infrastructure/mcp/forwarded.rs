//! What a grant keeps of the results its stand-ins forward: the "Stand-in,
//! recording" rows of #435's table (S1–S11), each test named after its row.
use super::super::{
    framing::MAX_FRAME_BYTES, ForwardedResults, McpOwner, McpSession, MAX_FORWARDED_RESULTS,
};
use super::fixture::{Behaviour, FixtureLauncher};
use super::{conversation, servers, Harness};
use crate::domain::agent_execution::tools::{ToolContent, MAX_STRUCTURED_RESULT_BYTES};
use crate::infrastructure::mcp::STRUCTURED_RESULT_OMITTED;
use serde_json::{json, Value};
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, DuplexStream, WriteHalf};

/// A listed session on `fixture` with `behaviour`, under a grant the test
/// holds, so it can read what the grant kept.
async fn granted(behaviour: Behaviour) -> (McpSession, McpOwner, Arc<FixtureLauncher>) {
    let (servers, launcher, _) = servers(behaviour);
    let owner = McpOwner::new(conversation());
    let session = servers.open("fixture", owner.clone()).await.unwrap();
    session.list_tools().await.unwrap();
    (session, owner, launcher)
}

fn silent(method: &'static str) -> Behaviour {
    let mut behaviour = Behaviour::default();
    behaviour.silent.insert(method);
    behaviour
}

/// A `tools/call` of `tool` with `arguments`, under the harness's `id`, naming
/// the call `call` as Claude's harness does.
fn call(id: Value, tool: &str, arguments: Value, call: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": tool, "arguments": arguments,
                        "_meta": { "claudecode/toolUseId": call, "progressToken": 3 } } })
}

fn structured(json: &str) -> ToolContent {
    ToolContent::structured(json).unwrap()
}

/// A stand-in's output whose flush never finishes: the bytes it writes reach
/// the harness, but the stand-in stays inside that write. What it does after
/// the write can then not have happened yet when the harness has the answer.
struct HeldFlush(WriteHalf<DuplexStream>);
impl AsyncWrite for HeldFlush {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(context, bytes)
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Pending
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(context)
    }
}

#[tokio::test]
async fn s1_a_structured_result_is_kept_under_the_harness_call_id_before_the_harness_is_answered() {
    let (session, owner, _) = granted(Behaviour::default()).await;
    let (harness, served) = tokio::io::duplex(64 * 1024);
    let (input, output) = tokio::io::split(served);
    tokio::spawn(session.serve(input, HeldFlush(output)));
    let (read, mut write) = tokio::io::split(harness);
    let mut lines = BufReader::new(read).lines();
    let mut request = serde_json::to_vec(&call(
        json!(1),
        "echo",
        json!({ "rows": [1, 2] }),
        json!("toolu_1"),
    ))
    .unwrap();
    request.push(b'\n');
    write.write_all(&request).await.unwrap();
    let answer: Value = serde_json::from_str(
        &tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .expect("answered within 5 s")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    // The harness's answer is the server's, verbatim — and the stand-in, still
    // in the write that gave it, kept the result before it.
    assert_eq!(
        answer["result"]["structuredContent"],
        json!({ "rows": [1, 2] })
    );
    assert_eq!(
        owner.forwarded().take("toolu_1"),
        Some(structured(r#"{"rows":[1,2]}"#))
    );
}

#[tokio::test]
async fn s2_a_structured_result_past_the_bound_is_kept_as_said_never_cut() {
    let (session, owner, _) = granted(Behaviour::default()).await;
    let mut harness = Harness::attach(session);
    let past = "a".repeat(MAX_STRUCTURED_RESULT_BYTES);
    harness
        .send(call(
            json!(1),
            "echo",
            json!({ "pad": past }),
            json!("toolu_big"),
        ))
        .await;
    let answer = harness.next().await.unwrap();
    // The harness still has all of it.
    assert_eq!(answer["result"]["structuredContent"]["pad"], json!(past));
    assert_eq!(
        owner.forwarded().take("toolu_big"),
        Some(ToolContent::text(STRUCTURED_RESULT_OMITTED))
    );
    // At the bound exactly, it is kept: `{"p":"…"}` is 8 bytes around the text.
    let mut harness = Harness::attach(granted_again(&owner).await);
    let fits = "a".repeat(MAX_STRUCTURED_RESULT_BYTES - 8);
    harness
        .send(call(
            json!(2),
            "echo",
            json!({ "p": fits }),
            json!("toolu_fits"),
        ))
        .await;
    harness.next().await.unwrap();
    assert_eq!(
        owner.forwarded().take("toolu_fits"),
        Some(structured(&json!({ "p": fits }).to_string()))
    );
}

/// Another listed session under `owner`'s grant.
async fn granted_again(owner: &McpOwner) -> McpSession {
    let (servers, _, _) = servers(Behaviour::default());
    let session = servers.open("fixture", owner.clone()).await.unwrap();
    session.list_tools().await.unwrap();
    session
}

#[tokio::test]
async fn s3_a_result_without_structured_content_keeps_nothing() {
    let (session, owner, _) = granted(Behaviour::default()).await;
    let mut harness = Harness::attach(session);
    // `echo` with no arguments answers `structuredContent: null`; `remember`
    // answers text alone.
    harness
        .send(call(json!(1), "echo", Value::Null, json!("toolu_null")))
        .await;
    harness
        .send(call(json!(2), "remember", json!({}), json!("toolu_text")))
        .await;
    let answers = [harness.next().await.unwrap(), harness.next().await.unwrap()];
    assert!(answers.iter().all(|answer| answer.get("result").is_some()));
    assert_eq!(owner.forwarded().len(), 0);
}

#[tokio::test]
async fn s4_an_error_answer_keeps_nothing() {
    let (session, owner, _) = granted(Behaviour::default()).await;
    let mut harness = Harness::attach(session);
    harness
        .send(call(json!(1), "nope", json!({}), json!("toolu_error")))
        .await;
    let answer = harness.next().await.unwrap();
    assert_eq!(answer["error"]["code"], -32602);
    assert_eq!(owner.forwarded().len(), 0);
}

#[tokio::test]
async fn s4_a_result_too_large_to_answer_the_harness_with_keeps_nothing() {
    let (session, owner, launcher) = granted(silent("tools/call")).await;
    let server = launcher.server(0);
    let mut harness = Harness::attach(session);
    // A long id of the harness's own: the server's answer fits a frame under
    // the connection's short id, and the harness's would not.
    let id = json!("i".repeat(200));
    harness
        .send(call(id.clone(), "echo", json!({}), json!("toolu_huge")))
        .await;
    server.arrived("tools/call", 1).await;
    let upstream = server.with_method("tools/call")[0]["id"].clone();
    let answer = |text: String| {
        json!({ "jsonrpc": "2.0", "id": upstream, "result": {
            "content": [{ "type": "text", "text": text }], "structuredContent": { "rows": 1 } } })
    };
    let padding = MAX_FRAME_BYTES - serde_json::to_vec(&answer(String::new())).unwrap().len();
    server.send(answer("a".repeat(padding)));
    let refused = harness.next().await.unwrap();
    assert_eq!(
        (refused["id"].clone(), refused["error"]["code"].clone()),
        (id, json!(-32603))
    );
    assert_eq!(owner.forwarded().len(), 0);
}

#[tokio::test]
async fn s5_a_call_naming_no_usable_call_id_keeps_nothing() {
    let (session, owner, _) = granted(Behaviour::default()).await;
    let mut harness = Harness::attach(session);
    // No `_meta` at all, as Codex's harness sends.
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 0, "method": "tools/call",
                      "params": { "name": "echo", "arguments": { "rows": 1 } } }))
        .await;
    harness.next().await.unwrap();
    let unusable = [json!(7), json!(""), json!(null), json!("t".repeat(257))];
    for (id, unusable) in unusable.into_iter().enumerate() {
        harness
            .send(call(json!(id + 1), "echo", json!({ "rows": 1 }), unusable))
            .await;
        harness.next().await.unwrap();
    }
    assert_eq!(owner.forwarded().len(), 0);
    // 256 bytes is a usable id.
    harness
        .send(call(
            json!(9),
            "echo",
            json!({ "rows": 1 }),
            json!("t".repeat(256)),
        ))
        .await;
    harness.next().await.unwrap();
    assert_eq!(
        owner.forwarded().take(&"t".repeat(256)),
        Some(structured(r#"{"rows":1}"#))
    );
}

#[tokio::test]
async fn s6_a_call_cancelled_before_its_answer_keeps_nothing() {
    let (session, owner, launcher) = granted(silent("tools/call")).await;
    let server = launcher.server(0);
    let mut harness = Harness::attach(session);
    harness
        .send(call(
            json!("a"),
            "echo",
            json!({}),
            json!("toolu_cancelled"),
        ))
        .await;
    server.arrived("tools/call", 1).await;
    let upstream = server.with_method("tools/call")[0]["id"].clone();
    harness
        .send(json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": "a" } }))
        .await;
    server.arrived("notifications/cancelled", 1).await;
    server.send(json!({ "jsonrpc": "2.0", "id": upstream,
                        "result": { "content": [], "structuredContent": { "late": true } } }));
    // A ping after it is answered, so the late answer has been read and dropped.
    harness
        .send(json!({ "jsonrpc": "2.0", "id": "a", "method": "ping" }))
        .await;
    assert_eq!(harness.next().await.unwrap()["id"], "a");
    assert_eq!(owner.forwarded().len(), 0);
}

#[tokio::test]
async fn s7_a_refused_call_to_a_hidden_tool_keeps_nothing() {
    let behaviour = Behaviour {
        pages: vec![vec![
            json!({ "name": "echo", "_meta": { "ui": { "resourceUri": "ui://f/c", "visibility": ["app"] } } }),
        ]],
        ..Behaviour::default()
    };
    let (session, owner, launcher) = granted(behaviour).await;
    let mut harness = Harness::attach(session);
    harness
        .send(call(
            json!(1),
            "echo",
            json!({ "rows": 1 }),
            json!("toolu_hidden"),
        ))
        .await;
    assert_eq!(harness.next().await.unwrap()["error"]["code"], -32602);
    assert_eq!(launcher.server(0).with_method("tools/call").len(), 0);
    assert_eq!(owner.forwarded().len(), 0);
}

#[tokio::test]
async fn s8_an_answer_to_any_other_method_keeps_nothing() {
    let (session, owner, launcher) = granted(silent("ping")).await;
    let server = launcher.server(0);
    let mut harness = Harness::attach(session);
    harness
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping",
                      "params": { "_meta": { "claudecode/toolUseId": "toolu_ping" } } }))
        .await;
    server.arrived("ping", 1).await;
    let upstream = server.with_method("ping")[0]["id"].clone();
    server.send(json!({ "jsonrpc": "2.0", "id": upstream,
                        "result": { "structuredContent": { "rows": 1 } } }));
    assert_eq!(harness.next().await.unwrap()["id"], 1);
    assert_eq!(owner.forwarded().len(), 0);
}

#[test]
fn s9_past_the_bound_the_oldest_result_is_dropped() {
    let forwarded = ForwardedResults::default();
    for call in 0..=MAX_FORWARDED_RESULTS {
        forwarded.record(format!("toolu_{call}"), structured(&call.to_string()));
    }
    assert_eq!(forwarded.len(), MAX_FORWARDED_RESULTS);
    assert_eq!(forwarded.take("toolu_0"), None);
    assert_eq!(forwarded.take("toolu_1"), Some(structured("1")));
    let last = format!("toolu_{MAX_FORWARDED_RESULTS}");
    assert_eq!(
        forwarded.take(&last),
        Some(structured(&MAX_FORWARDED_RESULTS.to_string()))
    );
}

#[test]
fn s10_an_id_kept_again_holds_the_later_result_once() {
    let forwarded = ForwardedResults::default();
    forwarded.record("toolu_1".into(), structured("1"));
    forwarded.record("toolu_1".into(), structured("2"));
    assert_eq!(forwarded.len(), 1);
    assert_eq!(forwarded.take("toolu_1"), Some(structured("2")));
    assert_eq!(forwarded.take("toolu_1"), None);
}

#[tokio::test]
async fn s11_a_grant_keeps_only_its_own_stand_ins_results() {
    let (session, owner, _) = granted(Behaviour::default()).await;
    let other = McpOwner::new(conversation());
    let (servers, _, _) = servers(Behaviour::default());
    let others = servers.open("fixture", other.clone()).await.unwrap();
    others.list_tools().await.unwrap();
    let mut harness = Harness::attach(session);
    harness
        .send(call(
            json!(1),
            "echo",
            json!({ "rows": 1 }),
            json!("toolu_mine"),
        ))
        .await;
    harness.next().await.unwrap();
    // Another grant of the same conversation sees none of it.
    assert_eq!(other.forwarded().len(), 0);
    // The grant's clones share one store: the host's handle is the stand-in's.
    assert_eq!(owner.clone().forwarded().len(), 1);
    assert_eq!(
        owner.forwarded().take("toolu_mine"),
        Some(structured(r#"{"rows":1}"#))
    );
}
