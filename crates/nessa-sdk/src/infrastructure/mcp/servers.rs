use super::connection::Connection;
use super::process::{self, Launched, Launcher, ProcessLauncher};
use super::{stand_in, wire, McpError};
use crate::domain::agent_execution::tools::McpTool;
use crate::domain::mcp_apps::{ListedTool, ToolUi, UiResource, UiResourceUri};
use crate::infrastructure::acp::sessions::StdioMcpServer;
use crate::infrastructure::clock::{within, Clock};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock, Weak},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    process::Child,
    sync::{broadcast, watch, OnceCell},
    task::JoinSet,
};

/// The budget for starting a server: launching it and its answer to
/// `initialize`.
pub const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(30);
/// The budget for each of this client's own requests (`tools/list` pages,
/// `resources/read`). A forwarded request has none from here.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// The most `tools/list` pages read for one list.
pub const MAX_TOOL_PAGES: usize = 32;
/// The most tools one server may list.
pub const MAX_TOOLS: usize = 1024;

/// One configured stdio server and what it is started with.
#[derive(Clone, Debug)]
pub struct McpServerLaunch {
    /// The trusted configuration: name, absolute executable, arguments.
    pub server: StdioMcpServer,
    /// The server's working directory.
    pub working_directory: PathBuf,
    /// The server's whole environment; nothing is inherited.
    pub environment: BTreeMap<OsString, OsString>,
}

/// The one connection to each configured MCP server, shared by this
/// client's own requests and every stand-in.
///
/// Each server is started on first use, and again on the next use after it
/// ended: there is no retry of its own. One start at a time per server; a use
/// during a start waits for that start. A *generation* — one process, one
/// connection, one upstream MCP session — ends when the process exits, its
/// pipes close, or it sends a frame that is not JSON or is past the frame
/// bound; its stand-ins end with it. The states and orderings are tabled in
/// `docs/design/mcp-connections.md`, and each row has a test in
/// `tests/infrastructure/mcp/`.
///
/// Cloning shares the servers. [`McpServers::stop`] ends them all; until
/// then, dropping the last clone kills every process.
#[derive(Clone)]
pub struct McpServers {
    inner: Arc<Inner>,
}

struct Inner {
    slots: BTreeMap<String, Slot>,
    clock: Arc<dyn Clock>,
    launcher: Arc<dyn Launcher>,
    stopping: watch::Sender<bool>,
}

struct Slot {
    launch: McpServerLaunch,
    current: Mutex<Arc<Generation>>,
    /// The tools as last listed, by whichever generation listed them last.
    tools: RwLock<Arc<[ListedTool]>>,
}

#[derive(Default)]
struct Generation {
    started: OnceCell<Result<Arc<Ready>, McpError>>,
}
impl Generation {
    /// Whether this generation is over: its start failed, or its connection
    /// ended. A start still running is not.
    fn over(&self) -> bool {
        match self.started.get() {
            None => false,
            Some(Err(_)) => true,
            Some(Ok(ready)) => ready.connection.end_cause().is_some(),
        }
    }
}

struct Ready {
    connection: Arc<Connection>,
    /// The server's answer to `initialize`, given to each stand-in as its own.
    initialized: Arc<Value>,
    process: Mutex<Option<Child>>,
}

impl McpServers {
    /// The servers in `servers`, none started yet; deadlines are measured on
    /// `clock`.
    ///
    /// # Errors
    ///
    /// [`McpError::InvalidConfiguration`] unless every server passes
    /// [`StdioMcpServer::all_valid`].
    pub fn new(servers: Vec<McpServerLaunch>, clock: Arc<dyn Clock>) -> Result<Self, McpError> {
        Self::with_launcher(servers, clock, Arc::new(ProcessLauncher))
    }

    pub(crate) fn with_launcher(
        servers: Vec<McpServerLaunch>,
        clock: Arc<dyn Clock>,
        launcher: Arc<dyn Launcher>,
    ) -> Result<Self, McpError> {
        let configured: Vec<StdioMcpServer> =
            servers.iter().map(|each| each.server.clone()).collect();
        if !StdioMcpServer::all_valid(&configured) {
            return Err(McpError::InvalidConfiguration);
        }
        let slots = servers
            .into_iter()
            .map(|launch| {
                let slot = Slot {
                    launch,
                    current: Mutex::default(),
                    tools: RwLock::new(Arc::from([])),
                };
                (slot.launch.server.name.clone(), slot)
            })
            .collect();
        Ok(Self {
            inner: Arc::new(Inner {
                slots,
                clock,
                launcher,
                stopping: watch::channel(false).0,
            }),
        })
    }

