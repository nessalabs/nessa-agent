use super::connection::Connection;
use super::process::{Launched, Launcher, ProcessLauncher, ServerProcess};
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
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, RwLock, Weak,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{broadcast, watch},
    task::JoinSet,
};

/// The budget for opening a session: launching its server and the server's
/// answer to `initialize`.
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

/// The configured MCP servers, and every session open on them.
///
/// A session is one server process and the one connection to it, opened for
/// one harness session ([`McpServers::open`]) and closed when that ends: one
/// connection per server for each harness session, held here (ADR 344). An
/// agent's calls and its app's reach the same upstream session; two
/// conversations never share one, so neither blocks, sees, or outlives the
/// other's. The states and orderings are tabled in
/// `docs/design/mcp-connections.md`, and each row has a test in
/// `tests/infrastructure/mcp/`.
///
/// Cloning shares the servers. [`McpServers::stop`] closes every session and
/// refuses new ones.
#[derive(Clone)]
pub struct McpServers {
    inner: Arc<Inner>,
}

struct Inner {
    launches: BTreeMap<String, McpServerLaunch>,
    clock: Arc<dyn Clock>,
    launcher: Arc<dyn Launcher>,
    /// Set once, by `stop`; what an opening races.
    stopping: watch::Sender<bool>,
    /// The sessions open now. A session is registered under this lock only
    /// while `stopping` is unset, and `stop` sets it and takes them under the
    /// same lock, so no session opens after a stop has looked.
    live: Mutex<Live>,
}

#[derive(Default)]
struct Live {
    next: u64,
    sessions: BTreeMap<u64, Weak<Session>>,
}

struct Session {
    id: u64,
    server: String,
    connection: Arc<Connection>,
    /// The server's answer to `initialize`, given to the harness as its own.
    initialized: Arc<Value>,
    process: Mutex<Option<ServerProcess>>,
    /// The tools as this session last listed them, with the order the list
    /// was asked in; `None` until a list has finished.
    tools: RwLock<Option<(u64, Arc<[ListedTool]>)>>,
    lists: AtomicU64,
    owner: Weak<Inner>,
}
impl Drop for Session {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            owner
                .live
                .lock()
                .expect("live sessions")
                .sessions
                .remove(&self.id);
        }
    }
}

