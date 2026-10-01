//! An MCP server in the test's own process, launched through the client's
//! [`Launcher`] seam over in-memory pipes. Each launch is one generation; the
//! test holds a [`Control`] for each, to read what the client sent, write
//! what a server would, and end it.
use super::super::{
    process::{Launched, Launcher},
    McpError, McpServerLaunch,
};
use crate::domain::mcp_apps::APP_MIME_TYPE;
use crate::infrastructure::acp::sessions::StdioMcpServer;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream},
    sync::{mpsc, Notify},
};

/// The chart app every fixture serves.
pub(super) const CHART: &str = "ui://fixture/chart.html";
pub(super) const CHART_HTML: &str = "<!doctype html><p>chart</p>";

/// How a fixture server behaves. The default is a well-behaved server with
/// one tool with UI (`show_chart`), one without (`report`), and its app.
#[derive(Clone)]
pub(super) struct Behaviour {
    /// The protocol version `initialize` answers with; `None` answers an error.
    pub version: Option<&'static str>,
    /// `tools/list` pages, in order.
    pub pages: Vec<Vec<Value>>,
    /// `resources/read` contents by URI.
    pub resources: BTreeMap<String, Value>,
    /// Methods the server never answers.
    pub silent: HashSet<&'static str>,
}
impl Default for Behaviour {
    fn default() -> Self {
        Self {
            version: Some("2025-06-18"),
            pages: vec![vec![
                json!({ "name": "show_chart", "inputSchema": {"type": "object"},
                        "_meta": { "ui": { "resourceUri": CHART } } }),
                json!({ "name": "report", "inputSchema": {"type": "object"} }),
            ]],
            resources: BTreeMap::from([(
                CHART.to_owned(),
                json!({ "uri": CHART, "mimeType": APP_MIME_TYPE, "text": CHART_HTML,
                        "_meta": { "ui": { "csp": { "connectDomains": ["https://api.example.com"] },
                                           "permissions": { "camera": {} }, "prefersBorder": true } } }),
            )]),
            silent: HashSet::new(),
        }
    }
}

/// One launched fixture server, as the test sees it.
#[derive(Clone)]
pub(super) struct Control {
    received: Arc<Mutex<Vec<Value>>>,
    arrived: Arc<Notify>,
    to_client: mpsc::UnboundedSender<Vec<u8>>,
    /// The pages `tools/list` answers with from now on.
    pages: Arc<Mutex<Vec<Vec<Value>>>>,
}
impl Control {
    /// Answer `tools/list` with `pages` from now on.
    pub fn set_pages(&self, pages: Vec<Vec<Value>>) {
        *self.pages.lock().unwrap() = pages;
    }
    /// Wait until a received message satisfies `wanted`.
    pub async fn arrived_where(&self, wanted: impl Fn(&Value) -> bool) -> Value {
        loop {
            let notified = self.arrived.notified();
            if let Some(found) = self.received().into_iter().find(|message| wanted(message)) {
                return found;
            }
            notified.await;
        }
    }
    /// Every message the server has received so far.
    pub fn received(&self) -> Vec<Value> {
        self.received.lock().unwrap().clone()
    }
    /// The received messages with `method`.
    pub fn with_method(&self, method: &str) -> Vec<Value> {
        self.received()
            .into_iter()
            .filter(|message| message["method"] == method)
            .collect()
    }
    /// Wait until a message with `method` has arrived `count` times.
    pub async fn arrived(&self, method: &str, count: usize) {
        loop {
            let notified = self.arrived.notified();
            if self.with_method(method).len() >= count {
                return;
            }
            notified.await;
        }
    }
    /// Write `bytes` to the client as they are.
    pub fn send_raw(&self, bytes: Vec<u8>) {
        let _ = self.to_client.send(bytes);
    }
    /// Write `message` to the client as one frame.
    pub fn send(&self, message: Value) {
        let mut bytes = serde_json::to_vec(&message).unwrap();
        bytes.push(b'\n');
        self.send_raw(bytes);
    }
    /// End the server: its stdout closes, as when the process exits.
    pub fn exit(&self) {
        let _ = self.to_client.send(Vec::new());
    }
}