    /// The configured servers' names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.inner.slots.keys().map(String::as_str)
    }

    /// Start every server in the background. A server that cannot start is
    /// logged, and started again by its next use.
    pub fn start_all(&self) {
        for name in self.inner.slots.keys() {
            let servers = self.clone();
            let name = name.clone();
            tokio::spawn(async move {
                if let Err(error) = servers.ready(&name).await {
                    tracing::warn!(server = %name, %error, "MCP server did not start");
                }
            });
        }
    }

    /// List `server`'s tools now, with each tool's UI, and keep the list for
    /// [`Self::tool_ui`]. Starts the server when it is not running.
    ///
    /// # Errors
    ///
    /// [`McpError::NotConfigured`], a start's failure, [`McpError::Timeout`]
    /// for a page not answered within [`REQUEST_TIMEOUT`],
    /// [`McpError::TooLarge`] past [`MAX_TOOL_PAGES`] or [`MAX_TOOLS`], and
    /// [`McpError::Malformed`] for a page of the wrong shape.
    pub async fn list_tools(&self, server: &str) -> Result<Vec<ListedTool>, McpError> {
        let ready = self.ready(server).await?;
        list(self.slot(server)?, &ready).await
    }

    /// Read the MCP App at `uri` from `server`. Starts the server when it is
    /// not running.
    ///
    /// # Errors
    ///
    /// [`McpError::NotAnApp`] for a resource that is not
    /// `text/html;profile=mcp-app`, [`McpError::TooLarge`] past the domain's
    /// bounds ([`MAX_UI_HTML_BYTES`](crate::domain::mcp_apps::MAX_UI_HTML_BYTES)
    /// and the CSP's), [`McpError::Remote`] for the server's refusal, and as
    /// [`Self::list_tools`].
    pub async fn read_ui_resource(
        &self,
        server: &str,
        uri: &UiResourceUri,
    ) -> Result<UiResource, McpError> {
        let ready = self.ready(server).await?;
        let params = json!({ "uri": uri.as_str() });
        let result = ready
            .connection
            .request("resources/read", Some(params), REQUEST_TIMEOUT)
            .await?;
        wire::ui_resource(uri, &result)
    }

    /// The UI of the tool an observed call names, from its server's tools as
    /// last listed ([`ListedTool::ui_for`]). `None` for a server not
    /// configured, a list not read yet, or a call naming no tool with a UI.
    pub fn tool_ui(&self, call: &McpTool) -> Option<ToolUi> {
        let slot = self.inner.slots.get(call.server())?;
        let tools = slot.tools.read().expect("tool list").clone();
        ListedTool::ui_for(&tools, call).cloned()
    }

    /// A stand-in for `server`, attached to its current generation; the
    /// server is started when it is not running.
    ///
    /// # Errors
    ///
    /// [`McpError::NotConfigured`], [`McpError::Stopped`], or why the server
    /// could not be started.
    pub async fn stand_in(&self, server: &str) -> Result<StandIn, McpError> {
        Ok(StandIn {
            ready: self.ready(server).await?,
        })
    }

    /// Stop every server: waiting uses and stand-ins end with
    /// [`McpError::Stopped`], each server's stdin is closed, and a server
    /// still running two seconds later is killed.
    /// Every use after this fails [`McpError::Stopped`].
    pub async fn stop(&self) {
        self.inner.stopping.send_replace(true);
        let mut stopping = JoinSet::new();
        for slot in self.inner.slots.values() {
            let generation = slot.current.lock().expect("generation").clone();
            if let Some(Ok(ready)) = generation.started.get() {
                ready.connection.close(McpError::Stopped);
                if let Some(child) = ready.process.lock().expect("process").take() {
                    stopping.spawn(process::stop(child));
                }
            }
        }
        while stopping.join_next().await.is_some() {}
    }

    fn slot(&self, server: &str) -> Result<&Slot, McpError> {
        self.inner.slots.get(server).ok_or(McpError::NotConfigured)
    }

    /// `server`'s running generation, started when there is none.
    async fn ready(&self, server: &str) -> Result<Arc<Ready>, McpError> {
        let slot = self.slot(server)?;
        loop {
            if *self.inner.stopping.borrow() {
                return Err(McpError::Stopped);
            }
            let generation = {
                let mut current = slot.current.lock().expect("generation");
                if current.over() {
                    *current = Arc::default();
                }
                current.clone()
            };
            let started = generation
                .started
                .get_or_init(|| start(&self.inner, slot))
                .await
                .clone();
            match started {
                Ok(ready) if ready.connection.end_cause().is_none() => return Ok(ready),
                // Ended since it started: the next pass starts another.
                Ok(_) => continue,
                Err(error) => return Err(error),
            }
        }
    }
}