impl McpServers {
    /// The servers in `servers`; deadlines are measured on `clock`.
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
        Ok(Self {
            inner: Arc::new(Inner {
                launches: servers
                    .into_iter()
                    .map(|launch| (launch.server.name.clone(), launch))
                    .collect(),
                clock,
                launcher,
                stopping: watch::channel(false).0,
                live: Mutex::default(),
            }),
        })
    }

    /// The configured servers' names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.inner.launches.keys().map(String::as_str)
    }

    /// Open a session on `server`: launch its process and initialize it,
    /// within [`INITIALIZE_TIMEOUT`]. Its tools are then listed in the
    /// background, and again whenever it says they changed.
    ///
    /// # Errors
    ///
    /// [`McpError::NotConfigured`], [`McpError::Stopped`], [`McpError::Start`]
    /// when the process cannot be launched, [`McpError::Handshake`] for a
    /// refused or unreadable `initialize` (an unsupported protocol version
    /// among them), [`McpError::Timeout`], and [`McpError::ServerGone`] for a
    /// server that ends during it. The process is stopped on each.
    pub async fn open(&self, server: &str) -> Result<McpSession, McpError> {
        let inner = &self.inner;
        let launch = inner.launches.get(server).ok_or(McpError::NotConfigured)?;
        if *inner.stopping.borrow() {
            return Err(McpError::Stopped);
        }
        let deadline = inner.clock.now() + INITIALIZE_TIMEOUT;
        let Launched {
            output,
            input,
            process,
        } = inner.launcher.launch(launch)?;
        let connection = Arc::new(Connection::open(output, input, inner.clock.clone()));
        let handshake = async {
            let answer = within(
                &*inner.clock,
                deadline,
                connection.call("initialize", Some(wire::initialize_params())),
            )
            .await
            .ok_or(McpError::Timeout)?
            .map_err(|error| match error {
                // Something on stdout that is not an answer: no handshake.
                McpError::Malformed(reason) => McpError::Handshake(reason),
                other => other,
            })?
            .map_err(|error| match super::connection::remote(&error) {
                McpError::Remote { code, message } => {
                    McpError::Handshake(format!("{code}: {message}"))
                }
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
                // The process group is killed as `process` is dropped.
                connection.close(error.clone());
                return Err(error);
            }
        };
        let session = {
            let mut live = inner.live.lock().expect("live sessions");
            if *inner.stopping.borrow() {
                connection.close(McpError::Stopped);
                return Err(McpError::Stopped);
            }
            live.next += 1;
            let session = Arc::new(Session {
                id: live.next,
                server: server.to_owned(),
                connection,
                initialized: Arc::new(initialized),
                process: Mutex::new(process),
                tools: RwLock::new(None),
                lists: AtomicU64::new(0),
                owner: Arc::downgrade(inner),
            });
            live.sessions.insert(session.id, Arc::downgrade(&session));
            session
        };
        // Subscribed before the first list, so a change during it is not missed.
        let notices = session.connection.notices();
        tokio::spawn(keep_listed(Arc::downgrade(&session), notices));
        Ok(McpSession { session })
    }

    /// The UI of the tool an observed call names, as the open sessions of its
    /// server last listed it ([`ListedTool::ui_for`]). `None` when no open
    /// session of that server has listed its tools yet, and when the sessions
    /// that have disagree about this tool's UI — then which one the call went
    /// through is not known here, so none is guessed, and the disagreement is
    /// logged.
    pub fn tool_ui(&self, call: &McpTool) -> Option<ToolUi> {
        let sessions: Vec<Arc<Session>> = {
            let live = self.inner.live.lock().expect("live sessions");
            live.sessions.values().filter_map(Weak::upgrade).collect()
        };
        let mut declared = sessions
            .iter()
            .filter(|session| {
                session.server == call.server() && session.connection.end_cause().is_none()
            })
            .filter_map(|session| {
                let tools = session.tools.read().expect("tool list").clone()?;
                Some(ListedTool::ui_for(&tools.1, call).cloned())
            });
        let first = declared.next()?;
        if declared.any(|other| other != first) {
            tracing::warn!(
                server = call.server(),
                tool = call.tool(),
                "open MCP sessions disagree about a tool's UI; no widget is shown for its calls"
            );
            return None;
        }
        first
    }

    /// Close every open session — each harness's stand-in ends, its server's
    /// stdin is closed, and a server still running two seconds later is
    /// killed with its process group — and refuse every later open with
    /// [`McpError::Stopped`].
    pub async fn stop(&self) {
        let sessions: Vec<Arc<Session>> = {
            let live = self.inner.live.lock().expect("live sessions");
            self.inner.stopping.send_replace(true);
            live.sessions.values().filter_map(Weak::upgrade).collect()
        };
        let mut closing = JoinSet::new();
        for session in sessions {
            closing.spawn(close(session, McpError::Stopped));
        }
        while closing.join_next().await.is_some() {}
    }
}

/// One server process and the one connection to it, for one harness session.
/// Clones share it; it is closed by [`McpSession::close`], by the end of
/// [`McpSession::serve`], by [`McpServers::stop`], or by its server ending —
/// and when the last clone is dropped, its process group is killed.
#[derive(Clone)]
pub struct McpSession {
    session: Arc<Session>,
}
impl McpSession {
    /// The configured server's name.
    pub fn server(&self) -> &str {
        &self.session.server
    }

    /// The server's process id, while it has one.
    pub fn process_id(&self) -> Option<u32> {
        self.session
            .process
            .lock()
            .expect("process")
            .as_ref()
            .and_then(ServerProcess::id)
    }

    /// List the tools now, with each tool's UI, and keep the list for
    /// [`McpServers::tool_ui`].
    ///
    /// # Errors
    ///
    /// [`McpError::Timeout`] for a page not answered within
    /// [`REQUEST_TIMEOUT`], [`McpError::TooLarge`] past [`MAX_TOOL_PAGES`] or
    /// [`MAX_TOOLS`], [`McpError::Malformed`] for a page of the wrong shape,
    /// and the session's end cause once it has ended.
    pub async fn list_tools(&self) -> Result<Vec<ListedTool>, McpError> {
        list(&self.session).await
    }

