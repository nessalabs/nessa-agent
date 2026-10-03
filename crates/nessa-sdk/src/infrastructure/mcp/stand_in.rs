//! A harness's view of a server whose connection the host holds.
//!
//! ```text
//! harness ──initialize──────────────▶ answered here, from the upstream's own answer
//!         ──request (its id)────────▶ Connection::call (an id of the connection's)
//!         ◀─answer (its id)─────────┘   tools/list: without the model's hidden tools
//!                                           tools/call: structuredContent kept first
//!         ──notifications/cancelled─▶ that call dropped: cancelled upstream
//!         ◀─*/list_changed────────── the connection's notices
//! ```
//!
//! Arrows are frames. The stand-in ends when the harness closes, its input
//! breaks, it sends a frame that is not JSON or is past the bound, or the
//! connection ends. Its calls still waiting are dropped then, unanswered; the
//! session closing after it ends them upstream by closing the server's stdin.
use super::connection::{Connection, Reply};
use super::forwarded::{self, ForwardedResults};
use super::framing::{self, Frames, MAX_FRAME_BYTES};
use super::{wire, McpError};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::broadcast::error::RecvError,
    task::{AbortHandle, Id, JoinSet},
};

/// What a finished call hands back: the harness's key and id for it, the
/// server's reply, for a `tools/list` the order it was asked in and which of
/// the tools its answer named it hid from the model, and for a `tools/call`
/// the harness's own id for the call, when it named one.
type Finished = (
    String,
    Value,
    Result<Reply, McpError>,
    Option<(u64, Vec<(String, bool)>)>,
    Option<String>,
);

/// What a stand-in sends a harness that fell too far behind the server's
/// change notices to have them all: all three, which are idempotent.
const CHANGES: [&str; 3] = [
    "notifications/tools/list_changed",
    "notifications/resources/list_changed",
    "notifications/prompts/list_changed",
];

/// The most tool names a session remembers the visibility of. Past it, only
/// the names the list asked latest gave are kept — and if that is still too
/// many, none are — and a name not remembered is taken as hidden: past the
/// bound the rule fails closed.
pub(crate) const MAX_REMEMBERED_TOOLS: usize = 4096;

/// Which of a session's tools the model may not see, as the list asked latest
/// that named each tool said — whether the session listed it for itself or a
/// stand-in forwarded the list. Lists are numbered as they are asked, so one
/// answered late cannot undo a later one.
#[derive(Default)]
pub(crate) struct Visibility {
    asked: AtomicU64,
    known: Mutex<Known>,
}

#[derive(Default)]
struct Known {
    /// By name: the number of the list that said it, and whether it hid it.
    tools: HashMap<String, (u64, bool)>,
    /// Set once a list has said anything: until then no name is known.
    listed: bool,
    /// Set once names have been forgotten past the bound.
    forgot: bool,
}
impl Visibility {
    /// The number of a list about to be asked.
    pub(crate) fn ask(&self) -> u64 {
        self.asked.fetch_add(1, Ordering::Relaxed) + 1
    }
    /// What list `order` said of each tool it named: hidden or not. A name
    /// the list gives twice is hidden if either says so.
    pub(crate) fn listed(&self, order: u64, tools: impl IntoIterator<Item = (String, bool)>) {
        let mut said: HashMap<String, bool> = HashMap::new();
        for (name, hidden) in tools {
            *said.entry(name).or_default() |= hidden;
        }
        let mut known = self.known.lock().expect("visibility");
        known.listed = true;
        for (name, hidden) in said {
            let entry = known.tools.entry(name).or_insert((order, hidden));
            if entry.0 <= order {
                *entry = (order, hidden);
            }
        }
        if known.tools.len() > MAX_REMEMBERED_TOOLS {
            // Keep what the list asked latest said — which may not be this
            // one, if this one was answered late — forget the rest, and all
            // of it if that is still too much.
            let latest = known.tools.values().map(|(said, _)| *said).max();
            known.tools.retain(|_, (said, _)| Some(*said) == latest);
            if known.tools.len() > MAX_REMEMBERED_TOOLS {
                known.tools.clear();
            }
            known.forgot = true;
        }
    }
    /// Whether the latest list naming `name` hid it from the model. A tool no
    /// list has named is not hidden — the server decides about it — once a
    /// list has said anything; before then, or once names have been forgotten
    /// past the bound, it is.
    pub(crate) fn hidden(&self, name: &str) -> bool {
        let known = self.known.lock().expect("visibility");
        known
            .tools
            .get(name)
            .map_or(!known.listed || known.forgot, |(_, hidden)| *hidden)
    }
}