/// A harness's connection to a server, attached to one generation of it.
pub struct StandIn {
    ready: Arc<Ready>,
}
impl StandIn {
    /// Serve the harness over `input` and `output` until it closes or the
    /// generation ends. The harness's `initialize` is answered with the
    /// server's own answer; its other requests are forwarded under ids of the
    /// connection's and answered under its own; its cancellations cancel
    /// upstream; the server's `*/list_changed` notices are passed on.
    pub async fn serve(self, input: impl AsyncRead + Unpin, output: impl AsyncWrite + Unpin) {
        stand_in::serve(
            self.ready.connection.clone(),
            self.ready.initialized.clone(),
            input,
            output,
        )
        .await;
    }
}

/// Launch and initialize one generation of `slot`'s server, then list its
/// tools. A failed list leaves the tools as they were and is logged; the
/// server is still started.
async fn start(inner: &Arc<Inner>, slot: &Slot) -> Result<Arc<Ready>, McpError> {
    let deadline = inner.clock.now() + INITIALIZE_TIMEOUT;
    let Launched {
        output,
        input,
        process,
    } = inner.launcher.launch(&slot.launch)?;
    let connection = Arc::new(Connection::open(output, input, inner.clock.clone()));
    let handshake = async {
        let answer = within(
            &*inner.clock,
            deadline,
            connection.call("initialize", Some(wire::initialize_params())),
        )
        .await
        .ok_or(McpError::Timeout)??
        .map_err(|error| match super::connection::remote(&error) {
            McpError::Remote { code, message } => McpError::Handshake(format!("{code}: {message}")),
            other => McpError::Handshake(other.to_string()),
        })?;
        wire::initialized(&answer)?;
        connection.notify("notifications/initialized", None).await?;
        Ok(answer)
    };
    let mut stopping = inner.stopping.subscribe();
    let initialized = tokio::select! {
        answer = handshake => answer,
        _ = stopping.wait_for(|stopping| *stopping) => Err(McpError::Stopped),
    };
    let initialized = match initialized {
        Ok(answer) => answer,
        Err(error) => {
            // The process is killed as it is dropped.
            connection.close(error.clone());
            return Err(error);
        }
    };
    let ready = Arc::new(Ready {
        connection,
        initialized: Arc::new(initialized),
        process: Mutex::new(process),
    });
    // Subscribed before the first list, so a change during it is not missed.
    let notices = ready.connection.notices();
    if let Err(error) = list(slot, &ready).await {
        tracing::warn!(server = %slot.launch.server.name, %error, "MCP server's tools could not be listed");
    }
    if *inner.stopping.borrow() {
        ready.connection.close(McpError::Stopped);
        return Err(McpError::Stopped);
    }
    tokio::spawn(relist(
        Arc::downgrade(inner),
        slot.launch.server.name.clone(),
        ready.clone(),
        notices,
    ));
    Ok(ready)
}

/// List the tools again whenever the server says they changed, until its
/// generation ends.
async fn relist(
    inner: Weak<Inner>,
    name: String,
    ready: Arc<Ready>,
    mut notices: broadcast::Receiver<Arc<Value>>,
) {
    let ended = ready.connection.ended();
    tokio::pin!(ended);
    loop {
        let notice = tokio::select! {
            _ = &mut ended => return,
            notice = notices.recv() => notice,
        };
        let changed = match notice {
            Ok(notice) => tools_changed(&notice),
            Err(broadcast::error::RecvError::Lagged(_)) => true,
            Err(broadcast::error::RecvError::Closed) => return,
        };
        if !changed {
            continue;
        }
        let Some(inner) = inner.upgrade() else { return };
        let Some(slot) = inner.slots.get(&name) else {
            return;
        };
        if let Err(error) = list(slot, &ready).await {
            tracing::warn!(server = %name, %error, "MCP server's changed tools could not be listed");
        }
    }
}

/// Whether a server's notice says its tools changed. Its other lists
/// (resources, prompts) are not this client's to keep.
pub(super) fn tools_changed(notice: &Value) -> bool {
    notice.get("method").and_then(Value::as_str) == Some("notifications/tools/list_changed")
}

/// Every page of `tools/list`, kept as `slot`'s tools when all were read.
async fn list(slot: &Slot, ready: &Ready) -> Result<Vec<ListedTool>, McpError> {
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_TOOL_PAGES {
        let params = cursor.map(|cursor| json!({ "cursor": cursor }));
        let result = ready
            .connection
            .request("tools/list", params, REQUEST_TIMEOUT)
            .await?;
        let (page, next) = wire::tools_page(&slot.launch.server.name, &result)?;
        if tools.len() + page.len() > MAX_TOOLS {
            return Err(McpError::TooLarge("tools/list"));
        }
        tools.extend(page);
        match next {
            Some(next) => cursor = Some(next),
            None => {
                *slot.tools.write().expect("tool list") = Arc::from(tools.clone());
                return Ok(tools);
            }
        }
    }
    Err(McpError::TooLarge("tools/list"))
}
