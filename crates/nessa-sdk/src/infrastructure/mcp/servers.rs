use super::authorization::{NoAuthorization, RemoteAuthorization};
use super::connection::Connection;
use super::http::{HttpSession, SessionClaims};
use super::http_exchange::HttpExchange;
use super::process::{Launched, Launcher, ProcessLauncher, ServerProcess};
use super::remote::RemoteMcpServer;
use super::stand_in::{self, Visibility};
use super::{wire, McpError};
use crate::application::agent_execution::caller_wake::contain_caller_wake;
use crate::domain::agent_execution::sessions::SessionId;
use crate::domain::agent_execution::tools::McpTool;
use crate::domain::mcp_apps::{ListedTool, ToolUi, UiResource, UiResourceUri, UiVisibility};
use crate::infrastructure::acp::sessions::{
    ForwardedResults, McpServerProblem, StandInGrant, StdioMcpServer, MAX_MCP_SERVERS,
};
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

/// The environment variable a host's MCP stand-ins carry their session
/// token in. A server process is never given it
/// ([`McpServerProblem::ReservedEnvironmentName`]), so a server cannot read a
/// token meant for the gateway, nor pass one on.
pub const MCP_SESSION_VARIABLE: &str = "NESSA_MCP_SESSION";

/// The most bytes in the name of a variable a server is started with
/// ([`McpServerLaunch::problem`]).
pub const MAX_MCP_ENVIRONMENT_NAME_BYTES: usize = 256;

/// One configured stdio server and what it is started with.
///
/// Its `Debug` names the environment's variables and never prints their
/// values, which may be credentials
/// (`a_launch_prints_its_environment_names_never_its_values`). Two launches
/// are equal when every field is: what [`McpServers::open_as`] compares.
#[derive(Clone, PartialEq, Eq)]
pub struct McpServerLaunch {
    /// The trusted configuration: name, absolute executable, arguments.
    pub server: StdioMcpServer,
    /// The server's working directory.
    pub working_directory: PathBuf,
    /// The server's whole environment; nothing is inherited.
    pub environment: BTreeMap<OsString, OsString>,
}
impl std::fmt::Debug for McpServerLaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServerLaunch")
            .field("server", &self.server)
            .field("working_directory", &self.working_directory)
            .field("environment", &self.environment.keys().collect::<Vec<_>>())
            .finish()
    }
}
impl McpServerLaunch {
    /// Why this server cannot be started as configured, or `None` when it
    /// can: its [`StdioMcpServer::problem`], then its environment — each
    /// name 1 to [`MAX_MCP_ENVIRONMENT_NAME_BYTES`] bytes of ASCII letters,
    /// digits and `_`, not starting with a
    /// digit, and not [`MCP_SESSION_VARIABLE`]; each value without NUL.
    pub fn problem(&self) -> Option<McpServerProblem> {
        self.server.problem().or_else(|| {
            self.environment.iter().find_map(|(name, value)| {
                let Some(name) = name.to_str().filter(|name| {
                    (1..=MAX_MCP_ENVIRONMENT_NAME_BYTES).contains(&name.len())
                        && !name.starts_with(|c: char| c.is_ascii_digit())
                        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                }) else {
                    return Some(McpServerProblem::EnvironmentName {
                        server: self.server.name.clone(),
                        name: name.to_string_lossy().into_owned(),
                    });
                };
                if name == MCP_SESSION_VARIABLE {
                    return Some(McpServerProblem::ReservedEnvironmentName {
                        server: self.server.name.clone(),
                        name: name.to_owned(),
                    });
                }
                value
                    .as_encoded_bytes()
                    .contains(&0)
                    .then(|| McpServerProblem::EnvironmentValue {
                        server: self.server.name.clone(),
                        name: name.to_owned(),
                    })
            })
        })
    }
    /// Why `launches` cannot run together, or `None` when they can: the
    /// set's rules ([`StdioMcpServer::problem_in`]), then each launch's
    /// [`Self::problem`]. What [`McpServers::new`] and [`McpServers::replace`]
    /// ask.
    pub fn problem_in(launches: &[McpServerLaunch]) -> Option<McpServerProblem> {
        StdioMcpServer::problem_in(launches.iter().map(|launch| &launch.server))
            .or_else(|| launches.iter().find_map(Self::problem))
    }
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
/// The configured set is live: [`McpServers::replace`] swaps it, and each
/// opening reads the set as it is then. A session already open keeps the
/// server it was opened on until it ends; replacing the set closes nothing.
///
/// Cloning shares the servers. [`McpServers::stop`] closes every session and
/// refuses new ones, and every later replacement.
#[derive(Clone)]
pub struct McpServers {
    /// Visible to this module's tests, which hold `live` as `stop` does.
    pub(super) inner: Arc<Inner>,
}

pub(super) struct Inner {
    /// The configured set now, by name. Swapped whole by `replace`, under
    /// `live`'s lock, so a replacement and a stop are ordered.
    launches: RwLock<Arc<BTreeMap<String, McpServerLaunch>>>,
    remotes: RwLock<Arc<BTreeMap<String, RemoteMcpServer>>>,
    http: Mutex<Option<Arc<dyn HttpExchange>>>,
    authorization: Mutex<Arc<dyn RemoteAuthorization>>,
    claims: Arc<SessionClaims>,
    clock: Arc<dyn Clock>,
    launcher: Arc<dyn Launcher>,
    /// Set once, by `stop`; what an opening races.
    pub(super) stopping: watch::Sender<bool>,
    /// The sessions open now. A session is registered under this lock only
    /// while `stopping` is unset, and `stop` sets it and takes them under the
    /// same lock, so no session opens after a stop has looked.
    pub(super) live: Mutex<Live>,
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
pub(super) struct Live {
    next: u64,
    sessions: BTreeMap<u64, Weak<Session>>,
}

struct Session {
    id: u64,
    server: String,
    /// The SDK session and grant it was opened for; `None` for a session
    /// opened once ([`McpServers::open_once`]), which belongs to none.
    owned_by: Option<McpOwner>,
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
    /// [`McpError::InvalidConfiguration`] with the first problem
    /// [`McpServerLaunch::problem_in`] finds.
    pub fn new(servers: Vec<McpServerLaunch>, clock: Arc<dyn Clock>) -> Result<Self, McpError> {
        Self::with_launcher(servers, clock, Arc::new(ProcessLauncher))
    }

