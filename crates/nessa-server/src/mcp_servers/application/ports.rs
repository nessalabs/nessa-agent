//! What managing the stored MCP servers needs from outside: the stored list
//! and its lock, the live set it is launched as, a way to start one server
//! once and look at it, and somewhere durable to record each change and
//! each inspection.
use crate::mcp_servers::domain::ConfiguredMcpServer;
use nessa_sdk::domain::mcp_apps::{UiCsp, UiPermissions};
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::watch;

/// The stored servers, as one read sees them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredServers {
    /// The digest of the stored block, which a write must name.
    pub revision: String,
    /// The servers in stored order.
    pub servers: Vec<ConfiguredMcpServer>,
}

/// Why the stored servers could not be read or written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// The configuration does not parse, as it is or as it would be written.
    /// It is never repaired
    /// (`s8_a_configuration_that_does_not_parse_is_refused_and_never_repaired`).
    ConfigInvalid,
    /// The configuration would be larger than its bound.
    ConfigTooLarge,
    /// Another holder kept the lock past the store's bounded wait.
    Busy,
    /// The stored block is not at the revision a write names: something
    /// changed it outside the lock since it was read. This carries the
    /// revision stored now.
    RevisionConflict { revision: String },
    /// It could not be read, locked or published.
    Unavailable,
}

/// What a write published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    /// The stored block's revision now.
    pub revision: String,
    /// Whether the publish was made durable: false when the file was
    /// replaced but its directory could not be synced, so a crash could
    /// still lose it. The file is new either way.
    pub durable: bool,
}

/// The lock on the stored configuration, held until dropped.
pub type StoreLock = Box<dyn Send>;

/// What [`McpServerStore::lock`] answers.
pub type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, StoreError>> + Send + 'a>>;

/// Where the servers are stored: `config.json`'s `agents.mcpServers`.
/// `read` and `write` block on the file system; the caller runs them off the
/// async runtime's workers.
pub trait McpServerStore: Send + Sync {
    /// The lock every write is made under, waiting a bounded time on the
    /// store's clock for another holder to let it go; [`StoreError::Busy`]
    /// past that.
    fn lock(&self) -> StoreFuture<'_, StoreLock>;
    /// How long [`Self::lock`] waits at most: what bounds a shutdown's wait
    /// for a change under way.
    fn lock_wait(&self) -> Duration;
    /// The servers stored now.
    fn read(&self) -> Result<StoredServers, StoreError>;
    /// Store `servers` in place of the stored block, leaving the rest of the
    /// configuration as it is, and answer the new revision and whether it
    /// was made durable — when the block it reads now is still at
    /// `revision`, and [`StoreError::RevisionConflict`] when it is not
    /// (`a_change_made_outside_the_lock_after_the_read_is_a_conflict`). Made
    /// only under the lock ([`Self::lock`]); the stored configuration is
    /// unchanged on every error. A file replaced whose directory could not
    /// be synced is not an error: it is [`Written`] with `durable: false`
    /// (`s_sync_a_publish_whose_directory_sync_fails_is_applied_not_durable`).
    fn write(&self, revision: &str, servers: &[ConfiguredMcpServer])
        -> Result<Written, StoreError>;
}

/// Why a list of servers cannot be the live set: the SDK's rules for a
/// server and a set, as the live set port reports them. Each problem about
/// one server names it (`server`), so an entry added by hand is named too;
/// `name` is a variable's name. No variant carries a variable's value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerProblem {
    TooMany,
    DuplicateName { server: String },
    Name { server: String },
    Command { server: String },
    Arguments { server: String },
    EnvironmentName { server: String, name: String },
    ReservedEnvironmentName { server: String, name: String },
    EnvironmentValue { server: String, name: String },
}

/// The live set was kept as it was: the gateway is stopping, or the SDK
/// refused the set, which [`LiveServerSet::problem`] reports first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveSetKept;

/// The live set the stored servers are launched as, with the server Nessa
/// manages beside them.
pub trait LiveServerSet: Send + Sync {
    /// The managed server as this gateway started with it — turned on or
    /// off, with its own variables — when it has one.
    fn managed(&self) -> Option<ConfiguredMcpServer>;
    /// Why `stored` — every server, on or off, with the managed one — cannot
    /// be launched as one set, or `None` when it can.
    fn problem(&self, stored: &[ConfiguredMcpServer]) -> Option<ServerProblem>;
    /// Make `stored`'s servers that are on, with the managed one, the live
    /// set: what the next open reads.
    ///
    /// # Errors
    ///
    /// [`LiveSetKept`] once the gateway is stopping, or for a set
    /// [`Self::problem`] refuses; the set is kept.
    fn replace(&self, stored: &[ConfiguredMcpServer]) -> Result<(), LiveSetKept>;
}

