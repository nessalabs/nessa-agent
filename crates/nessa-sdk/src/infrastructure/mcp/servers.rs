use super::connection::Connection;
use super::process::{Launched, Launcher, ProcessLauncher, ServerProcess};
use super::stand_in::{self, Visibility};
use super::{wire, McpError};
use crate::application::agent_execution::caller_wake::contain_caller_wake;
use crate::domain::agent_execution::sessions::SessionId;
use crate::domain::agent_execution::tools::McpTool;
use crate::domain::mcp_apps::{ListedTool, ToolUi, UiResource, UiResourceUri};
use crate::infrastructure::acp::sessions::{ForwardedResults, StandInGrant, StdioMcpServer};
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
/// one harness session ([`McpServers::open`]) and closed when that ends, or
/// when the grant it was opened under is revoked ([`McpServers::revoke`]):
/// one connection per server for each harness session, held here (ADR 344).
/// An agent's calls and its app's reach the same upstream session; two
/// openings never share one
/// (`each_opening_is_a_server_process_and_a_session_of_its_own`), so neither
/// blocks, sees, or outlives the other's. Each is owned by the SDK session
/// (a conversation's) it was opened for ([`McpOwner`]). The states and orderings are tabled in
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

/// Whose a session is: the SDK session (a conversation's) its harness was
/// opened for, under one grant of the host's for that open. Each
/// [`McpOwner::new`] is a grant of its own, shared by its clones; sessions of
/// one grant are closed together when the host revokes it
/// ([`McpServers::revoke`]), and none opens under it after.
#[derive(Clone)]
pub struct McpOwner {
    session: SessionId,
    grant: Arc<Grant>,
}
/// One grant. Whether it is revoked and which sessions were opened under it
/// share one lock, so registering a session (check, then add) and revoking
/// (set, then take) are each one step: a session either is registered
/// before the revocation, and is taken by it, or sees it and is refused —
/// whichever [`McpServers`] it is opened and revoked through.
struct Grant {
    state: Mutex<GrantState>,
    /// What its sessions' stand-ins forwarded, until the binding holding the
    /// grant takes each for the tool call it was reported under.
    forwarded: ForwardedResults,
}
impl Default for Grant {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            forwarded: ForwardedResults::new(),
        }
    }
}
#[derive(Default)]
struct GrantState {
    revoked: bool,
    /// The sessions opened under it, so revoking it visits only its own.
    /// Each registration first drops those already ended, so between
    /// registrations it holds the open ones and those ended since the last.
    sessions: Vec<Weak<Session>>,
}
impl McpOwner {
    /// A new grant for sessions opened for `session`.
    pub fn new(session: SessionId) -> Self {
        Self {
            session,
            grant: Arc::default(),
        }
    }
    /// The SDK session these sessions belong to.
    pub fn session(&self) -> &SessionId {
        &self.session
    }
    /// The results this grant's stand-ins forwarded to their harness, shared,
    /// for comparing: only the SDK writes and takes them.
    pub fn forwarded(&self) -> ForwardedResults {
        self.grant.forwarded.clone()
    }
    /// The grant a host gives the binding for this owner's open: its stand-ins'
    /// `environment`, `held` until the grant is dropped (the host's revocation
    /// of this owner), and this owner's forwarded results — its own, so a
    /// grant cannot carry another owner's.
    pub fn stand_in_grant(
        &self,
        environment: Vec<(String, String)>,
        held: Box<dyn Send + Sync>,
    ) -> StandInGrant {
        StandInGrant::new(environment, held).with_forwarded(self.forwarded())
    }
    /// Whether `other` is this same grant.
    fn same_grant(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.grant, &other.grant)
    }
    fn revoked(&self) -> bool {
        self.grant.state.lock().expect("grant").revoked
    }
    /// How many sessions the grant holds now, ended ones not yet dropped
    /// among them.
    #[cfg(all(test, unix))]
    pub(super) fn held(&self) -> usize {
        self.grant.state.lock().expect("grant").sessions.len()
    }
}
impl PartialEq for McpOwner {
    fn eq(&self, other: &Self) -> bool {
        self.session == other.session && self.same_grant(other)
    }
}
impl Eq for McpOwner {}
impl std::fmt::Debug for McpOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpOwner")
            .field("session", &self.session)
            .field("revoked", &self.revoked())
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct Live {
    next: u64,
    sessions: BTreeMap<u64, Weak<Session>>,
}