    pub(crate) fn with_launcher(
        servers: Vec<McpServerLaunch>,
        clock: Arc<dyn Clock>,
        launcher: Arc<dyn Launcher>,
    ) -> Result<Self, McpError> {
        Ok(Self {
            inner: Arc::new(Inner {
                launches: RwLock::new(Arc::new(by_name(servers)?)),
                remotes: RwLock::new(Arc::new(BTreeMap::new())),
                http: Mutex::new(None),
                authorization: Mutex::new(Arc::new(NoAuthorization) as Arc<dyn RemoteAuthorization>),
                claims: Arc::new(SessionClaims::default()),
                clock,
                launcher,
                stopping: watch::channel(false).0,
                live: Mutex::default(),
            }),
        })
    }

    /// The configured servers now, each as it is launched, in name order.
    /// What a stand-in is admitted against and what a provider open is given
    /// are read from here, each time.
    pub fn configured(&self) -> Vec<McpServerLaunch> {
        self.launches().values().cloned().collect()
    }

    /// Replace the configured set with `servers`. Openings from now on read
    /// the new set; sessions already open, and the harnesses using them, keep
    /// what they were opened on. Nothing is launched or closed.
    ///
    /// # Errors
    ///
    /// [`McpError::InvalidConfiguration`] with the first problem
    /// [`McpServerLaunch::problem_in`] finds, and [`McpError::Stopped`] once
    /// [`McpServers::stop`] has begun; the set is unchanged on both.
    ///
    /// # Panics
    ///
    /// When another thread panicked while holding the live sessions' lock or
    /// the configured set's, which leaves them poisoned.
    pub fn replace(&self, servers: Vec<McpServerLaunch>) -> Result<(), McpError> {
        let launches = Arc::new(by_name(servers)?);
        // Under the lock `stop` sets `stopping` under: a replacement either
        // lands before the stop looks, or sees it.
        let _live = self.inner.live.lock().expect("live sessions");
        if *self.inner.stopping.borrow() {
            return Err(McpError::Stopped);
        }
        *self.inner.launches.write().expect("configured servers") = launches;
        Ok(())
    }

    /// The exchange and authorization owner later remote openings use.
    /// Replacing them does not retarget a session already open.
    pub fn set_remote_transport(
        &self,
        http: Arc<dyn HttpExchange>,
        authorization: Arc<dyn RemoteAuthorization>,
    ) {
        *self.inner.http.lock().expect("http exchange") = Some(http);
        self.set_authorization(authorization);
    }

