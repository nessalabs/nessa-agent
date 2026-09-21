use std::fmt;
use std::path::PathBuf;

use super::ports::{ArchiveSource, RuntimeStore, SourceFailure, StagedArchive, StoreFailure};
use crate::agent_install::domain::{
    AgentName, ArchiveRejected, HostPlatform, PinnedRelease, ReleaseVersion,
};

/// An agent runtime that is on this machine and ready to launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledRuntime {
    /// The version now installed, which is always the pinned one.
    pub version: ReleaseVersion,
    /// The executable to launch.
    pub executable: PathBuf,
    /// Whether this call fetched the archive.
    ///
    /// Reported rather than inferred from timing, because it is the difference
    /// between "downloaded a hundred megabytes" and "looked at a directory",
    /// and a surface that says "Installed Opencode" for the second is lying to
    /// someone who just watched it take no time at all.
    ///
    /// The archive is fetched before the store excludes other installs, so a
    /// call that downloaded and then found the artifact already published by
    /// another install reports `true`: it did the waiting, whoever did the
    /// unpacking. Which of the two put the file there is not a distinction
    /// anybody watching can see, and not one worth reporting.
    pub downloaded: bool,
}

/// Why an agent runtime is not installed.
///
/// The three cases are kept apart because they are three different things to
/// tell a person: the network did not cooperate, this machine could not hold
/// the file, or what arrived was not what Nessa pinned. Only the last is a
/// reason to stop rather than to offer a retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallFailure {
    /// The release offered is not one this machine can run. Carries the
    /// machine rather than the release, because that is the part the person
    /// reading it has and can act on.
    UnsupportedPlatform(HostPlatform),
    /// The archive could not be fetched.
    Download(SourceFailure),
    /// What arrived was not the archive that was pinned. Nothing is installed.
    Rejected(ArchiveRejected),
    /// This machine could not read, write, or unpack the runtime.
    Store(StoreFailure),
}

impl fmt::Display for InstallFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform(host) => {
                write!(f, "no tested release for {host}")
            }
            Self::Download(failure) => failure.fmt(f),
            Self::Rejected(rejection) => rejection.fmt(f),
            Self::Store(failure) => failure.fmt(f),
        }
    }
}

impl std::error::Error for InstallFailure {}

/// Put an agent's own runtime on this machine, at the version Nessa tested.
///
/// The whole of the ordering rule lives here, and it is short on purpose:
///
/// ```text
/// installed already at the pinned version? -> done, nothing downloaded
/// download -> hash -> the release accepts the hash -> publish
///                                    \-> rejected: discard, install nothing
/// ```
///
/// The hash is checked against the pin *before* anything is unpacked, so an
/// archive that is not the pinned one never has its contents touched — not
/// written to the runtime directory, not read for an entry, not consulted for
/// its version. That ordering is the only thing standing between a replaced
/// download and an executable Nessa will launch, which is why it is in the use
/// case rather than left to an adapter to remember.
///
/// The three middle steps share one open file — see [`StagedArchive`], which
/// also states where that holds and where it would not — so "the bytes that
/// were measured" and "the bytes that were unpacked" are the same bytes by
/// construction rather than by both steps agreeing on a path.
pub struct InstallAgentRuntime<'a> {
    pub source: &'a dyn ArchiveSource,
    pub store: &'a dyn RuntimeStore,
}

impl InstallAgentRuntime<'_> {
    /// Install `agent` from `release`, or report why it is not installed.
    ///
    /// Installing something already installed is not an error and not a
    /// download: the surface that offers this cannot know whether an earlier
    /// attempt finished, and a user who presses it twice should not wait twice.
    ///
    /// **There is no audit port here yet, and there is owed to be one.** This
    /// writes an executable Nessa later launches under the person's own
    /// account, replaces a previous one, and can reject an archive whose bytes
    /// were not the pinned ones — a consequential transition by the hard audit
    /// rule, and a `tracing::info!` and a line of stdout are not a durable
    /// record of it. What that port covers is decided: started, verified,
    /// digest rejected, replaced, rolled back, and the removal of a superseded
    /// artifact, with a test on a failing sink, modelled on `RetirementAudit`
    /// in `desktop_runtime`. Reclaiming those superseded artifacts, and the
    /// ordering [`ManagedRuntimes::record`] documents — the new record replaces
    /// the previous one before the last thing that can fail, so a failure
    /// leaves the previous runtime's bytes named by nothing — belong to the
    /// same change.
    ///
    /// It is deliberately not in the change that added this module: a port, a
    /// durable sink and artifact reclamation are a subsystem, and adding one
    /// to the first install path while it is under review is how the rest of
    /// it stops being reviewable. Saying so here is the alternative to a
    /// silence that reads like nobody noticed.
    pub fn execute(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
        host: &HostPlatform,
    ) -> Result<InstalledRuntime, InstallFailure> {
        if !release.runs_on(host) {
            return Err(InstallFailure::UnsupportedPlatform(host.clone()));
        }
        if let Some(runtime) = self.already_installed(agent, release)? {
            return Ok(runtime);
        }
        let mut staged = self.store.stage(agent).map_err(InstallFailure::Store)?;
        let outcome = self.fetch_and_publish(agent, release, &mut staged);
        // The archive has served its purpose either way, and it is the largest
        // thing this operation writes. Discarding it on the failure path too is
        // what keeps a run of refused downloads from filling the disk.
        self.store.discard(staged);
        Ok(InstalledRuntime {
            version: release.version().clone(),
            executable: outcome?,
            downloaded: true,
        })
    }

    /// The runtime already on disk, when it is this release.
    ///
    /// A different version installed is not reused and not deleted here: the
    /// pin moving is an install of the new one, and what happens to the old one
    /// is the store's business.
    fn already_installed(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
    ) -> Result<Option<InstalledRuntime>, InstallFailure> {
        let installed = self
            .store
            .installed(agent, release)
            .map_err(InstallFailure::Store)?;
        Ok(installed.map(|executable| InstalledRuntime {
            version: release.version().clone(),
            executable,
            downloaded: false,
        }))
    }

    /// Download, verify, and unpack — the part that must be undone as a whole
    /// if any step of it fails.
    fn fetch_and_publish(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
        staged: &mut StagedArchive,
    ) -> Result<PathBuf, InstallFailure> {
        self.source
            .download(release.archive_url().as_str(), staged)
            .map_err(InstallFailure::Download)?;
        let digest = self.store.digest(staged).map_err(InstallFailure::Store)?;
        release.accept(&digest).map_err(InstallFailure::Rejected)?;
        self.store
            .publish(agent, release, staged)
            .map_err(InstallFailure::Store)
    }
}

#[cfg(test)]
#[path = "../../../tests/agent_install/install.rs"]
mod tests;
