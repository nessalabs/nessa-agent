//! A harness's view of a server whose one connection someone else holds.
//!
//! ```text
//! harness ──initialize──────────────▶ answered here, from the upstream's own answer
//!         ──request (its id)────────▶ Connection::call (an id of the connection's)
//!         ◀─answer (its id)─────────┘
//!         ──notifications/cancelled─▶ that call dropped: cancelled upstream
//!         ◀─*/list_changed────────── the connection's notices
//! ```
//!
//! Arrows are frames. The stand-in ends when the harness closes, sends a
//! frame that is not JSON or is past the bound, or the connection ends; its
//! calls still waiting are dropped then, which cancels each upstream.
use super::connection::{Connection, Reply};
use super::framing::{self, Frames, MAX_FRAME_BYTES};
use super::McpError;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::broadcast::error::RecvError,
    task::{AbortHandle, JoinSet},
};

/// Serve one harness's side of `connection` over `input` and `output` until
/// either ends. `initialized` is the upstream's answer to the client's own
/// `initialize`, given to the harness as its answer.
pub(crate) async fn serve(
    connection: Arc<Connection>,
    initialized: Arc<Value>,
    input: impl AsyncRead + Unpin,
    mut output: impl AsyncWrite + Unpin,
) {
    let mut frames = Frames::new(input, MAX_FRAME_BYTES);
    let mut notices = connection.notices();
    let ended = connection.ended();
    tokio::pin!(ended);
    let mut calls: JoinSet<(String, Value, Result<Reply, McpError>)> = JoinSet::new();
    // By the harness's id, as JSON text, so `1` and `"1"` stay distinct.
    let mut waiting: HashMap<String, AbortHandle> = HashMap::new();
    loop {
        let frame = tokio::select! {
            _ = &mut ended => return,
            frame = frames.next() => frame,
            Some(done) = calls.join_next(), if !calls.is_empty() => {
                // An aborted call has nothing to answer: its harness cancelled it.
                if let Ok((key, id, reply)) = done {
                    waiting.remove(&key);
                    if !send(&mut output, &answer(&id, reply)).await {
                        return;
                    }
                }
                continue;
            }
            notice = notices.recv() => {
                match notice {
                    Ok(notice) if !send(&mut output, &notice).await => return,
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return,
                }
                continue;
            }
        };
        let Ok(frame) = frame else { return };
        let Ok(message) = serde_json::from_slice::<Value>(&frame) else {
            return;
        };
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").filter(|id| !id.is_null()).cloned();
        match (method, id) {
            (Some("initialize"), Some(id)) => {
                let answer = json!({ "jsonrpc": "2.0", "id": id, "result": *initialized });
                if !send(&mut output, &answer).await {
                    return;
                }
            }
            (Some(method), Some(id)) => {
                let key = id.to_string();
                if waiting.contains_key(&key) {
                    let answer = failure(&id, -32600, "a request with this id is already waiting");
                    if !send(&mut output, &answer).await {
                        return;
                    }
                    continue;
                }
                let connection = connection.clone();
                let method = method.to_owned();
                let params = message.get("params").cloned();
                let call = calls.spawn({
                    let key = key.clone();
                    async move {
                        let reply = connection.call(&method, params).await;
                        (key, id, reply)
                    }
                });
                waiting.insert(key, call);
            }
            (Some("notifications/cancelled"), None) => {
                let cancelled = message.pointer("/params/requestId").map(Value::to_string);
                if let Some(call) = cancelled.and_then(|key| waiting.remove(&key)) {
                    call.abort();
                }
            }
            // `notifications/initialized` (the upstream was initialized once),
            // other notifications, and answers to requests never sent.
            _ => {}
        }
    }
}

/// The harness's answer under its own `id`. One that would not fit a frame
/// is an error for that id, so the harness is not left waiting.
fn answer(id: &Value, reply: Result<Reply, McpError>) -> Value {
    let answer = match reply {
        Ok(Ok(result)) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Ok(Err(error)) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
        Err(error) => return failure(id, -32603, &error.to_string()),
    };
    match framing::encode(&answer) {
        Ok(_) => answer,
        Err(error) => failure(id, -32603, &error.to_string()),
    }
}

fn failure(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Whether `value` was written. Answers are already within the bound
/// ([`answer`]); a notice past it is not forwarded.
async fn send(output: &mut (impl AsyncWrite + Unpin), value: &Value) -> bool {
    match framing::encode(value) {
        Ok(frame) => framing::write(output, &frame).await.is_ok(),
        Err(_) => true,
    }
}
