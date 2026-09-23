use std::fmt;
use std::path::PathBuf;

use super::ports::{
    ArchiveSource, AuditFailure, InstallAudit, PublicationChange, PublicationCleanupFailure,
    PublicationRecovery, RollbackChange, RuntimeStore, SourceFailure, StagedArchive, StoreFailure,
};
use crate::agent_install::domain::{
    AgentName, ArchiveRejected, HostPlatform, InstallAttempt, InstallAttemptError,
    InstallFailureEvidence, InstallFailureKind, InstallRequest, InstallTransition, PinnedRelease,
    RecoveryFailureEvidence, RecoveryState, ReleaseVersion, RollbackState, RuntimeArtifact,
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
    /// A store result contradicted the legal install sequence.
    Evidence(InstallAttemptError),
    /// Durable evidence was not acknowledged. `operation` preserves an
    /// installation failure that happened too, and `runtime_state` retains
    /// exactly what was established before the audit attempt.
    Audit {
        operation: Option<Box<InstallFailure>>,
        runtime_state: RuntimeStateEvidence,
        failure: AuditFailure,
    },
    /// Publication failed and its cleanup also failed. The original operation
    /// and cleanup failures remain separate facts.
    Recovery {
        operation: StoreFailure,
        cleanup: Box<PublicationCleanupFailure>,
    },
}

/// Installed-state evidence retained when audit acknowledgement fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeStateEvidence {
    /// No publication had started, so the operation did not change the state.
    Unchanged,
    /// The requested target was durably published.
    TargetInstalled,
    /// The prior artifact was durably restored.
    Restored(RuntimeArtifact),
    /// Cleanup confirmed that no runtime record remains installed.
    NoInstalledRuntime,
    /// Cleanup could not establish the remaining installed state.
    Unconfirmed,
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
            Self::Evidence(failure) => failure.fmt(f),
            Self::Audit {
                operation,
                runtime_state,
                failure,
            } => {
                if let Some(operation) = operation {
                    write!(f, "{operation}; install audit was not committed: {failure}")
                } else if *runtime_state == RuntimeStateEvidence::TargetInstalled {
                    write!(
                        f,
                        "the runtime was installed, but its audit record was not committed: {failure}"
                    )
                } else {
                    write!(f, "install audit was not committed: {failure}")
                }
            }
            Self::Recovery { operation, cleanup } => {
                write!(f, "{operation}; publication cleanup also failed: {cleanup}")
            }
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
/// download (bounded by the pinned length) -> hash -> the release accepts the
///     hash -> publish
///                                    \-> rejected: discard, install nothing
/// ```
///
/// The hash is checked against the pin *before* anything is unpacked, so an
/// archive that is not the pinned one never has its contents touched — not
/// written to the runtime directory, not read for an entry, not consulted for
/// its version. That ordering is the only thing standing between a replaced
/// download and an executable Nessa will launch, which is why it is in the use
/// case rather than left to an adapter to remember. It holds whatever shape the
/// release has: a release that installs seven files is verified once, as the
/// one archive it arrived in, before the first of them is written.
///
/// The three middle steps share one open file — see [`StagedArchive`], which
/// also states where that holds and where it would not — so "the bytes that
/// were measured" and "the bytes that were unpacked" are the same bytes by
/// construction rather than by both steps agreeing on a path.
pub struct InstallAgentRuntime<'a> {
    pub source: &'a dyn ArchiveSource,
    pub store: &'a dyn RuntimeStore,
    pub audit: &'a dyn InstallAudit,
}

