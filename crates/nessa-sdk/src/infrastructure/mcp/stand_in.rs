//! A harness's view of a server whose connection the host holds.
//!
//! ```text
//! harness ──initialize──────────────▶ answered here, from the upstream's own answer
//!         ──request (its id)────────▶ Connection::call (an id of the connection's)
//!         ◀─answer (its id)─────────┘   tools/list: without the model's hidden tools
//!         ──notifications/cancelled─▶ that call dropped: cancelled upstream
//!         ◀─*/list_changed────────── the connection's notices
//! ```
//!
//! Arrows are frames. The stand-in ends when the harness closes, its input
//! breaks, it sends a frame that is not JSON or is past the bound, or the
//! connection ends; its calls still waiting are dropped then, which cancels
//! each upstream.
use super::connection::{Connection, Reply};
use super::framing::{self, Frames, MAX_FRAME_BYTES};
use super::{wire, McpError};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::broadcast::error::RecvError,
    task::{AbortHandle, Id, JoinSet},
};

/// What a finished call hands back: the harness's key and id for it, the
/// server's reply, and what a `tools/list` answer said of each tool's
/// visibility to the model.
type Finished = (String, Value, Result<Reply, McpError>, Vec<(String, bool)>);

/// Serve one harness's side of `connection` over `input` and `output` until
/// either ends. `initialized` is the upstream's answer to the client's own
/// `initialize`; the harness gets it, without `resources.subscribe`.
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
    let mut calls: JoinSet<Finished> = JoinSet::new();
    // By the harness's id, as JSON text, so `1` and `"1"` stay distinct; with
    // the task answering it, so an id reused after a cancellation is not
    // answered with the cancelled call's reply.
    let mut waiting: HashMap<String, AbortHandle> = HashMap::new();
    // Tools a list this stand-in forwarded left out: the model may not call them.
    let mut hidden: HashSet<String> = HashSet::new();
    loop {
        let frame = tokio::select! {
            _ = &mut ended => return,
            frame = frames.next() => frame,
            Some(done) = calls.join_next_with_id(), if !calls.is_empty() => {
                // An aborted call has nothing to answer: its harness cancelled it.
                if let Ok((task, (key, id, reply, listed))) = done {
                    if !answered_by(&waiting, &key, task) {
                        continue;
                    }
                    waiting.remove(&key);
                    // The latest list a tool was in says whether it is hidden.
                    for (name, visible) in listed {
                        if visible {
                            hidden.remove(&name);
                        } else {
                            hidden.insert(name);
                        }
                    }
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
        let refusal = match (method, &id) {
            (Some("initialize"), Some(id)) => {
                Some(json!({ "jsonrpc": "2.0", "id": id, "result": for_harness(&initialized) }))
            }
            // One session's subscription would be every client's of it, and
            // its updates are not routed back; the capability is not offered.
            (Some("resources/subscribe" | "resources/unsubscribe"), Some(id)) => Some(failure(
                id,
                -32601,
                "resource subscriptions are not offered",
            )),
            (Some("tools/call"), Some(id))
                if message
                    .pointer("/params/name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| hidden.contains(name)) =>
            {
                Some(failure(
                    id,
                    -32602,
                    "this tool is not available to the model",
                ))
            }
            (Some(_), Some(id)) if waiting.contains_key(&id.to_string()) => Some(failure(
                id,
                -32600,
                "a request with this id is already waiting",
            )),
            _ => None,
        };
        if let Some(refusal) = refusal {
            if !send(&mut output, &refusal).await {
                return;
            }
            continue;
        }
        match (method, id) {
            (Some(method), Some(id)) => {
                let key = id.to_string();
                let connection = connection.clone();
                let method = method.to_owned();
                let params = message.get("params").cloned();
                let call = calls.spawn({
                    let key = key.clone();
                    async move {
                        let reply = connection.call(&method, params).await;
                        let (reply, listed) = match reply {
                            Ok(Ok(result)) if method == "tools/list" => {
                                let (result, listed) = for_model(result);
                                (Ok(Ok(result)), listed)
                            }
                            other => (other, Vec::new()),
                        };
                        (key, id, reply, listed)
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

/// Whether the call that finished as `task` is still the one waiting under
/// `key`: not when it was cancelled, or its id reused since.
pub(super) fn answered_by(waiting: &HashMap<String, AbortHandle>, key: &str, task: Id) -> bool {
    waiting.get(key).is_some_and(|call| call.id() == task)
}

/// The upstream's `initialize` answer as a harness is given it: without
/// `resources.subscribe`, which is not offered.
fn for_harness(initialized: &Value) -> Value {
    let mut answer = initialized.clone();
    if let Some(resources) = answer
        .pointer_mut("/capabilities/resources")
        .and_then(Value::as_object_mut)
    {
        resources.remove("subscribe");
    }
    answer
}

/// A `tools/list` result without the tools the model may not see
/// ([`wire::model_may_see`]), and each listed tool's name with whether it may.
fn for_model(mut result: Value) -> (Value, Vec<(String, bool)>) {
    let mut listed = Vec::new();
    if let Some(tools) = result.get_mut("tools").and_then(Value::as_array_mut) {
        tools.retain(|tool| {
            let visible = wire::model_may_see(tool);
            if let Some(name) = tool.get("name").and_then(Value::as_str) {
                listed.push((name.to_owned(), visible));
            }
            visible
        });
    }
    (result, listed)
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
