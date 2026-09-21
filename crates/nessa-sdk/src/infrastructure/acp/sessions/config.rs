//! Trusted host configuration for launching and supervising an ACP process.
#![deny(missing_docs)]

use crate::application::agent_execution::{agents::AgentError, providers::UserImageSource};
use crate::domain::agent_execution::permissions::PermissionOfferPolicy;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsString,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

/// Trusted stdio MCP server. This is host configuration, never model-supplied input.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StdioMcpServer {
    /// Unique ASCII server name (letters, digits, hyphen, underscore; at most 64 bytes).
    pub name: String,
    /// Absolute UTF-8 executable path, launched directly without shell interpolation.
    pub command: PathBuf,
    /// Ordered UTF-8 arguments. Never put credentials here; these enter the context fingerprint.
    #[serde(default)]
    pub args: Vec<String>,
}

/// Host-owned launch configuration. Environment is explicit, never inherited by
/// the adapter. Use the normal HOME/auth environment without extracting secrets.
/// Only trusted composition may choose the executable and its arguments.
#[derive(Clone)]
pub struct AcpConfig {
    /// Absolute path to the trusted provider executable. The adapter launches it directly,
    /// without a shell.
    pub executable: PathBuf,
    /// Noncredential arguments passed verbatim in order. These select restoration context;
    /// never place secrets in arguments. Only trusted composition may supply them.
    pub arguments: Vec<OsString>,
    /// Noncredential context-selecting environment, including HOME, configuration directories,
    /// endpoints, and executable search paths. These values enter the restoration fingerprint.
    /// Parent variables are cleared; this map and credential_environment supply the child.
    pub environment: BTreeMap<OsString, OsString>,
    /// Host-supplied credentials forwarded verbatim, without discovery or extraction.
    /// Excluded from restoration identity so credentials may rotate. Do not put context
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
    /// Trusted MCP servers exposed by profiles that support MCP. Empty disables custom tools.
    /// Servers require tools_enabled and share the provider session lifetime.
    pub mcp_servers: Vec<StdioMcpServer>,
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
}
impl AcpConfig {
    pub(crate) fn validate(&self) -> Result<(), AgentError> {
        if self.mcp_servers.len() > 16 || (!self.tools_enabled && !self.mcp_servers.is_empty()) {
            return Err(AgentError::Configuration(
                "at most 16 MCP servers; MCP requires tools enabled".into(),
            ));
        }
        let mut names = HashSet::new();
        for server in &self.mcp_servers {
            if server.name.is_empty()
                || server.name.len() > 64
                || server.name.contains("__")
                || !server
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                || !names.insert(&server.name)
                || !server.command.is_absolute()
                || server.command.to_str().is_none()
                || server.args.len() > 64
                || server
                    .args
                    .iter()
                    .any(|arg| arg.len() > 8192 || arg.contains('\0'))
            {
                return Err(AgentError::Configuration(
                    "invalid MCP server name, executable or arguments".into(),
                ));
            }
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
        if !self.executable.is_absolute() || !self.workspace.is_absolute() {
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