struct Session {
    id: u64,
    server: String,
    owned_by: McpOwner,
    connection: Arc<Connection>,
    /// The server's answer to `initialize`, given to the harness as its own.
    initialized: Arc<Value>,
    process: Mutex<Option<ServerProcess>>,
    /// Set once the process taken from `process` has been stopped — or
    /// dropped, which kills its group — so a close that found it already
    /// taken waits for that, not for nothing.
    stopped: watch::Sender<bool>,
    /// The tools as this session last listed them; `None` until a list has
    /// finished.
    tools: RwLock<Option<Arc<Listed>>>,
    /// Which tools the model may not see, by every list of the session's —
    /// its own and its stand-in's — and the numbering of those lists.
    visibility: Arc<Visibility>,
    owner: Weak<Inner>,
}

/// One finished list of a session's tools.
struct Listed {
    /// The order the list was asked in, among the session's lists.
    order: u64,
    tools: Arc<[ListedTool]>,
}

impl Session {
    /// Close the connection — calls waiting end with `cause`, the server's
    /// stdin closes — and hand back the process to stop, if it is still held.
    fn close_now(&self, cause: McpError) -> Option<ServerProcess> {
        self.connection.close(cause);
        self.process.lock().expect("process").take()
    }
    /// Close the connection and kill the process group at once, without the
    /// grace: dropping the process kills its group. Said stopped only when it
    /// took the process; a close already stopping it says so itself, once
    /// it has.
    fn kill_now(&self, cause: McpError) {
        if let Some(process) = self.close_now(cause) {
            drop(process);
            self.stopped.send_replace(true);
        }
    }
}

