//! What managing the stored MCP servers needs from outside: the stored list
//! and its lock, the live set it is launched as, and somewhere durable to
//! record each change.
use crate::mcp_servers::domain::{ConfiguredMcpServer, StdioServer};
use std::{future::Future, pin::Pin};

/// The stored servers, as one read sees them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredServers {
    /// The digest of the stored block, which a write must name.
    pub revision: String,
    /// The servers in stored order.
    pub servers: Vec<ConfiguredMcpServer>,
}

/// Why the stored servers could not be read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// The configuration does not parse, as it is or as it would be written.
    /// It is never repaired
    /// (`s8_a_configuration_that_does_not_parse_is_refused_and_never_repaired`).
    ConfigInvalid,
    /// The configuration would be larger than its bound.
    ConfigTooLarge,
    /// Another holder kept the lock past the store's bounded wait.
    Busy,
    /// It could not be read, locked or published.
    Unavailable,
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
    /// The servers stored now.
    fn read(&self) -> Result<StoredServers, StoreError>;
    /// Store `servers` in place of the stored block, leaving the rest of the
    /// configuration as it is, and answer the new revision. Made only under
    /// the lock ([`Self::try_lock`]); the stored configuration is unchanged on
    /// every error.
    fn write(&self, servers: &[ConfiguredMcpServer]) -> Result<String, StoreError>;
}

/// Why a list of servers cannot be the live set: the SDK's rules for a
/// server and a set, as the live set port reports them. No variant carries a
/// variable's value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerProblem {
    TooMany,
    DuplicateName { name: String },
    Name,
    Command,
    Arguments,
    EnvironmentName,
    ReservedEnvironmentName { name: String },
    EnvironmentValue { name: String },
}

/// The live set was kept as it was: the gateway is stopping, or the SDK
/// refused the set, which [`LiveServerSet::problem`] reports first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveSetKept;

/// The live set the stored servers are launched as, with the server Nessa
/// manages beside them.
pub trait LiveServerSet: Send + Sync {
    /// The managed server, when this gateway has one.
    fn managed(&self) -> Option<StdioServer>;
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
    /// Who asked: the authenticated caller.
    pub initiator: McpServerInitiator,
    /// What was asked, with variable names and never their values
    /// (`a_save_is_published_then_replaces_the_live_set_and_is_audited_both_sides`).
    pub request: McpServerChangeRequest,
    pub phase: McpServerAuditPhase,
}

/// The authenticated caller of a change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServerInitiator {
    pub organization_id: String,
    pub principal_id: String,
    pub credential_id: String,
}

/// A change as it was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServerChangeRequest {
    /// `save` or `remove`.
    pub action: McpServerAction,
    /// The name stored or removed.
    pub target: String,
    /// The name it was stored under, for a rename.
    pub previous_name: Option<String>,
    /// The revision the caller named.
    pub revision: String,
    /// For a save: its variables' names, in order, and whether it is on.
    pub env_names: Vec<String>,
    pub enabled: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpServerAction {
    Save,
    Remove,
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
    /// file.
    Applied {
        before: ServerNames,
        after: ServerNames,
        live_set_replaced: bool,
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
}

/// A revision and the server names stored at it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerNames {
    pub revision: String,
    pub names: Vec<String>,
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
