use crate::gateway::domain::value_objects::{SearchPath, SearchPathError};
use std::{error::Error, fmt, path::Path};

/// Exact native runtime incarnation established by successful reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciledGateway {
    service: String,
    runtime_fingerprint: String,
    runtime_instance: String,
    service_generation: String,
    process_id: u32,
    /// Loopback port this service answers on, decided by its stage. Carried so
    /// a later health probe asks the socket that was registered rather than
    /// re-deriving one that could disagree.
    port: u16,
}
// The identity itself is portable evidence carried by the `GatewayHost`
// contract on every target. Reading its parts is what one native adapter does,
// and macOS is the only host that manages a background service today.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl ReconciledGateway {
    pub fn new(
        service: String,
        runtime_fingerprint: String,
        runtime_instance: String,
        service_generation: String,
        process_id: u32,
        port: u16,
    ) -> Self {
        Self {
            service,
            runtime_fingerprint,
            runtime_instance,
            service_generation,
            process_id,
            port,
        }
    }
    pub fn service(&self) -> &str {
        &self.service
    }
    pub fn runtime_fingerprint(&self) -> &str {
        &self.runtime_fingerprint
    }
    pub fn runtime_instance(&self) -> &str {
        &self.runtime_instance
    }
    pub fn service_generation(&self) -> &str {
        &self.service_generation
    }
    pub fn process_id(&self) -> u32 {
        self.process_id
    }
    pub fn port(&self) -> u16 {
        self.port
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayError {
    Registration(String),
    NotReconciled,
    Stop(String),
}
impl fmt::Display for GatewayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registration(message) | Self::Stop(message) => f.write_str(message),
            Self::NotReconciled => f.write_str("gateway service has not reconciled"),
        }
    }
}
impl Error for GatewayError {}
/// Why the user's login shell did not produce a search path.
///
/// Kept apart from [`GatewayError`]: none of these stop a registration. They
/// are what the fallback to the system path is reported as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginShellError {
    /// The shell could not be started, or did not exit successfully.
    Unavailable(String),
    /// The shell was still running when its deadline passed and was stopped. A
    /// login file that waits for input or never returns lands here.
    TimedOut,
    /// The shell answered with something that is not a search path.
    Rejected(SearchPathError),
}
impl fmt::Display for LoginShellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(message) => write!(f, "login shell unavailable: {message}"),
            Self::TimedOut => f.write_str("login shell did not answer before its deadline"),
            Self::Rejected(error) => {
                write!(f, "login shell answered with an unusable path: {error}")
            }
        }
    }
}
impl Error for LoginShellError {}

/// The `PATH` the user's own login shell would give them.
///
/// The agent is meant to reach the tools the user installed, and the desktop
/// host is the part of Nessa that runs inside that user's session — so it is the
/// part that can ask. A login shell is user-controlled code: the implementation
/// runs it with a bounded deadline and a clean environment, and the value it
/// returns has been through [`SearchPath::parse`].
///
/// One resolution per registration, so the answer is fixed in the service
/// definition and changes only by re-registering.
pub trait LoginShellPath: Send + Sync {
    fn resolve(&self) -> Result<SearchPath, LoginShellError>;
}

/// Reconciliation returns the exact native runtime incarnation only after matching readiness. A stop
/// acknowledges request delivery, not the eventual physical cleanup of each agent.
pub trait GatewayHost: Send + Sync {
    /// Registers the service for `stage`, running the staged `runtime`.
    ///
    /// `agent_path` is the search path resolved for the agent this launch, or
    /// `None` when the login shell could not be read. `None` is not "use the
    /// system path": an adapter that already registered a service keeps the
    /// path that service was registered with, so one slow login shell does not
    /// rewrite the service definition and retire a healthy gateway.
    fn register(
        &self,
        runtime: &Path,
        stage: &str,
        agent_path: Option<&SearchPath>,
    ) -> Result<ReconciledGateway, GatewayError>;
    fn stop_agents(&self, gateway: &ReconciledGateway) -> Result<(), GatewayError>;
}
