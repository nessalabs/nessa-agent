//! Resolve the launch for an agent Nessa installs instead of bundles.
//!
//! The runtime store is the authority: a configured path is never trusted on
//! its own. Composition asks about the release selected for this host and only
//! publishes the executable the store verified for that exact pin.
//!
//! ```text
//! pinned releases + host -> preferred release -> RuntimeStore::managed_launch
//!                                                    |
//!                         installed / missing / unreadable
//! ```
//!
//! The arrows mean "is used to decide". Missing, unsupported and unreadable
//! remain separate answers so callers cannot present an I/O failure as proof
//! that no runtime was installed.

use std::{path::Path, sync::Arc};

use crate::agent_install::application::{
    ManagedExecutableUseAdmissionOwner, ManagedExecutableUseGuard, ManagedLaunchSnapshot,
    RuntimeStore, StoreFailure,
};
use crate::agent_install::domain::{preferred_release, AgentName, HostPlatform};
use crate::agent_install::infrastructure::{releases_for, PinFileError};
use crate::agents::domain::AgentId;
use nessa_sdk::application::agent_execution::providers::{
    ExecutableUse, ExecutableUseAdmissionFailure, ExecutableUseError, ExecutableUseGuard,
    ExecutableUseSnapshot,
};

const OPENCODE_ACP_SUBCOMMAND: &str = "acp";

/// What the current pin and installed-runtime store say about a launch.
#[derive(Debug, Clone)]
pub(super) enum InstalledLaunch {
    /// The store verified this executable for the current preferred release.
    Ready(ExecutableUseSnapshot),
    /// The current preferred release has no installation record.
    Missing,
    /// This build pins no release that runs on this host.
    UnsupportedHost,
    /// The store had an answer to read but could not read it.
    Unknown(StoreFailure),
}

impl PartialEq for InstalledLaunch {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Ready(left), Self::Ready(right)) => left.executable() == right.executable(),
            (Self::Missing, Self::Missing) | (Self::UnsupportedHost, Self::UnsupportedHost) => true,
            (Self::Unknown(left), Self::Unknown(right)) => left == right,
            _ => false,
        }
    }
}

impl Eq for InstalledLaunch {}

/// Why the compiled release description could not be interpreted.
#[derive(Debug)]
pub(super) enum LaunchError {
    Pins(PinFileError),
    Name(String),
    ManagedUse(String),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pins(error) => write!(out, "pinned releases could not be read: {error}"),
            Self::Name(name) => write!(out, "{name} is not a valid installed-agent name"),
            Self::ManagedUse(error) => write!(out, "managed launch authority is invalid: {error}"),
        }
    }
}

/// Resolve only the executable the current pin identifies for this host.
pub(super) fn installed_launch(
    agent: AgentId,
    host: &HostPlatform,
    store: &dyn RuntimeStore,
) -> Result<InstalledLaunch, LaunchError> {
    let (name, Some(release)) = preferred(agent, host)? else {
        return Ok(InstalledLaunch::UnsupportedHost);
    };
    match store.managed_launch(&name, &release) {
        Ok(Some(executable)) => managed_launch(executable)
            .map(InstalledLaunch::Ready)
            .map_err(|error| LaunchError::ManagedUse(error.to_string())),
        Ok(None) => Ok(InstalledLaunch::Missing),
        Err(failure) => Ok(InstalledLaunch::Unknown(failure)),
    }
}

/// Whether this build has a pinned managed release for `agent` on `host`.
///
/// This consults only the compiled pin description. It performs no runtime
/// store or credential effect, so composition can exclude an unsupported host
/// before readiness or provider resolution asks either boundary.
pub(super) fn supports_installed_launch(
    agent: AgentId,
    host: &HostPlatform,
) -> Result<bool, LaunchError> {
    preferred(agent, host).map(|(_, release)| release.is_some())
}

fn preferred(
    agent: AgentId,
    host: &HostPlatform,
) -> Result<
    (
        AgentName,
        Option<crate::agent_install::domain::PinnedRelease>,
    ),
    LaunchError,
> {
    let name =
        AgentName::parse(agent.name()).map_err(|_| LaunchError::Name(agent.name().into()))?;
    let releases = releases_for(&name).map_err(LaunchError::Pins)?;
    let release = preferred_release(releases, host);
    Ok((name, release))
}

struct SdkExecutableUseBridge {
    snapshot: ManagedLaunchSnapshot,
}

impl ExecutableUse for SdkExecutableUseBridge {
    fn executable(&self) -> &Path {
        self.snapshot.executable()
    }

    fn admit(&self) -> Result<Box<dyn ExecutableUseGuard>, ExecutableUseAdmissionFailure> {
        match self.snapshot.admit() {
            Ok(guard) => Ok(Box::new(SdkExecutableUseGuardBridge(guard))),
            Err(failure) => {
                let (error, owner) = failure.into_parts();
                let error = ExecutableUseError::new(error.to_string());
                Err(match owner {
                    Some(ManagedExecutableUseAdmissionOwner::Confirmed(guard)) => {
                        ExecutableUseAdmissionFailure::with_confirmed_generation(
                            error,
                            Box::new(SdkExecutableUseGuardBridge(guard)),
                        )
                    }
                    Some(ManagedExecutableUseAdmissionOwner::Uncertain(guard)) => {
                        ExecutableUseAdmissionFailure::with_uncertain_generation(
                            error,
                            Box::new(SdkExecutableUseGuardBridge(guard)),
                        )
                    }
                    None => ExecutableUseAdmissionFailure::without_owner(error),
                })
            }
        }
    }
}

struct SdkExecutableUseGuardBridge(Box<dyn ManagedExecutableUseGuard>);

impl ExecutableUseGuard for SdkExecutableUseGuardBridge {
    fn release(&mut self) -> Result<(), ExecutableUseError> {
        self.0
            .release()
            .map_err(|error| ExecutableUseError::new(error.to_string()))
    }
}

fn managed_launch(
    snapshot: ManagedLaunchSnapshot,
) -> Result<ExecutableUseSnapshot, ExecutableUseError> {
    let executable = snapshot.executable().to_owned();
    ExecutableUseSnapshot::new(executable, Arc::new(SdkExecutableUseBridge { snapshot }))
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
