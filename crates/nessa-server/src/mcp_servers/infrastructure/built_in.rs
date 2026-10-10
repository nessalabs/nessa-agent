//! A server the gateway serves itself, on a stand-in's relay connection, in
//! place of a process it would relay to (issue #700): MCP's JSON-RPC, one
//! message a line, as a stdio server speaks it.
//!
//! ```text
//! Relay::serve ──hello admitted──▶ serve(server, owner, token)
//!   initialize ──▶ its protocol version and its tools capability
//!   tools/list ──▶ BuiltInServer::tools
//!   tools/call ──▶ BuiltInServer::call(session, call, tool, arguments, stop) ──▶ its result
//!   notifications/cancelled ──▶ that call's stop
//!   the stand-in gone, or its grant revoked ──▶ every call's stop ──▶ the end
//! ```
//!
//! Arrows are messages and calls, in order. A call runs on a task of its
//! own, so the connection keeps reading while it runs, and at most
//! [`MAX_BUILT_IN_CALLS`] run at once. A line past [`MAX_BUILT_IN_LINE_BYTES`]
//! ends the connection. Stopping a call asks it to stop; it records its own
//! end, whether or not anyone is left to read its answer.
use super::grants::ConversationGrants;
use crate::mcp_servers::application::BuiltInServer;
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use serde_json::{json, Value};
use std::{collections::HashMap, io, sync::Arc};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, watch},
};

/// The longest message a harness may send, newline included.
pub const MAX_BUILT_IN_LINE_BYTES: usize = 64 * 1024;
/// Most tool calls one connection runs at once.
pub const MAX_BUILT_IN_CALLS: usize = 8;
/// The MCP versions served, newest first: an `initialize` asking another is
/// answered with the newest.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// Serve `server` to the stand-in of SDK session `session`'s open, whose
/// grant `token` names, until the stand-in goes or the grant is revoked.
pub(crate) async fn serve(
    server: Arc<dyn BuiltInServer>,
    session: SessionId,
    token: String,
    grants: ConversationGrants,
    mut input: impl AsyncBufRead + Unpin,
    mut output: impl AsyncWrite + Unpin,
) {
    let mut revocations = grants.revocations();
    let (answers, mut answered) = mpsc::channel::<(String, Value)>(MAX_BUILT_IN_CALLS);
    // Each running call's stop, by its request id as JSON.
    let mut calls: HashMap<String, watch::Sender<bool>> = HashMap::new();
    let mut line = Vec::new();
    loop {
        tokio::select! {
            read = next_line(&mut input, &mut line) => {
                let Ok(Some(message)) = read else { break };
                let answer = handle(&server, &session, &mut calls, &answers, &message);
                if let Some(answer) = answer {
                    if write(&mut output, &answer).await.is_err() {
                        break;
                    }
                }
            }
            Some((id, result)) = answered.recv() => {
                let Some(stop) = calls.remove(&id) else { continue };
                // A call its caller cancelled is not answered.
                if *stop.borrow() {
                    continue;
                }
                let id: Value = serde_json::from_str(&id).unwrap_or(Value::Null);
                if write(&mut output, &json!({"jsonrpc": "2.0", "id": id, "result": result}))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            changed = revocations.changed() => {
                if changed.is_err() || grants.owner(&token).is_none() {
                    break;
                }
            }
        }
    }
    for stop in calls.into_values() {
        stop.send_replace(true);
    }
}

/// The answer to one message, if it has one; a call answers later.
fn handle(
    server: &Arc<dyn BuiltInServer>,
    session: &SessionId,
    calls: &mut HashMap<String, watch::Sender<bool>>,
    answers: &mpsc::Sender<(String, Value)>,
    message: &[u8],
) -> Option<Value> {
    let Ok(message) = serde_json::from_slice::<Value>(message) else {
        return Some(error(Value::Null, -32700, "the message is not JSON"));
    };
    let method = message.get("method").and_then(Value::as_str);
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = message.get("id").cloned() else {
        // A notification: only a cancellation is acted on.
        if method == Some("notifications/cancelled") {
            if let Some(request) = params.get("requestId") {
                if let Some(stop) = calls.get(&request.to_string()) {
                    stop.send_replace(true);
                }
            }
        }
        return None;
    };
    let result = match method {
        // A response to something this server never asks: ignored.
        None => return None,
        Some("initialize") => {
            let asked = params.get("protocolVersion").and_then(Value::as_str);
            let version = PROTOCOL_VERSIONS
                .into_iter()
                .find(|version| Some(*version) == asked)
                .unwrap_or(PROTOCOL_VERSIONS[0]);
            json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": server.name(), "version": env!("CARGO_PKG_VERSION")},
            })
        }
        Some("ping") => json!({}),
        Some("tools/list") => json!({"tools": server.tools()}),
        Some("tools/call") => {
            let key = id.to_string();
            if calls.contains_key(&key) {
                return Some(error(id, -32600, "a call with this id is already running"));
            }
            if calls.len() >= MAX_BUILT_IN_CALLS {
                return Some(error(id, -32000, "too many calls are running on this server"));
            }
            let Some(tool) = params.get("name").and_then(Value::as_str) else {
                return Some(error(id, -32602, "a tool call names its tool"));
            };
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            let (stop, stopped) = watch::channel(false);
            let call = server.call(
                session,
                uuid::Uuid::new_v4().to_string(),
                tool.to_owned(),
                arguments,
                stopped,
            );
            calls.insert(key.clone(), stop);
            let answers = answers.clone();
            tokio::spawn(async move {
                let result = call.await;
                let _ = answers.send((key, result)).await;
            });
            return None;
        }
        Some(_) => return Some(error(id, -32601, "no such method")),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

async fn write(output: &mut (impl AsyncWrite + Unpin), message: &Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(message).map_err(io::Error::other)?;
    bytes.push(b'\n');
    output.write_all(&bytes).await?;
    output.flush().await
}

/// The next line of `input`, newline included, gathered in `line`; `None` at
/// its end. Safe to cancel: what was read stays in `line` for the next call.
async fn next_line(
    input: &mut (impl AsyncBufRead + Unpin),
    line: &mut Vec<u8>,
) -> io::Result<Option<Vec<u8>>> {
    loop {
        let available = input.fill_buf().await?;
        if available.is_empty() {
            return Ok(None);
        }
        let (taken, ended) = match available.iter().position(|byte| *byte == b'\n') {
            Some(at) => (at + 1, true),
            None => (available.len(), false),
        };
        line.extend_from_slice(&available[..taken]);
        input.consume(taken);
        if line.len() > MAX_BUILT_IN_LINE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a message longer than the server reads",
            ));
        }
        if ended {
            return Ok(Some(std::mem::take(line)));
        }
    }
}
