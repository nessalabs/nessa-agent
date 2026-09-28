//! The authenticated surface's narrow installation boundary. Composition chooses
//! pins and effects; requests may name an agent, never a URL or executable path.
use super::{InstallFailure, InstalledRuntime};
use crate::agent_install::domain::{AgentName, InstallRequest};

pub struct InstallationOffer {
    pub agent: AgentName,
    pub version: String,
    pub archive_bytes: u64,
    pub installed: bool,
}

pub enum GatewayInstallFailure {
    Unavailable,
    Unsupported,
    Install(Box<InstallFailure>),
}

pub trait AgentInstallations: Send + Sync {
    /// Verified operating-system account owning this private installation namespace.
    fn account_id(&self) -> &str;
    fn offers(&self) -> Result<Vec<InstallationOffer>, GatewayInstallFailure>;
    fn install(
        &self,
        agent: &AgentName,
        request: &InstallRequest,
    ) -> Result<InstalledRuntime, GatewayInstallFailure>;
}
