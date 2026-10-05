//! Trusted host configuration for launching and supervising an ACP process.
#![deny(missing_docs)]

use super::StandInSessions;
use crate::application::agent_execution::{
    agents::AgentError,
    providers::{ExecutableUseSnapshot, UserImageSource},
};
use crate::domain::agent_execution::permissions::PermissionOfferPolicy;
use crate::infrastructure::clock::Clock;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsString,
    fmt,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

/// Trusted stdio MCP server. This is host configuration, never model-supplied input.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StdioMcpServer {
    /// Unique ASCII server name (letters, digits, hyphen, underscore; at most
    /// [`MAX_MCP_SERVER_NAME_BYTES`]).
    pub name: String,
    /// Absolute UTF-8 executable path, launched directly without shell interpolation.
    pub command: PathBuf,
    /// Ordered UTF-8 arguments. Never put credentials here: any process list shows them.
    #[serde(default)]
    pub args: Vec<String>,
}
impl StdioMcpServer {
    /// Why this server cannot be launched as configured, or `None` when it
    /// can: a name of ASCII letters, digits, `-` and `_`, 1 to
    /// [`MAX_MCP_SERVER_NAME_BYTES`] bytes, without
    /// `__` and neither starting nor ending with `_` (a harness names a tool
    /// `mcp__<server>__<tool>`, so the server's name must not run into the
    /// separators on either side); an absolute UTF-8
    /// executable; at most [`MAX_MCP_SERVER_ARGS`] arguments of at most
    /// [`MAX_MCP_SERVER_ARG_BYTES`] bytes, none holding NUL. The rules for
    /// one server; [`Self::problem_in`] adds the set's.
    pub fn problem(&self) -> Option<McpServerProblem> {
        if self.name.is_empty()
            || self.name.len() > MAX_MCP_SERVER_NAME_BYTES
            || self.name.contains("__")
            || self.name.starts_with('_')
            || self.name.ends_with('_')
            || !self
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Some(McpServerProblem::Name {
                server: self.name.clone(),
            });
        }
        if !self.command.is_absolute() || self.command.to_str().is_none() {
            return Some(McpServerProblem::Command {
                server: self.name.clone(),
            });
        }
        if self.args.len() > MAX_MCP_SERVER_ARGS
            || self
                .args
                .iter()
                .any(|arg| arg.len() > MAX_MCP_SERVER_ARG_BYTES || arg.contains('\0'))
        {
            return Some(McpServerProblem::Arguments {
                server: self.name.clone(),
            });
        }
        None
    }
    /// Why `servers` cannot be launched together, or `None` when they can:
    /// at most [`MAX_MCP_SERVERS`], each without a [`Self::problem`], each
    /// under a name of its own. The one statement of the set's rules: an ACP
    /// binding ([`AcpConfig`]) and the MCP client
    /// ([`McpServers`](crate::infrastructure::mcp::McpServers), when it is
    /// built and when its set is replaced) both ask it. The first problem
    /// found is the one returned, the count before any server's.
    pub fn problem_in<'a>(
        servers: impl IntoIterator<Item = &'a StdioMcpServer>,
    ) -> Option<McpServerProblem> {
        let servers: Vec<_> = servers.into_iter().collect();
        if servers.len() > MAX_MCP_SERVERS {
            return Some(McpServerProblem::TooMany);
        }
        let mut names = HashSet::new();
        servers.into_iter().find_map(|server| {
            server.problem().or_else(|| {
                (!names.insert(&server.name)).then(|| McpServerProblem::DuplicateName {
                    server: server.name.clone(),
                })
            })
        })
    }
}