impl InstallAgentRuntime<'_> {
    /// Install `agent` from `release`, or report why it is not installed.
    ///
    /// Installing something already installed is not an error and not a
    /// download: the surface that offers this cannot know whether an earlier
    /// attempt finished, and a user who presses it twice should not wait twice.
    ///
    /// Every consequential transition is passed to [`InstallAudit`] before
    /// this call reports its outcome. Publication and rollback retain the
    /// store's per-agent lease through that acknowledgement, so two concurrent
    /// installs cannot report their effects in an order that contradicts the
    /// before/after evidence captured under the publication lock.
    pub fn execute(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
        host: &HostPlatform,
        request: &InstallRequest,
    ) -> Result<InstalledRuntime, InstallFailure> {
        if !release.runs_on(host) {
            return Err(InstallFailure::UnsupportedPlatform(host.clone()));
        }
        let target = RuntimeArtifact::for_release(release);
        let (mut attempt, started) = InstallAttempt::start(agent.clone(), target, request.clone());
        self.audit(started)?;
        if let Some(runtime) = self.already_installed(agent, release)? {
            return Ok(runtime);
        }
        let mut staged = self.store.stage(agent).map_err(InstallFailure::Store)?;
        let outcome = self.fetch_and_publish(agent, release, &mut attempt, &mut staged);
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
        attempt: &mut InstallAttempt,
        staged: &mut StagedArchive,
    ) -> Result<PathBuf, InstallFailure> {
        self.source
            .download(
                release.archive_url().as_str(),
                release.archive_size().bytes(),
                staged,
            )
            .map_err(InstallFailure::Download)?;
        let digest = self.store.digest(staged).map_err(InstallFailure::Store)?;
        if let Err(rejection) = release.accept(&digest) {
            let operation = InstallFailure::Rejected(rejection);
            let transition = attempt.rejected(digest).map_err(InstallFailure::Evidence)?;
            return match self.audit.record(transition) {
                Ok(()) => Err(operation),
                Err(failure) => Err(with_audit_failure(
                    operation,
                    RuntimeStateEvidence::Unchanged,
                    failure,
                )),
            };
        }
        self.audit(attempt.verified().map_err(InstallFailure::Evidence)?)?;
        match self.store.publish(agent, release, staged) {
            Ok(publication) => {
                let transition = match publication.change() {
                    PublicationChange::Reused => {
                        attempt.reused().map_err(InstallFailure::Evidence)?;
                        return Ok(publication.executable().to_owned());
                    }
                    PublicationChange::Installed => {
                        attempt.installed().map_err(InstallFailure::Evidence)?
                    }
                    PublicationChange::Replaced(previous) => attempt
                        .replaced(previous.clone())
                        .map_err(InstallFailure::Evidence)?,
                };
                self.audit
                    .record(transition)
                    .map_err(|failure| InstallFailure::Audit {
                        operation: None,
                        runtime_state: RuntimeStateEvidence::TargetInstalled,
                        failure,
                    })?;
                Ok(publication.executable().to_owned())
            }
            Err(publish) => {
                let operation = InstallFailure::Store(publish.failure().clone());
                let (transition, runtime_state, outcome) = match publish.recovery() {
                    PublicationRecovery::NotRequired => return Err(operation),
                    PublicationRecovery::RolledBack(rollback) => {
                        let restored = rollback_state(rollback);
                        let runtime_state = runtime_state(rollback);
                        let transition = attempt
                            .rolled_back(restored)
                            .map_err(InstallFailure::Evidence)?;
                        (transition, runtime_state, operation)
                    }
                    PublicationRecovery::Incomplete { rollback, cleanup } => {
                        let state = rollback
                            .as_ref()
                            .map(|rollback| RecoveryState::Confirmed(rollback_state(rollback)))
                            .unwrap_or(RecoveryState::Unconfirmed);
                        let runtime_state = rollback
                            .as_ref()
                            .map(runtime_state)
                            .unwrap_or(RuntimeStateEvidence::Unconfirmed);
                        let failures = recovery_failures(publish.failure(), cleanup);
                        let transition = attempt
                            .recovery_incomplete(state, failures)
                            .map_err(InstallFailure::Evidence)?;
                        let outcome = InstallFailure::Recovery {
                            operation: publish.failure().clone(),
                            cleanup: Box::new(cleanup.clone()),
                        };
                        (transition, runtime_state, outcome)
                    }
                };
                match self.audit.record(transition) {
                    Ok(()) => Err(outcome),
                    Err(failure) => Err(with_audit_failure(outcome, runtime_state, failure)),
                }
            }
        }
    }

    fn audit(&self, transition: InstallTransition) -> Result<(), InstallFailure> {
        self.audit
            .record(transition)
            .map_err(|failure| InstallFailure::Audit {
                operation: None,
                runtime_state: RuntimeStateEvidence::Unchanged,
                failure,
            })
    }
}

fn rollback_state(rollback: &RollbackChange) -> RollbackState {
    match rollback {
        RollbackChange::Restored(artifact) => RollbackState::Restored(artifact.clone()),
        RollbackChange::NoInstalledRuntime => RollbackState::NoInstalledRuntime,
    }
}

fn runtime_state(rollback: &RollbackChange) -> RuntimeStateEvidence {
    match rollback {
        RollbackChange::Restored(artifact) => RuntimeStateEvidence::Restored(artifact.clone()),
        RollbackChange::NoInstalledRuntime => RuntimeStateEvidence::NoInstalledRuntime,
    }
}

fn recovery_failures(
    publication: &StoreFailure,
    cleanup: &PublicationCleanupFailure,
) -> RecoveryFailureEvidence {
    RecoveryFailureEvidence::new(
        failure_evidence(publication),
        cleanup.withdrawal().map(failure_evidence),
        cleanup.restoration().map(failure_evidence),
        cleanup.confirmation().map(failure_evidence),
    )
    .expect("publication cleanup failure always contains at least one failure")
}

fn failure_evidence(failure: &StoreFailure) -> InstallFailureEvidence {
    let (kind, detail) = match failure {
        StoreFailure::Unwritable(detail) => (InstallFailureKind::Unwritable, detail),
        StoreFailure::Unreadable(detail) => (InstallFailureKind::Unreadable, detail),
        StoreFailure::MissingExecutable(detail) => (InstallFailureKind::MissingExecutable, detail),
        StoreFailure::MalformedArchive(detail) => (InstallFailureKind::MalformedArchive, detail),
    };
    InstallFailureEvidence::new(kind, detail.clone())
}

fn with_audit_failure(
    operation: InstallFailure,
    runtime_state: RuntimeStateEvidence,
    failure: AuditFailure,
) -> InstallFailure {
    InstallFailure::Audit {
        operation: Some(Box::new(operation)),
        runtime_state,
        failure,
    }
}

#[cfg(test)]
#[path = "../../../tests/agent_install/install.rs"]
mod tests;