/// What every [`McpSession`] clone holds. When the last one goes, the session
/// is closed and its process group killed at once — whoever else (its
/// background list) still holds the session itself.
struct Owner(Arc<Session>);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.kill_now(McpError::Closed);
    }
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

    /// Open a session on `server` for `owner`: launch its process and initialize it,
    /// within [`INITIALIZE_TIMEOUT`]. Its tools are then listed in the
    /// background, and again whenever it says they changed.
    ///
    /// # Errors
    ///
    /// [`McpError::NotConfigured`], [`McpError::Stopped`], [`McpError::Start`]
    /// when the process cannot be launched, [`McpError::Handshake`] for a
    /// refused or unreadable `initialize` (an unsupported protocol version
    /// among them), [`McpError::Timeout`], [`McpError::ServerGone`] for a
    /// server that ends during it, and [`McpError::Closed`] when `owner`'s
    /// grant is revoked — before it launches anything, or while it opens. The
    /// process is stopped on each. Nothing else makes it [`McpError::Closed`].
    pub async fn open(&self, server: &str, owner: McpOwner) -> Result<McpSession, McpError> {
        contain_caller_wake(
            format!("MCP open of {server}"),
            self.open_session(server, owner),
        )
        .await
    }

    async fn open_session(&self, server: &str, owner: McpOwner) -> Result<McpSession, McpError> {
        let inner = &self.inner;
        let launch = inner.launches.get(server).ok_or(McpError::NotConfigured)?;
        if *inner.stopping.borrow() {
            return Err(McpError::Stopped);
        }
        // Revoked already: nothing to launch. Revoked from here on is seen
        // when the session is registered, below.
        if owner.revoked() {
            return Err(McpError::Closed);
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
            // Revoked while opening: checked, and the session added to the
            // grant, under the grant's own lock, which `revoke` sets and takes
            // under — so either it takes this session or this sees it.
            let grant = owner.grant.clone();
            let mut granted = grant.state.lock().expect("grant");
            if granted.revoked {
                connection.close(McpError::Closed);
                return Err(McpError::Closed);
            }
            live.next += 1;
            let session = Arc::new(Session {
                id: live.next,
                server: server.to_owned(),
                owned_by: owner,
                connection,
                initialized: Arc::new(initialized),
                // A server with no process of its own is stopped already.
                stopped: watch::channel(process.is_none()).0,
                process: Mutex::new(process),
                tools: RwLock::new(None),
                visibility: Arc::default(),
                owner: Arc::downgrade(inner),
            });
            live.sessions.insert(session.id, Arc::downgrade(&session));
            granted.sessions.retain(|each| each.strong_count() > 0);
            granted.sessions.push(Arc::downgrade(&session));
            drop(granted);
            session
        };
        // Subscribed before the first list, so a change during it is not missed.
        let notices = session.connection.notices();
        tokio::spawn(keep_listed(Arc::downgrade(&session), notices));
        Ok(McpSession {
            owner: Arc::new(Owner(session)),
        })
    }

    /// What the tool an observed call names declared in `_meta.ui` — its UI
    /// resource, when it has one, and its visibility — as `session`'s own
    /// session of its server last listed it ([`ListedTool::ui_for`]): the one
    /// of its sessions of that server registered last and still open. An SDK
    /// session holds one provider attachment at a time, so that is the one
    /// its harness talks to now; while a resumed open and the one it replaces
    /// briefly overlap, it is the resumed one's once that has said hello.
    /// `None` when it has none open, when that one has not listed its tools
    /// yet, or when its list names no one tool for the call.
    pub fn tool_ui(&self, session: &SessionId, call: &McpTool) -> Option<ToolUi> {
        let own = self.newest(session, call.server())?;
        let listed = own.tools.read().expect("tool list").clone()?;
        ListedTool::ui_for(&listed.tools, call).cloned()
    }

    /// The tool `name` exactly as `session`'s own newest open session of
    /// `server` last listed it, or `None` when that list does not have it —
    /// or there is no list yet, which an app cannot be acting on.
    ///
    /// # Errors
    ///
    /// [`McpError::NoSession`] when `session` has no open session of
    /// `server`.
    pub fn listed_tool(
        &self,
        session: &SessionId,
        server: &str,
        name: &str,
    ) -> Result<Option<ListedTool>, McpError> {
        let own = self.newest(session, server).ok_or(McpError::NoSession)?;
        let listed = own.tools.read().expect("tool list").clone();
        Ok(listed.and_then(|listed| {
            listed
                .tools
                .iter()
                .find(|each| each.tool().tool() == name)
                .cloned()
        }))
    }

    /// Call the tool `name` with `arguments` over `session`'s own newest open
    /// session of `server`, within `timeout`: an MCP App's call, on the
    /// connection its agent's calls use. `timeout` is the caller's policy,
    /// measured on the clock these servers were made with from this call, and
    /// covers sending the request as well as its answer; one not answered by
    /// then — a zero `timeout` included — is cancelled upstream. The answer is the server's
    /// `CallToolResult` as it gave it (`isError` included); which tools an
    /// app may call is the caller's to decide.
    ///
    /// # Errors
    ///
    /// [`McpError::NoSession`], [`McpError::Timeout`] (the call is cancelled
    /// upstream), [`McpError::Remote`] for the server's JSON-RPC error,
    /// [`McpError::Malformed`] for an answer that is not an object,
    /// [`McpError::Busy`], and the session's end cause once it has ended.
    pub async fn call_tool(
        &self,
        session: &SessionId,
        server: &str,
        name: &str,
        arguments: Option<Value>,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        contain_caller_wake(
            format!("MCP tool call on {server}"),
            self.call_tool_on_session(session, server, name, arguments, timeout),
        )
        .await
    }

    async fn call_tool_on_session(
        &self,
        session: &SessionId,
        server: &str,
        name: &str,
        arguments: Option<Value>,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        let own = self.newest(session, server).ok_or(McpError::NoSession)?;
        let mut params = json!({ "name": name });
        if let Some(arguments) = arguments {
            params["arguments"] = arguments;
        }
        let result = own
            .connection
            .request("tools/call", Some(params), timeout)
            .await?;
        if !result.is_object() {
            return Err(McpError::Malformed(
                "a tools/call result that is not an object".into(),
            ));
        }
        Ok(result)
    }

    /// Read the MCP App resource `uri` over `session`'s own newest open
    /// session of `server`, as [`McpSession::read_ui_resource`] does, within
    /// `timeout`: the caller's policy, measured as for [`Self::call_tool`].
    ///
    /// # Errors
    ///
    /// [`McpError::NoSession`], [`McpError::Timeout`] past `timeout`, and what
    /// [`McpSession::read_ui_resource`] fails with.
    pub async fn read_app_resource(
        &self,
        session: &SessionId,
        server: &str,
        uri: &UiResourceUri,
        timeout: Duration,
    ) -> Result<UiResource, McpError> {
        contain_caller_wake(
            format!("MCP app resource read on {server}"),
            self.read_app_resource_on_session(session, server, uri, timeout),
        )
        .await
    }

    async fn read_app_resource_on_session(
        &self,
        session: &SessionId,
        server: &str,
        uri: &UiResourceUri,
        timeout: Duration,
    ) -> Result<UiResource, McpError> {
        let own = self.newest(session, server).ok_or(McpError::NoSession)?;
        let params = json!({ "uri": uri.as_str() });
        let result = own
            .connection
            .request("resources/read", Some(params), timeout)
            .await?;
        wire::ui_resource(uri, &result)
    }

    /// `session`'s newest open session of `server`, if it has one.
    fn newest(&self, session: &SessionId, server: &str) -> Option<Arc<Session>> {
        self.open_sessions().into_iter().rev().find(|each| {
            each.owned_by.session() == session
                && each.server == server
                && each.connection.end_cause().is_none()
        })
    }

    /// The sessions open now, oldest first. Upgraded under the lock and
    /// handed out of it: a session whose last other reference goes meanwhile
    /// is dropped by the caller, after the lock is released — its `Drop`
    /// takes the lock itself.
    fn open_sessions(&self) -> Vec<Arc<Session>> {
        let live = self.inner.live.lock().expect("live sessions");
        live.sessions.values().filter_map(Weak::upgrade).collect()
    }

    /// Revoke `owner`'s grant: no session opens under it from now on — one
    /// opening now is refused [`McpError::Closed`] — and each open one is
    /// closed as its stand-in ending would close it: calls waiting end
    /// [`McpError::Closed`] at once, the server's stdin is closed, and a
    /// server still running two seconds later is killed with its process
    /// group. Returns at once: the connections are closed before it does,
    /// and stopping the processes runs on the current runtime. Without one,
    /// the process groups are killed at once, without the grace. On a runtime
    /// shutting down, which drops what is spawned on it, each is stopped by
    /// whatever else closes it — its stand-in's end, with the grace, or its
    /// last handle going, at once.
    pub fn revoke(&self, owner: &McpOwner) {
        // Its own sessions only, taken with the flag set in one step under
        // the grant's lock (see `Grant`), and upgraded — and dropped — after
        // it is released: see `open_sessions`.
        let granted = {
            let mut state = owner.grant.state.lock().expect("grant");
            state.revoked = true;
            std::mem::take(&mut state.sessions)
        };
        for session in granted.iter().filter_map(Weak::upgrade) {
            session.connection.close(McpError::Closed);
            match tokio::runtime::Handle::try_current() {
                Ok(runtime) => {
                    runtime.spawn(close(session, McpError::Closed));
                }
                Err(_) => session.kill_now(McpError::Closed),
            }
        }
    }

    /// Close every open session — each harness's stand-in ends, its server's
    /// stdin is closed, and a server still running two seconds later is
    /// killed with its process group — and refuse every later open with
    /// [`McpError::Stopped`].
    pub async fn stop(&self) {
        contain_caller_wake("MCP stop", self.stop_sessions()).await
    }

    async fn stop_sessions(&self) {
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
/// [`McpSession::serve`], by [`McpServers::stop`], by [`McpServers::revoke`]
/// of its grant, or by its server ending — and when the last clone is
/// dropped, it is closed and its process group killed at once.
#[derive(Clone)]
pub struct McpSession {
    owner: Arc<Owner>,
}
impl McpSession {
    /// The configured server's name.
    pub fn server(&self) -> &str {
        &self.owner.0.server
    }

    /// The server's process id, while it has one.
    pub fn process_id(&self) -> Option<u32> {
        self.owner
            .0
            .process
            .lock()
            .expect("process")
            .as_ref()
            .and_then(ServerProcess::id)
    }

    /// List the tools now, with what each declared in `_meta.ui`, and keep
    /// the list for
    /// [`McpServers::tool_ui`].
    ///
    /// # Errors
    ///
    /// [`McpError::Timeout`] for a page not answered within
    /// [`REQUEST_TIMEOUT`], [`McpError::TooLarge`] past [`MAX_TOOL_PAGES`] or
    /// [`MAX_TOOLS`], [`McpError::Malformed`] for a page of the wrong shape,
    /// and the session's end cause once it has ended.
    pub async fn list_tools(&self) -> Result<Vec<ListedTool>, McpError> {
        contain_caller_wake(
            format!("MCP tool list of {}", self.server()),
            list(&self.owner.0),
        )
        .await
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
        contain_caller_wake(
            format!("MCP UI resource read of {}", self.server()),
            self.read_ui_resource_now(uri),
        )
        .await
    }

    async fn read_ui_resource_now(&self, uri: &UiResourceUri) -> Result<UiResource, McpError> {
        let params = json!({ "uri": uri.as_str() });
        let result = self
            .owner
            .0
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
    /// `*/list_changed` notices are passed on. A `tools/call` result's
    /// `structuredContent` is kept in the grant's [`McpOwner::forwarded`]
    /// under the harness's id for the call, with this server's name, before
    /// the harness is answered.
    pub async fn serve(self, input: impl AsyncRead + Unpin, output: impl AsyncWrite + Unpin) {
        let server = self.server().to_owned();
        contain_caller_wake(format!("MCP serve of {server}"), async move {
            stand_in::serve(
                self.owner.0.connection.clone(),
                self.owner.0.initialized.clone(),
                self.owner.0.visibility.clone(),
                self.owner.0.owned_by.forwarded(),
                &self.owner.0.server,
                input,
                output,
            )
            .await;
            close(self.owner.0.clone(), McpError::Closed).await;
        })
        .await;
    }

    /// Close the session: calls waiting on it end with
    /// [`McpError::Closed`], the server's stdin is closed, and a server still
    /// running two seconds later is killed with its process group. Returns
    /// once the server is stopped, whichever close — this one, another
    /// clone's, or [`McpServers::stop`] — is stopping it.
    pub async fn close(&self) {
        contain_caller_wake(
            format!("MCP close of {}", self.server()),
            close(self.owner.0.clone(), McpError::Closed),
        )
        .await;
    }
}

/// Close `session` and return once its server is stopped. The close that
/// takes the process stops it; any other waits for that one to finish.
async fn close(session: Arc<Session>, cause: McpError) {
    let mut stopped = session.stopped.subscribe();
    if let Some(process) = session.close_now(cause) {
        // Said stopped however this ends: finished, or cancelled with the
        // process dropped, which kills its group.
        let _said = Stopped(&session.stopped);
        process.stop().await;
        return;
    }
    // The session holds the sender, so this ends.
    let _ = stopped.wait_for(|stopped| *stopped).await;
}

/// Says a session's server is stopped when dropped.
struct Stopped<'a>(&'a watch::Sender<bool>);
impl Drop for Stopped<'_> {
    fn drop(&mut self) {
        self.0.send_replace(true);
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
    let order = session.visibility.ask();
    let mut tools = Vec::new();
    let mut hidden = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_TOOL_PAGES {
        let params = cursor.map(|cursor| json!({ "cursor": cursor }));
        let result = session
            .connection
            .request("tools/list", params, REQUEST_TIMEOUT)
            .await?;
        let page = wire::tools_page(&session.server, &result)?;
        if tools.len() + page.tools.len() > MAX_TOOLS
            || hidden.len() + page.hidden.len() > MAX_TOOLS
        {
            return Err(McpError::TooLarge("tools/list"));
        }
        tools.extend(page.tools);
        hidden.extend(page.hidden);
        match page.next {
            Some(next) => cursor = Some(next),
            None => {
                session.visibility.listed(order, hidden);
                let mut kept = session.tools.write().expect("tool list");
                if kept.as_ref().is_none_or(|kept| kept.order < order) {
                    *kept = Some(Arc::new(Listed {
                        order,
                        tools: Arc::from(tools.clone()),
                    }));
                }
                return Ok(tools);
            }
        }
    }
    Err(McpError::TooLarge("tools/list"))
}