/// Launches fixture servers with `behaviour`, keeping a [`Control`] for each.
pub(super) struct FixtureLauncher {
    pub behaviour: Mutex<Behaviour>,
    pub launched: Mutex<Vec<Control>>,
    /// When set, launching fails with this.
    pub refuse: Mutex<Option<McpError>>,
}
impl FixtureLauncher {
    pub fn new(behaviour: Behaviour) -> Arc<Self> {
        Arc::new(Self {
            behaviour: Mutex::new(behaviour),
            launched: Mutex::default(),
            refuse: Mutex::default(),
        })
    }
    /// The `index`th generation's control.
    pub fn generation(&self, index: usize) -> Control {
        self.launched.lock().unwrap()[index].clone()
    }
    pub fn launches(&self) -> usize {
        self.launched.lock().unwrap().len()
    }
}
impl Launcher for FixtureLauncher {
    fn launch(&self, _: &McpServerLaunch) -> Result<Launched, McpError> {
        if let Some(error) = self.refuse.lock().unwrap().clone() {
            return Err(error);
        }
        let (client_input, server_output) = tokio::io::duplex(64 * 1024);
        let (server_input, client_output) = tokio::io::duplex(64 * 1024);
        let (to_client, from_test) = mpsc::unbounded_channel();
        let behaviour = self.behaviour.lock().unwrap().clone();
        let control = Control {
            received: Arc::default(),
            arrived: Arc::default(),
            to_client,
            pages: Arc::new(Mutex::new(behaviour.pages.clone())),
        };
        tokio::spawn(serve(
            behaviour,
            control.clone(),
            server_input,
            server_output,
            from_test,
        ));
        self.launched.lock().unwrap().push(control);
        Ok(Launched {
            output: Box::new(client_input),
            input: Box::new(client_output),
            process: None,
        })
    }
}

/// A launch configuration the fixture launcher ignores but the client checks.
pub(super) fn launch(name: &str) -> McpServerLaunch {
    McpServerLaunch {
        server: StdioMcpServer {
            name: name.into(),
            command: PathBuf::from("/fixture"),
            args: vec![],
        },
        working_directory: PathBuf::from("/"),
        environment: BTreeMap::new(),
    }
}

async fn serve(
    behaviour: Behaviour,
    control: Control,
    input: DuplexStream,
    mut output: DuplexStream,
    mut from_test: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    let mut lines = BufReader::new(input).lines();
    // Handles this generation has given out: its session's state.
    let mut handles: HashSet<String> = HashSet::new();
    loop {
        let line = tokio::select! {
            line = lines.next_line() => line,
            bytes = from_test.recv() => match bytes {
                Some(bytes) if !bytes.is_empty() => {
                    if output.write_all(&bytes).await.is_err() { return }
                    continue;
                }
                _ => return,
            },
        };
        let Ok(Some(line)) = line else { return };
        let message: Value = serde_json::from_str(&line).unwrap();
        control.received.lock().unwrap().push(message.clone());
        control.arrived.notify_waiters();
        let (Some(method), Some(id)) = (message["method"].as_str(), message.get("id")) else {
            continue;
        };
        if behaviour.silent.contains(method) {
            continue;
        }
        let ok = |result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });
        let error = |code: i64, text: &str| json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": text } });
        let answer = match method {
            "initialize" => match behaviour.version {
                Some(version) => ok(
                    json!({ "protocolVersion": version, "capabilities": { "tools": {} },
                                            "serverInfo": { "name": "fixture", "version": "1" } }),
                ),
                None => error(-32600, "no"),
            },
            "tools/list" => {
                let page = message["params"]["cursor"].as_str().map_or(0, |cursor| {
                    cursor.trim_start_matches("page-").parse().unwrap()
                });
                let pages = control.pages.lock().unwrap().clone();
                let mut result = json!({ "tools": pages[page] });
                if page + 1 < pages.len() {
                    result["nextCursor"] = json!(format!("page-{}", page + 1));
                }
                ok(result)
            }
            "resources/read" => {
                let uri = message["params"]["uri"].as_str().unwrap();
                if let Some(handle) = uri.strip_prefix("ui://fixture/handle/") {
                    if handles.contains(handle) {
                        ok(
                            json!({ "contents": [{ "uri": uri, "mimeType": APP_MIME_TYPE,
                                                  "text": format!("<p>{handle}</p>") }] }),
                        )
                    } else {
                        error(-32002, "unknown handle")
                    }
                } else if let Some(content) = behaviour.resources.get(uri) {
                    ok(json!({ "contents": [content] }))
                } else {
                    error(-32002, "not found")
                }
            }
            "tools/call" => match message["params"]["name"].as_str() {
                Some("remember") => {
                    let handle = format!("h{}", handles.len() + 1);
                    handles.insert(handle.clone());
                    ok(json!({ "content": [{ "type": "text", "text": handle }] }))
                }
                Some("echo") => ok(
                    json!({ "content": [], "structuredContent": message["params"]["arguments"] }),
                ),
                Some("big") => {
                    let bytes = message["params"]["arguments"]["bytes"].as_u64().unwrap() as usize;
                    ok(json!({ "content": [{ "type": "text", "text": "a".repeat(bytes) }] }))
                }
                _ => error(-32602, "unknown tool"),
            },
            "ping" => ok(json!({})),
            _ => error(-32601, "method not found"),
        };
        let mut bytes = serde_json::to_vec(&answer).unwrap();
        bytes.push(b'\n');
        if output.write_all(&bytes).await.is_err() {
            return;
        }
    }
}