/// The durable record of one change, by one caller, to the stored servers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServerAuditRecord {
    /// Minted when the change was asked for; the requested record and its
    /// outcome share it.
    pub operation_id: String,
    /// Why this record's transition happened, and who set it off.
    pub cause: McpServerCause,
    /// What was asked, with variable names and never their values
    /// (`a_save_is_published_then_replaces_the_live_set_and_is_audited_both_sides`).
    pub request: McpServerChangeRequest,
    pub phase: McpServerAuditPhase,
}

/// Why a record's transition happened: the operation's cause, which is the
/// caller's request — a deadline or a fault included, as they end the
/// caller's operation — unless shutdown ended it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpServerCause {
    /// The authenticated caller asked for it.
    CallerRequested(McpServerInitiator),
    /// The gateway began to stop, which ended an inspection: not started,
    /// or cut. The gateway — the system — is its initiator; the caller who
    /// asked is on the requested record with the same operation
    /// (`shutdown_ended_inspections_are_recorded_as_the_gateway_stopping`).
    GatewayStopping,
}

/// The authenticated caller of a change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServerInitiator {
    pub organization_id: String,
    pub principal_id: String,
    pub credential_id: String,
}

/// A change, or an inspection, as it was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServerChangeRequest {
    /// `save`, `remove` or `inspect`.
    pub action: McpServerAction,
    /// The name stored, removed or inspected.
    pub target: String,
    /// The name it was stored under, for a rename.
    pub previous_name: Option<String>,
    /// The revision the caller named; for an inspection, the one its server
    /// was read at.
    pub revision: String,
    /// For a save, the server asked for — its executable and arguments, on
    /// or off, its variables' names — so a refused save still names what
    /// was asked; for an inspection, the server as stored, which ran;
    /// `None` for a remove
    /// (`a_refused_save_still_records_the_executable_it_asked_for`).
    pub server: Option<Box<AuditedServer>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpServerAction {
    Save,
    Remove,
    /// `mcpServers.inspect`: the stored server started once and looked at.
    /// Nothing stored changes; the record is of a process the gateway ran
    /// with the server's variables.
    Inspect,
}

/// Which record of a change this is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpServerAuditPhase {
    /// Written before the lock is taken. When it cannot be, nothing is
    /// locked, written or applied.
    Requested,
    /// Written after the effect, or after the refusal or failure that stopped
    /// it.
    Outcome(McpServerOutcome),
}

/// How a change ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpServerOutcome {
    /// Published. `live_set_replaced` is false only when the gateway was
    /// stopping, so its live set was not replaced; the next start reads the
    /// file. `durable` is false when the file was replaced but its
    /// directory could not be synced ([`Written::durable`]).
    Applied {
        before: ServerNames,
        after: ServerNames,
        live_set_replaced: bool,
        durable: bool,
    },
    /// Refused before anything was written; `before` is what was stored, when
    /// it could be read.
    Refused {
        reason: &'static str,
        before: Option<ServerNames>,
    },
    /// Failed before anything was written; `before` as for a refusal.
    Failed {
        reason: &'static str,
        before: Option<ServerNames>,
    },
    /// An inspection the server failed, or that was not started; `started`
    /// says whether its process was ([`InspectFailure::started`]), so a
    /// record of `stopping` says in so many words that nothing ran.
    InspectFailed { reason: &'static str, started: bool },
    /// An inspection read what the server offered, within its bounds, and
    /// the server was stopped. `cut` names the bound that stopped the
    /// reading early — shutdown among them ([`InspectCut::Stopping`]); the
    /// answer's own byte bound is the wire's, after this.
    Inspected {
        tools: usize,
        cut: Option<InspectCut>,
    },
}

/// A revision, the server names stored at it, and the server the change
/// names as it was stored there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerNames {
    pub revision: String,
    pub names: Vec<String>,
    /// The change's target at this revision: before a change, the server
    /// under its previous name for a rename, else under its name; after
    /// one, the server under its name. `None` where none is stored — before
    /// an add, after a remove, and for an inspection
    /// (`the_audit_records_the_targets_before_and_after_on_save_rename_disable_and_remove`).
    pub target: Option<Box<AuditedServer>>,
}

/// A stored server as a record names it: everything it is started with but
/// its variables' values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditedServer {
    pub name: String,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub enabled: bool,
    /// Its variables' names, sorted by name; never their values.
    pub env_names: Vec<String>,
}
impl AuditedServer {
    /// `server` as a record names it.
    pub fn of(server: &ConfiguredMcpServer) -> Self {
        Self {
            name: server.server().name().to_owned(),
            command: server.server().command().to_owned(),
            args: server.server().args().to_vec(),
            enabled: server.enabled(),
            env_names: server.env_names(),
        }
    }
}

/// The record could not be made durable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuditUnavailable;

/// Where each change's records are kept. Blocks on the file system; the
/// caller runs it off the async runtime's workers.
pub trait McpServerAudit: Send + Sync {
    /// Make `record` durable.
    fn record(&self, record: &McpServerAuditRecord) -> Result<(), AuditUnavailable>;
}