    /// The authorization owner later remote openings ask. Replacing it does
    /// not retarget a session already open, and does not replace the HTTP
    /// exchange.
    pub fn set_authorization(&self, authorization: Arc<dyn RemoteAuthorization>) {
        *self.inner.authorization.lock().expect("authorization") = authorization;
    }

    /// Replace stdio launches and remote servers together. Openings from now
    /// on read the new set; sessions already open keep what they were opened
    /// on.
    ///
    /// # Errors
    ///
    /// [`McpError::InvalidConfiguration`] when the combined set breaks a rule,
    /// and [`McpError::Stopped`] once [`McpServers::stop`] has begun.
    pub fn replace_all(
        &self,
        servers: Vec<McpServerLaunch>,
        remotes: Vec<RemoteMcpServer>,
    ) -> Result<(), McpError> {
        if let Some(problem) = configuration_problem(&servers, &remotes) {
            return Err(McpError::InvalidConfiguration(problem));
        }
        let launches = servers
            .into_iter()
            .map(|launch| (launch.server.name.clone(), launch))
            .collect();
        let remotes = remotes
            .into_iter()
            .map(|remote| (remote.name().to_owned(), remote))
            .collect();
        let _live = self.inner.live.lock().expect("live sessions");
        if *self.inner.stopping.borrow() {
            return Err(McpError::Stopped);
        }
        *self.inner.launches.write().expect("configured servers") = Arc::new(launches);
        *self.inner.remotes.write().expect("configured servers") = Arc::new(remotes);
        Ok(())
    }

    /// The remote servers configured now, in name order.
    pub fn configured_remotes(&self) -> Vec<RemoteMcpServer> {
        self.inner
            .remotes
            .read()
            .expect("configured servers")
            .values()
            .cloned()
            .collect()
    }

    fn launches(&self) -> Arc<BTreeMap<String, McpServerLaunch>> {
        self.inner
            .launches
            .read()
            .expect("configured servers")
            .clone()
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
        let launches = self.launches();
        if let Some(launch) = launches.get(server) {
            return self.open_launch(launch, Some(owner)).await;
        }
        let remote = self
            .configured_remotes()
            .into_iter()
            .find(|remote| remote.name() == server)
            .ok_or(McpError::NotConfigured)?;
        self.open_remote(remote, Some(owner)).await
    }

    /// Open a session, as [`Self::open`] does, on the server named
    /// `admitted.server.name` only while it is still configured exactly as
    /// `admitted` — executable, arguments, working directory and
    /// environment: what a host admitted a stand-in against is what serves
    /// it, even when the set is replaced between the admission and the
    /// opening.
    ///
    /// # Errors
    ///
    /// [`McpError::ConfigurationChanged`] when the server under that name is
    /// configured differently now, and every error of [`Self::open`].
    pub async fn open_as(
        &self,
        admitted: &McpServerLaunch,
        owner: McpOwner,
    ) -> Result<McpSession, McpError> {
        let launches = self.launches();
        let launch = launches
            .get(&admitted.server.name)
            .ok_or(McpError::NotConfigured)?;
        if launch != admitted {
            return Err(McpError::ConfigurationChanged);
        }
        self.open_launch(launch, Some(owner)).await
    }

    /// Open a session on `launch` once, for no SDK session: a host's look at
    /// a server — whether it starts, and what it lists — outside any
    /// conversation, configured in the live set or not. Launched and
    /// initialized as [`Self::open`] does, within [`INITIALIZE_TIMEOUT`], but
    /// its tools are listed only when asked ([`McpSession::list_tool_pages`])
    /// and never kept for [`Self::tool_ui`], and it serves no stand-in.
    ///
    /// It is closed as any session is — [`McpSession::close`], its last clone
    /// dropped (its process group killed at once), or [`Self::stop`], which
    /// it is registered for, so a gateway stopping ends it too.
    ///
    /// # Errors
    ///
    /// [`McpError::InvalidConfiguration`] with `launch`'s
    /// [`McpServerLaunch::problem`], before anything is launched;
    /// [`McpError::Stopped`], [`McpError::Start`], [`McpError::Handshake`],
    /// [`McpError::Timeout`] and [`McpError::ServerGone`] as for
    /// [`Self::open`]. The process is stopped on each.
    pub async fn open_once(&self, launch: &McpServerLaunch) -> Result<McpSession, McpError> {
        if let Some(problem) = launch.problem() {
            return Err(McpError::InvalidConfiguration(problem));
        }
        self.open_launch(launch, None).await
    }