    /// Read the MCP App at `uri` from this session's server.
    ///
    /// # Errors
    ///
    /// [`McpError::NotAnApp`] for a resource that is not
    /// `text/html;profile=mcp-app`, [`McpError::TooLarge`] past the domain's
    /// bounds ([`MAX_UI_HTML_BYTES`](crate::domain::mcp_apps::MAX_UI_HTML_BYTES)
    /// and the CSP's), [`McpError::Remote`] for the server's refusal,
    /// [`McpError::Timeout`] past [`REQUEST_TIMEOUT`], and the session's end
    /// cause once it has ended.
    pub async fn read_ui_resource(&self, uri: &UiResourceUri) -> Result<UiResource, McpError> {
        let params = json!({ "uri": uri.as_str() });
        let result = self
            .session
            .connection
            .request("resources/read", Some(params), REQUEST_TIMEOUT)
            .await?;
        wire::ui_resource(uri, &result)
    }

    /// Serve a harness over `input` and `output` until it closes, its input
    /// breaks, or the server ends; then close the session. The harness's
    /// `initialize` is answered with the server's own answer; its other
    /// requests are forwarded under ids of the connection's and answered
    /// under its own; its cancellations cancel upstream; tools the model may
    /// not see are left out of its lists and refused if called; the server's
    /// `*/list_changed` notices are passed on.
    pub async fn serve(self, input: impl AsyncRead + Unpin, output: impl AsyncWrite + Unpin) {
        stand_in::serve(
            self.session.connection.clone(),
            self.session.initialized.clone(),
            input,
            output,
        )
        .await;
        close(self.session, McpError::Closed).await;
    }

    /// Close the session: calls waiting on it end with
    /// [`McpError::Closed`], the server's stdin is closed, and a server still
    /// running two seconds later is killed with its process group.
    pub async fn close(&self) {
        close(self.session.clone(), McpError::Closed).await;
    }
}

async fn close(session: Arc<Session>, cause: McpError) {
    session.connection.close(cause);
    let process = session.process.lock().expect("process").take();
    if let Some(process) = process {
        process.stop().await;
    }
}

/// List the session's tools now, and again whenever the server says they
/// changed, until it ends. A failed list leaves the tools as they were and
/// is logged.
async fn keep_listed(session: Weak<Session>, mut notices: broadcast::Receiver<Arc<Value>>) {
    let Some(ended) = session.upgrade().map(|session| session.connection.ended()) else {
        return;
    };
    tokio::pin!(ended);
    let mut changed = true;
    loop {
        if changed {
            let Some(session) = session.upgrade() else {
                return;
            };
            if let Err(error) = list(&session).await {
                tracing::warn!(server = %session.server, %error, "MCP server's tools could not be listed");
            }
        }
        let notice = tokio::select! {
            _ = &mut ended => return,
            notice = notices.recv() => notice,
        };
        changed = match notice {
            Ok(notice) => tools_changed(&notice),
            Err(broadcast::error::RecvError::Lagged(_)) => true,
            Err(broadcast::error::RecvError::Closed) => return,
        };
    }
}

/// Whether a server's notice says its tools changed. Its other lists
/// (resources, prompts) are not this client's to keep.
pub(super) fn tools_changed(notice: &Value) -> bool {
    notice.get("method").and_then(Value::as_str) == Some("notifications/tools/list_changed")
}

/// Every page of `tools/list`, kept as the session's tools when all were read
/// and no list asked later has been kept already.
async fn list(session: &Session) -> Result<Vec<ListedTool>, McpError> {
    let order = session.lists.fetch_add(1, Ordering::Relaxed);
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_TOOL_PAGES {
        let params = cursor.map(|cursor| json!({ "cursor": cursor }));
        let result = session
            .connection
            .request("tools/list", params, REQUEST_TIMEOUT)
            .await?;
        let (page, next) = wire::tools_page(&session.server, &result)?;
        if tools.len() + page.len() > MAX_TOOLS {
            return Err(McpError::TooLarge("tools/list"));
        }
        tools.extend(page);
        match next {
            Some(next) => cursor = Some(next),
            None => {
                let mut kept = session.tools.write().expect("tool list");
                if kept.as_ref().is_none_or(|(kept, _)| *kept < order) {
                    *kept = Some((order, Arc::from(tools.clone())));
                }
                return Ok(tools);
            }
        }
    }
    Err(McpError::TooLarge("tools/list"))
}