/// What one inspection may spend: an overall deadline on the gateway's clock,
/// from before the server is launched until it has been read, and caps on
/// the pages of tools and the UI resources read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InspectBounds {
    pub deadline: Duration,
    pub max_tool_pages: usize,
    pub max_ui_reads: usize,
}

/// Which bound left an inspection incomplete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectCut {
    /// More pages of tools than [`InspectBounds::max_tool_pages`], or more
    /// tools than the SDK reads from one server.
    Tools,
    /// More distinct UI resources than [`InspectBounds::max_ui_reads`]; tools
    /// past it carry no UI.
    Ui,
    /// The answer would pass its byte bound; tools were dropped from the end.
    Bytes,
    /// The gateway began to stop after the server was started: the reading
    /// was dropped, with what it had read, and the server's process group
    /// killed at once. Its tools are none.
    Stopping,
}

/// What a server offered, as one inspection read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inspection {
    /// Its tools, in its order.
    pub tools: Vec<InspectedTool>,
    /// The bound that stopped the reading early, if one did.
    pub cut: Option<InspectCut>,
}

/// One tool a server lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectedTool {
    pub name: String,
    pub read_only_hint: Option<bool>,
    pub destructive_hint: Option<bool>,
    /// Its MCP App, when it declares one and it was read.
    pub ui: Option<InspectedUi>,
}

/// A tool's MCP App: its resource, and what that resource asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectedUi {
    pub uri: String,
    pub csp: UiCsp,
    pub permissions: UiPermissions,
}

/// Why an inspection read nothing. The server is not running after any of
/// them: stopped, or never started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InspectFailure {
    /// The stored server breaks the SDK's rules for what a server is
    /// started with — an entry added to the file by hand, say — so it was
    /// not started (`i_invalid_a_stored_server_that_breaks_a_rule_is_invalid_and_not_started`).
    Invalid(ServerProblem),
    /// Its process could not be launched.
    StartFailed,
    /// It was not started: the gateway is stopping, and the SDK refused to
    /// launch it.
    Stopping,
    /// It did not finish within the deadline, or a request's own budget.
    TimedOut,
    /// It ended, or its session was closed, before it answered.
    Gone,
    /// It answered something that is not MCP, or a UI resource that is not
    /// an MCP App within its bounds.
    Malformed,
    /// It answered with a JSON-RPC error.
    RemoteError { code: i64, message: String },
}
impl InspectFailure {
    /// Whether the server's process was started before this.
    pub fn started(&self) -> bool {
        !matches!(self, Self::Invalid(_) | Self::StartFailed | Self::Stopping)
    }
}

/// Shutdown's word to the inspections under way, observed beside each
/// one's deadline: once given, an inspection not yet started is not
/// ([`InspectFailure::Stopping`]), and one started is cut
/// ([`InspectCut::Stopping`]) with its process group killed. Given by
/// [`McpServerSettings::shutdown`](super::McpServerSettings::shutdown).
#[derive(Clone, Debug)]
pub struct InspectStop(watch::Receiver<bool>);
impl InspectStop {
    /// The stop `given` reports: given once it holds `true`. A sender gone
    /// without giving it is never given.
    pub fn new(given: watch::Receiver<bool>) -> Self {
        Self(given)
    }
    /// Whether the stop has been given.
    pub fn given(&self) -> bool {
        *self.0.borrow()
    }
    /// Once the stop is given; never, if its sender goes without giving it.
    pub async fn wait(&mut self) {
        if self.0.wait_for(|given| *given).await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Marked by an inspector as the inspected server's launch begins, so a
/// fault after that is answered as a server that may have run, and one
/// before it as a server that did not.
#[derive(Clone, Debug, Default)]
pub struct LaunchBegun(Arc<AtomicBool>);
impl LaunchBegun {
    /// The mark `flag` holds: begun once it holds `true`.
    pub fn over(flag: Arc<AtomicBool>) -> Self {
        Self(flag)
    }
    /// The launch begins now.
    pub fn mark(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    /// Whether the launch has begun.
    pub fn marked(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// What [`ServerInspector::inspect`] answers.
pub type InspectFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Inspection, InspectFailure>> + Send + 'a>>;

/// Starts one stored server once, outside any conversation, reads what it
/// offers within `bounds` — or until `stop` is given — and stops it with
/// its process group before answering, however the reading ended. It marks
/// `launch` just before it asks for the server to be opened, and not when it
/// is stopped first. The opening is also what refuses an invalid server, so a
/// refused opening can follow the mark: a fault after it is then answered as
/// a server that may have run, the side that over-reports rather than hides
/// a launch. Its outcome is left unrecorded, since the task never reached it.
pub trait ServerInspector: Send + Sync {
    fn inspect(
        &self,
        server: &ConfiguredMcpServer,
        bounds: InspectBounds,
        stop: InspectStop,
        launch: LaunchBegun,
    ) -> InspectFuture<'_>;
}
