//! Resolve the launch for an agent Nessa installs instead of bundles.
//!
//! The runtime store is the authority: a configured path is never trusted on
//! its own. Composition asks about the release selected for this host and only
//! publishes the executable the store verified for that exact pin.
//!
//! ```text
//! pinned releases + host -> preferred release -> RuntimeStore::installed
//!                                                    |
//!                         installed / missing / unreadable
//! ```
//!
//! The arrows mean "is used to decide". Missing, unsupported and unreadable
//! remain separate answers so callers cannot present an I/O failure as proof
//! that no runtime was installed.

use std::path::PathBuf;

use crate::agent_install::application::{RuntimeStore, StoreFailure};
use crate::agent_install::domain::{preferred_release, AgentName, HostPlatform};
use crate::agent_install::infrastructure::{releases_for, PinFileError};
use crate::agents::domain::AgentId;

const OPENCODE_ACP_SUBCOMMAND: &str = "acp";

/// What the current pin and installed-runtime store say about a launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum InstalledLaunch {
    /// The store verified this executable for the current preferred release.
    Ready(PathBuf),
    /// The current preferred release has no installation record.
    Missing,
    /// This build pins no release that runs on this host.
    UnsupportedHost,
    /// The store had an answer to read but could not read it.
    Unknown(StoreFailure),
}

/// Why the compiled release description could not be interpreted.
#[derive(Debug)]
pub(super) enum LaunchError {
    Pins(PinFileError),
    Name(String),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pins(error) => write!(out, "pinned releases could not be read: {error}"),
            Self::Name(name) => write!(out, "{name} is not a valid installed-agent name"),
        }
    }
}

/// Resolve only the executable the current pin identifies for this host.
pub(super) fn installed_launch(
    agent: AgentId,
    host: &HostPlatform,
    store: &dyn RuntimeStore,
) -> Result<InstalledLaunch, LaunchError> {
    let name =
        AgentName::parse(agent.name()).map_err(|_| LaunchError::Name(agent.name().into()))?;
    let releases = releases_for(&name).map_err(LaunchError::Pins)?;
    let Some(release) = preferred_release(releases, host) else {
        return Ok(InstalledLaunch::UnsupportedHost);
    };
    match store.installed(&name, &release) {
        Ok(Some(executable)) => Ok(InstalledLaunch::Ready(executable)),
        Ok(None) => Ok(InstalledLaunch::Missing),
        Err(failure) => Ok(InstalledLaunch::Unknown(failure)),
    }
}

/// The arguments for an installed agent's ACP entry point.
///
/// Opencode uses an `acp` subcommand. Bundled agents are included explicitly so
/// moving one to the managed store cannot silently inherit Opencode's command.
pub(super) fn installed_arguments(agent: AgentId) -> Vec<String> {
    match agent {
        AgentId::Opencode => vec![OPENCODE_ACP_SUBCOMMAND.to_owned()],
        AgentId::Claude | AgentId::Codex => Vec::new(),
    }
}

#[cfg(test)]
#[path = "../../tests/composition/installed_launch.rs"]
mod tests;