/// Serve one harness's side of `connection` over `input` and `output` until
/// either ends. `initialized` is the upstream's answer to the client's own
/// `initialize`; the harness gets it, without `resources.subscribe`.
/// `visibility` is the session's: what its lists, and this stand-in's, said
/// the model may not see. `forwarded` is its grant's: where a `tools/call`
/// result's `structuredContent` is kept, under the harness's id for the call,
/// before the harness is answered.
pub(crate) async fn serve(
    connection: Arc<Connection>,
    initialized: Arc<Value>,
    visibility: Arc<Visibility>,
    forwarded: ForwardedResults,
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
    loop {
        let frame = tokio::select! {
            _ = &mut ended => return,
            frame = frames.next() => frame,
            Some(done) = calls.join_next_with_id(), if !calls.is_empty() => {
                // An aborted call has nothing to answer: its harness cancelled it.
                if let Ok((task, (key, id, reply, listed, call))) = done {
                    if !answered_by(&waiting, &key, task) {
                        continue;
                    }
                    waiting.remove(&key);
                    if let Some((order, listed)) = listed {
                        visibility.listed(order, listed);
                    }
                    let answer = answer(&id, reply);
                    // Kept before the harness has it, so the harness cannot
                    // report the call before its result is here to take.
                    if let Some(call) = call {
                        keep_structured(&forwarded, call, &answer);
                    }
                    if !send(&mut output, &answer).await {
                        return;
                    }
                }
                continue;
            }
            notice = notices.recv() => {
                match notice {
                    Ok(notice) if !send(&mut output, &notice).await => return,
                    Ok(_) => {}
                    Err(RecvError::Lagged(_)) => {
                        for method in CHANGES {
                            let notice = json!({ "jsonrpc": "2.0", "method": method });
                            if !send(&mut output, &notice).await {
                                return;
                            }
                        }
                    }
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
                    .is_some_and(|name| visibility.hidden(name)) =>
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
                let order = (method == "tools/list").then(|| visibility.ask());
                let call = if method == "tools/call" {
                    forwarded::call_id(params.as_ref())
                } else {
                    None
                };
                let call = calls.spawn({
                    let key = key.clone();
                    async move {
                        let reply = connection.call(&method, params).await;
                        let (reply, listed) = match (reply, order) {
                            (Ok(Ok(result)), Some(order)) => {
                                let (result, listed) = for_model(result);
                                (Ok(Ok(result)), listed.map(|listed| (order, listed)))
                            }
                            (other, _) => (other, None),
                        };
                        (key, id, reply, listed, call)
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

/// Keep the `structuredContent` of `answer` — the harness's answer, as it will
/// be written — under `call`, when it is a result that has one. An error,
/// including a result too large for a frame ([`answer`]), keeps nothing.
fn keep_structured(forwarded: &ForwardedResults, call: String, answer: &Value) {
    let Some(structured) = answer
        .get("result")
        .and_then(|result| result.get("structuredContent"))
        .filter(|structured| !structured.is_null())
    else {
        return;
    };
    if let Ok(result) = wire::structured_result(structured) {
        forwarded.record(call, result);
    }
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
/// ([`wire::model_may_see`]), and each listed tool's name with whether it is
/// hidden — none for a result with no tools array, which says nothing.
fn for_model(mut result: Value) -> (Value, Option<Vec<(String, bool)>>) {
    let Some(tools) = result.get_mut("tools").and_then(Value::as_array_mut) else {
        return (result, None);
    };
    let mut listed = Vec::new();
    tools.retain(|tool| {
        let visible = wire::model_may_see(tool);
        if let Some(name) = tool.get("name").and_then(Value::as_str) {
            listed.push((name.to_owned(), !visible));
        }
        visible
    });
    (result, Some(listed))
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