/// The most MCP servers one set may hold: what an ACP binding is given and
/// what the MCP client runs ([`StdioMcpServer::problem_in`]).
pub const MAX_MCP_SERVERS: usize = 16;
/// The most bytes in an MCP server's name ([`StdioMcpServer::problem`]).
pub const MAX_MCP_SERVER_NAME_BYTES: usize = 64;
/// The most arguments an MCP server is started with
/// ([`StdioMcpServer::problem`]).
pub const MAX_MCP_SERVER_ARGS: usize = 64;
/// The most bytes in one of an MCP server's arguments
/// ([`StdioMcpServer::problem`]).
pub const MAX_MCP_SERVER_ARG_BYTES: usize = 8192;

/// Why a server, or a set of servers, cannot be launched as configured. The
/// one owner of those rules is [`StdioMcpServer::problem_in`] (with
/// [`StdioMcpServer::problem`] for one server's), and, for what a server
/// process is started with,
/// [`McpServerLaunch::problem`](crate::infrastructure::mcp::McpServerLaunch::problem).
/// Branch on the variant; the text is for people. No variant carries an
/// environment variable's value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpServerProblem {
    /// More than [`MAX_MCP_SERVERS`] servers.
    TooMany,
    /// Two servers are configured under this name.
    DuplicateName {
        /// The name configured twice.
        server: String,
    },
    /// The name is empty, longer than [`MAX_MCP_SERVER_NAME_BYTES`], holds `__`, starts or ends
    /// with `_`, or holds anything but ASCII letters, digits, `-` and `_`.
    Name {
        /// The name as it is configured.
        server: String,
    },
    /// The executable is not an absolute UTF-8 path.
    Command {
        /// The server's name.
        server: String,
    },
    /// More than [`MAX_MCP_SERVER_ARGS`] arguments, or one longer than
    /// [`MAX_MCP_SERVER_ARG_BYTES`] or holding NUL.
    Arguments {
        /// The server's name.
        server: String,
    },
    /// An environment variable's name is empty, longer than
    /// [`MAX_MCP_ENVIRONMENT_NAME_BYTES`](crate::infrastructure::mcp::MAX_MCP_ENVIRONMENT_NAME_BYTES),
    /// starts with a digit, or holds anything but ASCII letters, digits and
    /// `_`.
    EnvironmentName {
        /// The server's name.
        server: String,
        /// The variable's name, any byte that is not UTF-8 replaced; never
        /// its value.
        name: String,
    },
    /// An environment variable's name is one a server may not be given:
    /// [`MCP_SESSION_VARIABLE`](crate::infrastructure::mcp::MCP_SESSION_VARIABLE),
    /// which carries a host's session token to its stand-ins.
    ReservedEnvironmentName {
        /// The server's name.
        server: String,
        /// The reserved name.
        name: String,
    },
    /// An environment variable's value holds NUL, which no process can be
    /// given.
    EnvironmentValue {
        /// The server's name.
        server: String,
        /// The variable's name; never its value.
        name: String,
    },
}
impl fmt::Display for McpServerProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooMany => write!(f, "at most {MAX_MCP_SERVERS} MCP servers"),
            Self::DuplicateName { server } => {
                write!(f, "two MCP servers are configured as {server:?}")
            }
            Self::Name { server } => write!(f, "invalid MCP server name {server:?}"),
            Self::Command { server } => write!(
                f,
                "the MCP server {server:?}'s executable must be an absolute UTF-8 path"
            ),
            Self::Arguments { server } => {
                write!(f, "invalid arguments for the MCP server {server:?}")
            }
            Self::EnvironmentName { server, name } => write!(
                f,
                "invalid environment variable name {name:?} for the MCP server {server:?}"
            ),
            Self::ReservedEnvironmentName { server, name } => write!(
                f,
                "{name} is reserved and cannot be given to the MCP server {server:?}"
            ),
            Self::EnvironmentValue { server, name } => write!(
                f,
                "the environment variable {name} of the MCP server {server:?} holds NUL"
            ),
        }
    }
}
impl std::error::Error for McpServerProblem {}