    /// Open a remote session once, outside a conversation.
    ///
    /// # Errors
    ///
    /// [`McpError::Unreachable`] when no HTTP exchange was installed,
    /// [`McpError::Stopped`], [`McpError::Closed`], and the handshake errors
    /// of [`Self::open`].
    pub async fn open_remote_once(&self, remote: &RemoteMcpServer) -> Result<McpSession, McpError> {
        self.open_remote(remote.clone(), None).await
    }

    /// Open `remote` only while that id and URL are still configured under
    /// its name.
    ///
    /// # Errors
    ///
    /// [`McpError::ConfigurationChanged`] when the id or URL differs,
    /// [`McpError::NotConfigured`] when the name is gone, and the errors of
    /// [`Self::open`].
    pub async fn open_remote_as(
        &self,
        admitted: &RemoteMcpServer,
        owner: McpOwner,
    ) -> Result<McpSession, McpError> {
        let current = self
            .configured_remotes()
            .into_iter()
            .find(|remote| remote.name() == admitted.name())
            .ok_or(McpError::NotConfigured)?;
        if current.id() != admitted.id() || current.url() != admitted.url() {
            return Err(McpError::ConfigurationChanged);
        }
        self.open_remote(current, Some(owner)).await
    }

    async fn open_remote(
        &self,
        remote: RemoteMcpServer,
        owner: Option<McpOwner>,
    ) -> Result<McpSession, McpError> {
        contain_caller_wake(
            format!("MCP open of {}", remote.name()),
            self.open_remote_session(remote, owner),
        )
        .await
    }

    async fn open_remote_session(
        &self,
        remote: RemoteMcpServer,
        owner: Option<McpOwner>,
    ) -> Result<McpSession, McpError> {
        let inner = &self.inner;
        if *inner.stopping.borrow() {
            return Err(McpError::Stopped);
        }
        if owner.as_ref().is_some_and(McpOwner::revoked) {
            return Err(McpError::Closed);
        }
        let http = inner
            .http
            .lock()
            .expect("http exchange")
            .clone()
            .ok_or(McpError::Unreachable)?;
        let authorization = inner.authorization.lock().expect("authorization").clone();
        let (session, incoming) = HttpSession::open(
            remote.id(),
            remote.url().clone(),
            http,
            authorization,
            inner.claims.clone(),
        );
        let connection = Arc::new(Connection::open_http(
            session,
            incoming,
            inner.clock.clone(),
        ));
        let deadline = inner.clock.now() + INITIALIZE_TIMEOUT;
        let server = remote.name().to_owned();
        self.finish_open(connection, None, server, owner, deadline)
            .await
    }

    /// Open a session on `launch`, for `owner`'s grant — or, with no owner,
    /// once ([`Self::open_once`]): then no grant holds it and no background
    /// list keeps its tools.
    async fn open_launch(
        &self,
        launch: &McpServerLaunch,
        owner: Option<McpOwner>,
    ) -> Result<McpSession, McpError> {
        contain_caller_wake(
            format!("MCP open of {}", launch.server.name),
            self.open_session(launch, owner),
        )
        .await
    }

    async fn open_session(
        &self,
        launch: &McpServerLaunch,
        owner: Option<McpOwner>,
    ) -> Result<McpSession, McpError> {
        let inner = &self.inner;
        let server = launch.server.name.as_str();
        if *inner.stopping.borrow() {
            return Err(McpError::Stopped);
        }
        // Revoked already: nothing to launch. Revoked from here on is seen
        // when the session is registered, below.
        if owner.as_ref().is_some_and(McpOwner::revoked) {
            return Err(McpError::Closed);
        }
        let deadline = inner.clock.now() + INITIALIZE_TIMEOUT;
        let Launched {
            output,
            input,
            process,
        } = inner.launcher.launch(launch)?;
        let connection = Arc::new(Connection::open(output, input, inner.clock.clone()));
        self.finish_open(connection, process, server.to_owned(), owner, deadline)
            .await
    }