/// Where an ACP binding reads its MCP servers from: a host's live set, read
/// once at each provider open ([`McpServerList::opened`]).
pub trait McpServerSource: Send + Sync {
    /// The servers a provider open is given now. Called on the open's task,
    /// so it must not block; it may be called from several opens at once.
    fn servers(&self) -> Vec<StdioMcpServer>;
}

/// The MCP servers an [`AcpConfig`] gives its provider opens: a fixed list,
/// or a host's [`McpServerSource`] read once for each open. An open keeps
/// what it read for its provider session's life, through every restart of
/// its process, so a harness already running keeps the servers it was given
/// while a later open is given the source's servers then.
#[derive(Clone, Default)]
pub struct McpServerList {
    source: Option<Arc<dyn McpServerSource>>,
    servers: Arc<[StdioMcpServer]>,
}
impl McpServerList {
    /// No MCP servers.
    pub fn none() -> Self {
        Self::default()
    }
    /// The same `servers` for every open.
    pub fn fixed(servers: Vec<StdioMcpServer>) -> Self {
        Self {
            source: None,
            servers: servers.into(),
        }
    }
    /// What `source` says at each open.
    pub fn read_from(source: Arc<dyn McpServerSource>) -> Self {
        Self {
            source: Some(source),
            servers: Arc::new([]),
        }
    }
    /// The servers as they are now: the fixed list, or what the source says
    /// now. Two calls on a list read from a source may differ; a provider
    /// open reads once, through [`Self::opened`].
    pub fn current(&self) -> Arc<[StdioMcpServer]> {
        match &self.source {
            Some(source) => source.servers().into(),
            None => self.servers.clone(),
        }
    }
    /// For one provider open: the servers as they are now, fixed for that
    /// open. The ACP binding asks this itself, once per open; a host calls it
    /// only to check what an open would be given.
    pub fn opened(&self) -> Self {
        Self {
            source: None,
            servers: self.current(),
        }
    }
}
impl fmt::Debug for McpServerList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.source {
            Some(_) => f.write_str("McpServerList(read at each open)"),
            None => f.debug_list().entries(self.servers.iter()).finish(),
        }
    }
}