    async fn finish_open(
        &self,
        connection: Arc<Connection>,
        process: Option<ServerProcess>,
        server: String,
        owner: Option<McpOwner>,
        deadline: crate::infrastructure::clock::ClockInstant,
    ) -> Result<McpSession, McpError> {
        let inner = &self.inner;
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
            let grant = owner.as_ref().map(|owner| owner.grant.clone());
            let mut granted = grant
                .as_ref()
                .map(|grant| grant.state.lock().expect("grant"));
            if granted.as_ref().is_some_and(|granted| granted.revoked) {
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
            if let Some(granted) = granted.as_mut() {
                granted.sessions.retain(|each| each.strong_count() > 0);
                granted.sessions.push(Arc::downgrade(&session));
            }
            drop(granted);
            session
        };
        if session.owned_by.is_some() {
            // Subscribed before the first list, so a change during it is not missed.
            let notices = session.connection.notices();
            tokio::spawn(keep_listed(Arc::downgrade(&session), notices));
        }
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
    /// or there is no list yet, which an app cannot be acting on. A name
    /// listed more than once is one tool: a side may see it only when every
    /// entry says so, and the `resourceUri` and hints are the first entry's.
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
            let combined = wire::one_visibility_per_name(
                listed
                    .tools
                    .iter()
                    .filter(|each| each.tool().tool() == name)
                    .map(|each| (name.to_owned(), each.ui().visibility())),
            );
            let visibility = combined.first()?.1;
            let first = listed
                .tools
                .iter()
                .find(|each| each.tool().tool() == name)?;
            Some(listed_as(first, visibility))
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
            each.owned_by
                .as_ref()
                .is_some_and(|owner| owner.session() == session)
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

    /// Close every open session of the configured server `name`. Calls waiting
    /// end [`McpError::Closed`]. Other servers' sessions stay open. An
    /// authorization owner calls this when a grant is retired, so a token
    /// that must not be used again is not left on an open session.
    pub fn close_named(&self, name: &str) {
        let sessions: Vec<Arc<Session>> = self
            .open_sessions()
            .into_iter()
            .filter(|session| session.server == name)
            .collect();
        for session in sessions {
            session.connection.close(McpError::Closed);
            match tokio::runtime::Handle::try_current() {
                Ok(runtime) => {
                    runtime.spawn(close(session, McpError::Closed));
                }
                Err(_) => session.kill_now(McpError::Closed),
            }
        }
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

/// Why `servers` and `remotes` cannot be configured together, or `None`.
/// The count is [`MAX_MCP_SERVERS`] across both, names are unique across
/// both, remote ids are unique, and each entry's own rules apply. The first
/// problem found is the one returned, the count before any server's.
pub fn configuration_problem(
    servers: &[McpServerLaunch],
    remotes: &[RemoteMcpServer],
) -> Option<McpServerProblem> {
    if servers.len() + remotes.len() > MAX_MCP_SERVERS {
        return Some(McpServerProblem::TooMany);
    }
    let mut names = std::collections::HashSet::new();
    let mut ids = std::collections::HashSet::new();
    for launch in servers {
        if let Some(problem) = launch.problem() {
            return Some(problem);
        }
        if !names.insert(launch.server.name.clone()) {
            return Some(McpServerProblem::DuplicateName {
                server: launch.server.name.clone(),
            });
        }
    }
    for remote in remotes {
        if let Some(problem) = remote.problem() {
            return Some(problem);
        }
        if !names.insert(remote.name().to_owned()) {
            return Some(McpServerProblem::DuplicateName {
                server: remote.name().to_owned(),
            });
        }
        if !ids.insert(remote.id()) {
            return Some(McpServerProblem::DuplicateServerId {
                server: remote.name().to_owned(),
            });
        }
    }
    None
}

fn by_name(servers: Vec<McpServerLaunch>) -> Result<BTreeMap<String, McpServerLaunch>, McpError> {
    if let Some(problem) = McpServerLaunch::problem_in(&servers) {
        return Err(McpError::InvalidConfiguration(problem));
    }
    Ok(servers
        .into_iter()
        .map(|launch| (launch.server.name.clone(), launch))
        .collect())
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

    /// List the tools on the first `max_pages` pages of `tools/list`, at most
    /// [`MAX_TOOLS`] of them, and say whether the server had more: a look at
    /// what a server offers ([`McpServers::open_once`]) that stops at its
    /// bound instead of failing past it. Nothing is kept: neither
    /// [`McpServers::tool_ui`] nor a stand-in's visibility reads this list.
    ///
    /// # Errors
    ///
    /// [`McpError::Timeout`] for a page not answered within
    /// [`REQUEST_TIMEOUT`], [`McpError::Malformed`] for a page of the wrong
    /// shape, [`McpError::Remote`] for the server's refusal, and the
    /// session's end cause once it has ended.
    pub async fn list_tool_pages(&self, max_pages: usize) -> Result<ListedPages, McpError> {
        let paged = contain_caller_wake(
            format!("MCP tool pages of {}", self.server()),
            pages(&self.owner.0, max_pages),
        )
        .await?;
        Ok(ListedPages {
            tools: paged.tools,
            more: paged.more,
        })
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
    /// `*/list_changed` notices are passed on. A `tools/call`'s arguments are
    /// kept when the call is accepted, and its result's `structuredContent`
    /// before the harness is answered, both in the grant's
    /// [`McpOwner::forwarded`] under the harness's id for the call, with this
    /// server's name.
    pub async fn serve(self, input: impl AsyncRead + Unpin, output: impl AsyncWrite + Unpin) {
        let server = self.server().to_owned();
        contain_caller_wake(format!("MCP serve of {server}"), async move {
            stand_in::serve(
                self.owner.0.connection.clone(),
                self.owner.0.initialized.clone(),
                self.owner.0.visibility.clone(),
                self.owner
                    .0
                    .owned_by
                    .as_ref()
                    .map_or_else(ForwardedResults::new, McpOwner::forwarded),
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
/// A remote session waits up to [`STOP_GRACE`] for its DELETE observation.
async fn close(session: Arc<Session>, cause: McpError) {
    let http_done = session.connection.http_finished();
    let mut stopped = session.stopped.subscribe();
    if let Some(process) = session.close_now(cause) {
        // Said stopped however this ends: finished, or cancelled with the
        // process dropped, which kills its group.
        let _said = Stopped(&session.stopped);
        process.stop().await;
        return;
    }
    if let Some(mut done) = http_done {
        let _ = tokio::time::timeout(super::process::STOP_GRACE, done.wait_for(|done| *done)).await;
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

/// What [`McpSession::list_tool_pages`] read: the tools of the pages it
/// read, and whether the server had more than it read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedPages {
    /// The tools listed on the pages read, in order, at most [`MAX_TOOLS`].
    pub tools: Vec<ListedTool>,
    /// Whether the server had more — another page past the bound, or tools
    /// past [`MAX_TOOLS`] — that was not read.
    pub more: bool,
}

/// The tools on `session`'s first `max_pages` pages of `tools/list`, at most
/// [`MAX_TOOLS`] of them, with each tool's visibility; and whether there was
/// more.
async fn pages(session: &Session, max_pages: usize) -> Result<Paged, McpError> {
    let mut paged = Paged::default();
    let mut cursor: Option<String> = None;
    for _ in 0..max_pages {
        let params = cursor.map(|cursor| json!({ "cursor": cursor }));
        let result = session
            .connection
            .request("tools/list", params, REQUEST_TIMEOUT)
            .await?;
        let page = wire::tools_page(&session.server, &result)?;
        if paged.tools.len() + page.tools.len() > MAX_TOOLS
            || paged.hidden.len() + page.hidden.len() > MAX_TOOLS
        {
            let room = MAX_TOOLS - paged.tools.len();
            paged.tools.extend(page.tools.into_iter().take(room));
            paged.more = true;
            return Ok(paged);
        }
        paged.tools.extend(page.tools);
        paged.hidden.extend(page.hidden);
        match page.next {
            Some(next) => cursor = Some(next),
            None => return Ok(paged),
        }
    }
    paged.more = true;
    Ok(paged)
}

#[derive(Default)]
struct Paged {
    tools: Vec<ListedTool>,
    hidden: Vec<(String, bool)>,
    more: bool,
}

/// Every page of `tools/list`, kept as the session's tools when all were read
/// and no list asked later has been kept already.
async fn list(session: &Session) -> Result<Vec<ListedTool>, McpError> {
    let order = session.visibility.ask();
    let paged = pages(session, MAX_TOOL_PAGES).await?;
    if paged.more {
        return Err(McpError::TooLarge("tools/list"));
    }
    session
        .visibility
        .listed(order, wire::hidden_for_model(&paged.tools, &paged.hidden));
    let mut kept = session.tools.write().expect("tool list");
    if kept.as_ref().is_none_or(|kept| kept.order < order) {
        *kept = Some(Arc::new(Listed {
            order,
            tools: Arc::from(paged.tools.clone()),
        }));
    }
    Ok(paged.tools)
}

/// `first` with `visibility` in place of its own. The URI and hints stay
/// `first`'s; who may see the name does not.
fn listed_as(first: &ListedTool, visibility: UiVisibility) -> ListedTool {
    ListedTool::new(
        first.tool().clone(),
        ToolUi::new(first.ui().resource_uri().cloned(), visibility),
    )
    .with_hints(first.hints())
}