/// Host-owned launch configuration. Environment is explicit, never inherited by
/// the adapter. Use the normal HOME/auth environment without extracting secrets.
/// Only trusted composition may choose the executable and its arguments.
#[derive(Clone)]
pub struct AcpConfig {
    /// Trusted provider executable and the authority required before each launch.
    /// The adapter admits a distinct use generation immediately before spawning it.
    pub executable: ExecutableUseSnapshot,
    /// Noncredential arguments passed verbatim in order. These select restoration context, so
    /// they are part of the restoration identity, whose inputs are listed on `fingerprint` in
    /// `acp/sessions/identity.rs`. Never place secrets in arguments. Only trusted composition
    /// may supply them.
    pub arguments: Vec<OsString>,
    /// Noncredential context-selecting environment, including HOME, configuration directories,
    /// endpoints, and executable search paths. These values enter the restoration fingerprint,
    /// whose inputs are listed on `fingerprint` in `acp/sessions/identity.rs`.
    /// Parent variables are cleared; this map and credential_environment supply the child.
    pub environment: BTreeMap<OsString, OsString>,
    /// Host-supplied credentials forwarded verbatim, without discovery or extraction.
    /// Excluded from restoration identity so credentials may rotate; the fingerprint's inputs
    /// are listed on `fingerprint` in `acp/sessions/identity.rs`. Do not put context
    /// selectors here. Rotation assumes the same intended provider account/context;
    /// keep account/profile namespaces in environment when switching accounts.
    /// Keys must not overlap environment. Never persisted by the SDK.
    pub credential_environment: BTreeMap<OsString, OsString>,
    /// Absolute path used as the child working directory and ACP session workspace. It must exist
    /// and be accessible when the process starts. Must be UTF-8 because ACP encodes
    /// the workspace in JSON; invalid paths return Configuration before launch.
    pub workspace: PathBuf,
    /// Whether this binding accepts provider tool events and permission requests. When false,
    /// tool events are protocol errors and permission requests are cancelled; this switch is not
    /// an OS filesystem sandbox.
    pub tools_enabled: bool,
    /// Trusted MCP servers exposed by profiles that support MCP, read once at each provider
    /// open ([`McpServerList::opened`]) and kept for that provider session's life. None
    /// disables custom tools. Servers require tools_enabled.
    /// Excluded from restoration identity: like `stand_ins`, they are attached to each provider
    /// open rather than selecting the provider context, so changing them leaves a saved session
    /// restorable; the fingerprint's inputs are listed on `fingerprint` in
    /// `acp/sessions/identity.rs`.
    pub mcp_servers: McpServerList,
    /// Where each provider open's MCP server processes get their per-open environment, and the
    /// results its stand-ins forward are taken from: a host's grant for the SDK session being
    /// opened, held for that provider session's life.
    /// Excluded from restoration identity, like credentials; the fingerprint's inputs are listed
    /// on `fingerprint` in `acp/sessions/identity.rs`. Held in memory only: no session
    /// snapshot records it, since a snapshot records the provider context, not the launch.
    pub stand_ins: StandInSessions,
    /// Allowed permission decisions offered for provider requests. Provider choices are
    /// restricted to this policy; the selected profile must support every configured scope. An
    /// answer still requires verified caller attribution and audit delivery.
    pub permissions: PermissionOfferPolicy,
    /// Deadline for the whole `initialize` exchange, which is mostly the operating system's work
    /// rather than the provider's: `exec`, any first-execution scan of a newly written
    /// executable, the runtime's own boot, and only then a short protocol round trip. A freshly
    /// installed or updated runtime is scanned on first use and can take tens of seconds longer
    /// than the same executable a second time, so size this for the cold case.
    ///
    /// It is measured from the start of the adapter's worker task, which is spawned immediately
    /// after the process is: scheduling that task is not charged to this budget, and neither is
    /// anything before the process is launched. Provider notifications and provider-originated
    /// requests arriving during the exchange are answered within the same budget rather than
    /// extending it.
    ///
    /// Expiry fails with [`AgentError::StartupDeadline`] naming
    /// [`AgentStartupPhase::Initialize`](crate::application::agent_execution::agents::AgentStartupPhase::Initialize).
    /// Must be positive and fit the runtime clock. It is not required to exceed
    /// [`Self::startup_timeout`], but a launch budget smaller than the protocol budget inverts
    /// the intent of having two.
    pub launch_timeout: Duration,
    /// Deadline for protocol work after the child has answered `initialize`: session creation or
    /// restoration, and session configuration. It starts when that answer arrives, so it is not
    /// reduced by a slow launch, and it does not need to allow for one. Expiry fails with
    /// [`AgentError::StartupDeadline`] naming the step that was still waiting. Must be positive
    /// and fit the runtime clock; represented as a Duration, not an integer number of
    /// milliseconds.
    pub startup_timeout: Duration,
    /// None leaves execution unbounded in time (the default policy).
    /// Some sets an explicit total runtime limit, not a stuck-agent detector.
    /// It starts when the request is submitted and runs without restarting
    /// through reading the message's images, prompt writing, and execution:
    /// time spent on a slow image source is time the agent does not get. The
    /// read also has a bound of its own that this value can only shorten; see
    /// `images`.
    /// The duration must be positive and fit the runtime clock; expiry cancels
    /// the execution and starts process cleanup.
    pub execution_timeout: Option<Duration>,
    /// Positive shared grace interval for cooperative cancellation delivery and process exit.
    /// Initial and fallback cancellation writes consume the same interval; forced cleanup
    /// uses `kill_timeout` afterward. Each mandatory audit call has its own interval, so this
    /// is not a total close deadline. Must fit the runtime clock.
    pub shutdown_grace: Duration,
    /// Positive wait interval for forced process cleanup and child reaping. Cleanup may use this
    /// bound for multiple stages; it is not a total shutdown deadline. Must fit the runtime
    /// clock.
    pub kill_timeout: Duration,
    /// Maximum number of buffered execution events, from 1 through 4096 inclusive. A full event
    /// buffer fails execution rather than silently dropping audit-relevant updates. All process
    /// generations also share a 32 MiB budget for queued event storage and decoded payloads.
    /// An event exceeding the remaining byte budget fails with Backpressure even with free slots.
    /// Dequeue/drop releases its charge; consumer-retained events, domain state, decoding,
    /// audit copies, and allocator/channel overhead are outside this queue budget.
    pub event_capacity: usize,
    /// Maximum size in bytes of a frame this host writes, from 1024 through 16 MiB
    /// inclusive. An oversized outgoing frame fails transport processing. A user message
    /// that cannot fit one frame once encoded is refused when it is submitted, before it is
    /// accepted, with `AgentError::MessageTooLarge`; about 2 KiB of every frame is reserved
    /// for the request around the message. Writing a frame may take one second, plus one
    /// more for each whole mebibyte of it. What the agent may send is bounded separately by
    /// `max_incoming_frame_bytes`.
    pub max_frame_bytes: usize,
    /// Maximum size in bytes of a frame this host accepts from the agent process, from 1024
    /// through 16 MiB inclusive. An oversized incoming frame fails transport processing.
    ///
    /// This is the buffer the agent subprocess can make the host allocate for one frame, so
    /// it is set from the largest answer an agent is expected to send rather than from the
    /// largest prompt this host writes: raising `max_frame_bytes` to carry images does not
    /// have to raise what an agent can demand. Incoming frames also share a fixed limit of
    /// 65,536 JSON values and object keys, including ignored fields, to bound collection
    /// allocation before envelope validation.
    pub max_incoming_frame_bytes: usize,
    /// Where the bytes of a user message's images come from. `None` means this
    /// process cannot deliver images, so its binding offers no image input. With
    /// a source, an image is still sent only to an agent that advertised
    /// `promptCapabilities.image`, and one encoded message must fit
    /// `max_frame_bytes`: base64 grows image bytes by a third.
    ///
    /// The source is read on the task that submitted the message, never on the
    /// task that owns the agent process, so a slow source delays that one
    /// message and nothing else: the active execution, permission answers, and
    /// close all go on. All the images of one message are given ten seconds
    /// together, less when `execution_timeout` (or, for native steering, the
    /// five-second steering deadline) is shorter, and ten seconds even when
    /// `execution_timeout` is `None`. Past that, or when the source panics, the
    /// message fails with `UserImageError::Unavailable`, nothing is sent, and
    /// the context stays usable. Closing the context abandons the read.
    ///
    /// Reading is not free of the operation's own deadline: it spends that
    /// deadline rather than adding to it, so an execution bounded by
    /// `execution_timeout`, and a native steering, are finished or failed
    /// within the interval they were promised. Writing the frame may still take
    /// the extra time its size is allowed.
    ///
    /// Encoded bytes wait in the session's command queue until the worker turns
    /// them into a frame, so one session holds at most twice `max_frame_bytes`
    /// of them at a time. A message whose images would exceed that waits behind
    /// nothing and is refused with `AgentError::Busy` before a byte is read.
    pub images: Option<Arc<dyn UserImageSource>>,
    /// Where every protocol deadline above is measured — `launch_timeout`,
    /// `startup_timeout`, `execution_timeout`, the steering acknowledgement,
    /// cooperative cancellation and audit within `shutdown_grace`, an image
    /// read, a frame's write allowance, and session deletion's.
    /// [`RuntimeClock`](crate::infrastructure::clock::RuntimeClock) outside
    /// tests. Waiting for the process itself to exit is not measured on it:
    /// see [`crate::infrastructure::clock`].
    pub clock: Arc<dyn Clock>,
}
impl AcpConfig {
    /// The ACP `mcpServers` entries for `session/new` and `session/resume`:
    /// each trusted server as configured, with this open's environment. The
    /// one statement of the entry every profile sends.
    pub(crate) fn mcp_server_entries(&self) -> Vec<serde_json::Value> {
        let environment: Vec<_> = self
            .stand_ins
            .environment()
            .iter()
            .map(|(name, value)| serde_json::json!({ "name": name, "value": value }))
            .collect();
        self.mcp_servers
            .current()
            .iter()
            .map(|server| {
                serde_json::json!({
                    "name": server.name,
                    "command": server.command,
                    "args": server.args,
                    "env": environment,
                })
            })
            .collect()
    }
    /// The longest an ACP binding's
    /// [`ProviderSessionDeleter::delete_session`](crate::application::agent_execution::providers::ProviderSessionDeleter::delete_session)
    /// takes to answer with these budgets: `launch_timeout` (until
    /// `initialize` is answered) + `startup_timeout` (the delete) +
    /// `startup_timeout` (every `session/list` page, for the workspace, read
    /// only after a refusal) + `shutdown_grace` + 4 ×
    /// `kill_timeout`. Stopping the process waits out the grace, then a
    /// terminate, a kill, and reaping, each for at most `kill_timeout`; a
    /// binding that launches in a private directory then releases it within
    /// one more. Bindings without one finish a `kill_timeout` sooner; this is
    /// the bound for all of them. A launch that fails spends at most one
    /// `kill_timeout` releasing what it made, well inside it.
    ///
    /// The one statement of that sum in code. Nessa's gateway publishes its
    /// whole delete bound from it — its own waits plus this — and a gateway
    /// test holds the published number to this function.
    pub fn session_deletion_limit(&self) -> Duration {
        self.launch_timeout + self.startup_timeout * 2 + self.shutdown_grace + self.kill_timeout * 4
    }
    pub(crate) fn validate(&self) -> Result<(), AgentError> {
        let mcp_servers = self.mcp_servers.current();
        if let Some(problem) = StdioMcpServer::problem_in(mcp_servers.iter()) {
            return Err(AgentError::Configuration(problem.to_string()));
        }
        if !self.tools_enabled && !mcp_servers.is_empty() {
            return Err(AgentError::Configuration(
                "MCP servers require tools enabled".into(),
            ));
        }
        if !cfg!(unix) {
            return Err(AgentError::Unsupported("native ACP process supervision requires Unix; Windows needs an owned Job Object adapter".into()));
        }
        if self
            .environment
            .keys()
            .any(|key| self.credential_environment.contains_key(key))
        {
            return Err(AgentError::Configuration(
                "context and credential environment keys must be disjoint".into(),
            ));
        }
        if !self.executable.executable().is_absolute() || !self.workspace.is_absolute() {
            return Err(AgentError::Configuration(
                "executable and workspace must be absolute paths".into(),
            ));
        }
        if self.workspace.to_str().is_none() {
            return Err(AgentError::Configuration(
                "ACP workspace must be valid UTF-8".into(),
            ));
        }
        if [
            self.launch_timeout,
            self.startup_timeout,
            self.shutdown_grace,
            self.kill_timeout,
        ]
        .iter()
        .chain(self.execution_timeout.iter())
        .any(|duration| {
            duration.is_zero() || tokio::time::Instant::now().checked_add(*duration).is_none()
        }) || !(1..=4096).contains(&self.event_capacity)
            || !(1024..=16 * 1024 * 1024).contains(&self.max_frame_bytes)
            || !(1024..=16 * 1024 * 1024).contains(&self.max_incoming_frame_bytes)
        {
            return Err(AgentError::Configuration(
                "positive deadlines and bounded frame/event capacities are required".into(),
            ));
        }
        Ok(())
    }
}
