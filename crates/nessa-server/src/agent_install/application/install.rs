use std::fmt;
use std::path::PathBuf;

use super::ports::{
    ArchiveSource, AuditAcknowledgement, AuditFailure, InstallAudit, InstallDeliveryFailure,
    InstallationDelivery, InstallationDeliverySession, PendingInstallationDelivery,
    PreparedInstallation, PublicationChange, PublicationCleanupFailure, PublicationRecovery,
    RollbackChange, RuntimeStore, SourceFailure, StagedArchive, StoreFailure,
};
use crate::agent_install::domain::{
    AgentName, ArchiveRejected, HostPlatform, InstallAttempt, InstallAttemptError,
    InstallFailureEvidence, InstallFailureKind, InstallRequest, InstallTransition,
    InstallTransitionFacts, PinnedRelease, PublicationOutcome, PublicationPreparation,
    PublicationSettlement, RecoveryFailureEvidence, RecoveryState, ReleaseVersion, RollbackState,
    RuntimeArtifact,
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
    /// This request identity already began an attempt and must not repeat its
    /// install effects. A new user invocation needs a new request identity;
    /// an audit failure is retried through [`InstallAgentRuntime::retry_audit`].
    AttemptReused(InstallRequest),
    /// Durable evidence was not acknowledged. The boxed evidence preserves an
    /// installation failure that happened too and the exact runtime state and
    /// transition established before the audit attempt.
    Audit(Box<AuditDeliveryFailure>),
    /// Publication failed and its cleanup also failed. The original operation
    /// and cleanup failures remain separate facts.
    Recovery {
        operation: StoreFailure,
        cleanup: Box<PublicationCleanupFailure>,
    },
    /// Durable publication delivery could not be completed or reconciled.
    Delivery(Box<PublicationDeliveryFailure>),
    /// A prepared publication has no retained terminal in either durable route.
    UnresolvedPublication(PublicationPreparation),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationDeliveryFailure {
    operation: Option<Box<InstallFailure>>,
    runtime_state: RuntimeStateEvidence,
    terminal: Option<InstallTransition>,
    delivery: Option<InstallDeliveryFailure>,
    audit: Option<AuditFailure>,
}

impl PublicationDeliveryFailure {
    pub fn operation(&self) -> Option<&InstallFailure> {
        self.operation.as_deref()
    }

    pub fn runtime_state(&self) -> &RuntimeStateEvidence {
        &self.runtime_state
    }

    pub fn terminal(&self) -> Option<&InstallTransition> {
        self.terminal.as_ref()
    }

    pub fn delivery(&self) -> Option<&InstallDeliveryFailure> {
        self.delivery.as_ref()
    }

    pub fn audit(&self) -> Option<&AuditFailure> {
        self.audit.as_ref()
    }
}

/// Typed install and runtime facts retained when audit delivery fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditDeliveryFailure {
    operation: Option<Box<InstallFailure>>,
    runtime_state: RuntimeStateEvidence,
    pending: InstallTransition,
    failure: AuditFailure,
}

impl AuditDeliveryFailure {
    /// Return the install failure that also occurred, when there was one.
    pub fn operation(&self) -> Option<&InstallFailure> {
        self.operation.as_deref()
    }

    /// Return the installed state established before audit delivery.
    pub fn runtime_state(&self) -> &RuntimeStateEvidence {
        &self.runtime_state
    }

    /// Return the exact transition that is safe to redeliver.
    pub fn pending(&self) -> &InstallTransition {
        &self.pending
    }

    /// Return why durable acknowledgement failed.
    pub fn failure(&self) -> &AuditFailure {
        &self.failure
    }
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
            Self::AttemptReused(request) => write!(
                f,
                "install request {} has already begun",
                request.request_id()
            ),
            Self::Audit(evidence) => {
                if let Some(operation) = &evidence.operation {
                    write!(
                        f,
                        "{operation}; install audit was not committed: {}",
                        evidence.failure
                    )
                } else if evidence.runtime_state == RuntimeStateEvidence::TargetInstalled {
                    write!(
                        f,
                        "the runtime was installed, but its audit record was not committed: {}",
                        evidence.failure
                    )
                } else {
                    write!(f, "install audit was not committed: {}", evidence.failure)
                }
            }
            Self::Recovery { operation, cleanup } => {
                write!(f, "{operation}; publication cleanup also failed: {cleanup}")
            }
            Self::Delivery(failure) => {
                if let Some(operation) = &failure.operation {
                    write!(f, "{operation}; ")?;
                }
                formatter_delivery_failure(f, failure)
            }
            Self::UnresolvedPublication(preparation) => write!(
                f,
                "install request {} has an unresolved prepared publication",
                preparation.verified().request().request_id()
            ),
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
    pub delivery: &'a dyn InstallationDelivery,
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
    /// store's per-agent lease through that immediate audit attempt. The lease is dropped on return. A later invocation recovers the retained
    /// terminal before admitting any new install effect, so the earlier terminal
    /// is acknowledged before a later attempt can begin.
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
        let mut delivery = self
            .delivery
            .session(request.account_id())
            .map_err(delivery_failure)?;
        self.recover(delivery.as_mut())?;
        let (mut attempt, started) = InstallAttempt::start(agent.clone(), target, request.clone());
        if self.audit(started)? == AuditAcknowledgement::Replayed {
            return Err(InstallFailure::AttemptReused(request.clone()));
        }
        if let Some(runtime) = self.already_installed(agent, release)? {
            return Ok(runtime);
        }
        let mut staged = self.store.stage(agent).map_err(InstallFailure::Store)?;
        let outcome =
            self.fetch_and_publish(agent, release, &mut attempt, &mut staged, delivery.as_mut());
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
        delivery: &mut dyn InstallationDeliverySession,
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
            return match self.audit.record(transition.clone()) {
                Ok(_) => Err(operation),
                Err(failure) => Err(with_audit_failure(
                    operation,
                    RuntimeStateEvidence::Unchanged,
                    transition,
                    failure,
                )),
            };
        }
        let verified = attempt.verified().map_err(InstallFailure::Evidence)?;
        let _ = self.audit(verified.clone())?;
        let preparation = PublicationPreparation::new(verified)
            .map_err(|error| delivery_domain_failure(error.to_string()))?;
        let prepared = delivery
            .prepare(preparation.clone())
            .map_err(delivery_failure)?;
        match self.store.publish(agent, release, staged) {
            Ok(publication) => {
                let transition = match publication.change() {
                    PublicationChange::Reused => {
                        attempt.reused().map_err(InstallFailure::Evidence)?;
                        let outcome = PublicationOutcome::no_publication_effect(preparation);
                        let _ = self.finish_no_effect(
                            delivery,
                            &prepared,
                            outcome,
                            RuntimeStateEvidence::TargetInstalled,
                            None,
                        )?;
                        return Ok(publication.executable().to_owned());
                    }
                    PublicationChange::Installed => {
                        attempt.installed().map_err(InstallFailure::Evidence)?
                    }
                    PublicationChange::Replaced(previous) => attempt
                        .replaced(previous.clone())
                        .map_err(InstallFailure::Evidence)?,
                };
                let outcome = PublicationOutcome::terminal(&preparation, transition.clone())
                    .map_err(|error| delivery_domain_failure(error.to_string()))?;
                let _ = self.finish_terminal(
                    delivery,
                    &prepared,
                    outcome,
                    transition,
                    RuntimeStateEvidence::TargetInstalled,
                    None,
                )?;
                Ok(publication.executable().to_owned())
            }
            Err(publish) => {
                let (failure, recovery, publication_lease) = publish.into_parts();
                let (transition, runtime_state, outcome) = match recovery {
                    PublicationRecovery::NotRequired => {
                        let retained = PublicationOutcome::no_publication_effect(preparation);
                        let operation = InstallFailure::Store(failure);
                        let operation = self
                            .finish_no_effect(
                                delivery,
                                &prepared,
                                retained,
                                RuntimeStateEvidence::Unchanged,
                                Some(operation),
                            )?
                            .expect("the no-effect failure is returned after settlement");
                        drop(publication_lease);
                        return Err(operation);
                    }
                    PublicationRecovery::RolledBack(rollback) => {
                        let restored = rollback_state(&rollback);
                        let runtime_state = runtime_state(&rollback);
                        let transition = attempt
                            .rolled_back(restored)
                            .map_err(InstallFailure::Evidence)?;
                        (transition, runtime_state, InstallFailure::Store(failure))
                    }
                    PublicationRecovery::Incomplete { rollback, cleanup } => {
                        let publication_evidence = failure_evidence(&failure);
                        let state = rollback
                            .as_ref()
                            .map(|rollback| RecoveryState::Confirmed(rollback_state(rollback)))
                            .unwrap_or(RecoveryState::Unconfirmed);
                        let runtime_state = rollback
                            .as_ref()
                            .map(runtime_state)
                            .unwrap_or(RuntimeStateEvidence::Unconfirmed);
                        let failures = recovery_failures(publication_evidence, &cleanup);
                        let transition = attempt
                            .recovery_incomplete(state, failures)
                            .map_err(InstallFailure::Evidence)?;
                        let outcome = InstallFailure::Recovery {
                            operation: failure,
                            cleanup: Box::new(cleanup),
                        };
                        (transition, runtime_state, outcome)
                    }
                };
                let retained = PublicationOutcome::terminal(&preparation, transition.clone())
                    .map_err(|error| delivery_domain_failure(error.to_string()))?;
                let result = self.finish_terminal(
                    delivery,
                    &prepared,
                    retained,
                    transition,
                    runtime_state,
                    Some(outcome),
                );
                drop(publication_lease);
                Err(result?.expect("the publication failure is returned after settlement"))
            }
        }
    }

    fn recover(
        &self,
        delivery: &mut dyn InstallationDeliverySession,
    ) -> Result<(), InstallFailure> {
        let Some(pending) = delivery.pending().map_err(delivery_failure)? else {
            return Ok(());
        };
        let (prepared, outcome, retained) = match pending {
            PendingInstallationDelivery::Outcome { prepared, outcome } => (prepared, outcome, None),
            PendingInstallationDelivery::Prepared(prepared) => {
                let Some(terminal) =
                    self.audit
                        .completion_for(prepared.preparation())
                        .map_err(|audit| {
                            publication_delivery_failure(
                                None,
                                RuntimeStateEvidence::Unconfirmed,
                                None,
                                None,
                                Some(audit),
                            )
                        })?
                else {
                    return Err(InstallFailure::UnresolvedPublication(
                        prepared.preparation().clone(),
                    ));
                };
                let outcome =
                    PublicationOutcome::terminal(prepared.preparation(), terminal.clone())
                        .map_err(|error| delivery_domain_failure(error.to_string()))?;
                let retained = delivery.retain_outcome(&prepared, &outcome).err();
                (prepared, outcome, retained)
            }
        };
        if let Some(terminal) = outcome.terminal_transition() {
            let audited = self.audit.record(terminal.clone()).err();
            if retained.is_some() || audited.is_some() {
                return Err(publication_delivery_failure(
                    None,
                    runtime_state_for_transition(terminal),
                    Some(terminal.clone()),
                    retained,
                    audited,
                ));
            }
        } else if let Some(delivery) = retained {
            return Err(publication_delivery_failure(
                None,
                RuntimeStateEvidence::Unchanged,
                None,
                Some(delivery),
                None,
            ));
        }
        let settlement = PublicationSettlement::new(prepared.preparation(), outcome)
            .map_err(|error| delivery_domain_failure(error.to_string()))?;
        delivery
            .settle(&prepared, &settlement)
            .map_err(delivery_failure)
    }

    fn finish_no_effect(
        &self,
        delivery: &mut dyn InstallationDeliverySession,
        prepared: &PreparedInstallation,
        outcome: PublicationOutcome,
        runtime_state: RuntimeStateEvidence,
        operation: Option<InstallFailure>,
    ) -> Result<Option<InstallFailure>, InstallFailure> {
        if let Err(error) = delivery.retain_outcome(prepared, &outcome) {
            return Err(publication_delivery_failure(
                operation,
                runtime_state,
                None,
                Some(error),
                None,
            ));
        }
        let settlement = PublicationSettlement::new(prepared.preparation(), outcome)
            .map_err(|error| delivery_domain_failure(error.to_string()))?;
        if let Err(error) = delivery.settle(prepared, &settlement) {
            return Err(publication_delivery_failure(
                operation,
                runtime_state,
                None,
                Some(error),
                None,
            ));
        }
        Ok(operation)
    }

    fn finish_terminal(
        &self,
        delivery: &mut dyn InstallationDeliverySession,
        prepared: &PreparedInstallation,
        outcome: PublicationOutcome,
        terminal: InstallTransition,
        runtime_state: RuntimeStateEvidence,
        operation: Option<InstallFailure>,
    ) -> Result<Option<InstallFailure>, InstallFailure> {
        let retained = delivery.retain_outcome(prepared, &outcome).err();
        let audited = self.audit.record(terminal.clone()).err();
        if retained.is_some() || audited.is_some() {
            return Err(publication_delivery_failure(
                operation,
                runtime_state,
                Some(terminal),
                retained,
                audited,
            ));
        }
        let settlement = PublicationSettlement::new(prepared.preparation(), outcome)
            .map_err(|error| delivery_domain_failure(error.to_string()))?;
        if let Err(error) = delivery.settle(prepared, &settlement) {
            return Err(publication_delivery_failure(
                operation,
                runtime_state,
                Some(terminal),
                Some(error),
                None,
            ));
        }
        Ok(operation)
    }

    fn audit(&self, transition: InstallTransition) -> Result<AuditAcknowledgement, InstallFailure> {
        self.audit.record(transition.clone()).map_err(|failure| {
            InstallFailure::Audit(Box::new(AuditDeliveryFailure {
                operation: None,
                runtime_state: RuntimeStateEvidence::Unchanged,
                pending: transition,
                failure,
            }))
        })
    }

    /// Retry only the durable acknowledgement retained by an audit failure.
    ///
    /// This never repeats download, verification, publication, or rollback.
    pub fn retry_audit(
        &self,
        failure: &InstallFailure,
    ) -> Result<AuditAcknowledgement, AuditRetryError> {
        let InstallFailure::Audit(evidence) = failure else {
            return Err(AuditRetryError::NotPending);
        };
        self.audit
            .record(evidence.pending.clone())
            .map_err(AuditRetryError::Failed)
    }
}

/// Why bounded redelivery of retained audit evidence did not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditRetryError {
    /// The supplied install failure does not contain pending audit evidence.
    NotPending,
    /// Redelivery reached the audit adapter and still was not acknowledged.
    Failed(AuditFailure),
}

impl fmt::Display for AuditRetryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPending => formatter.write_str("install failure has no pending audit event"),
            Self::Failed(failure) => failure.fmt(formatter),
        }
    }
}

impl std::error::Error for AuditRetryError {}

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
    publication: InstallFailureEvidence,
    cleanup: &PublicationCleanupFailure,
) -> RecoveryFailureEvidence {
    RecoveryFailureEvidence::new(
        publication,
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
        StoreFailure::IncompleteArchive(detail) => (InstallFailureKind::IncompleteArchive, detail),
        StoreFailure::MalformedArchive(detail) => (InstallFailureKind::MalformedArchive, detail),
    };
    InstallFailureEvidence::capture(kind, detail)
}

fn with_audit_failure(
    operation: InstallFailure,
    runtime_state: RuntimeStateEvidence,
    pending: InstallTransition,
    failure: AuditFailure,
) -> InstallFailure {
    InstallFailure::Audit(Box::new(AuditDeliveryFailure {
        operation: Some(Box::new(operation)),
        runtime_state,
        pending,
        failure,
    }))
}

fn delivery_failure(failure: InstallDeliveryFailure) -> InstallFailure {
    publication_delivery_failure(
        None,
        RuntimeStateEvidence::Unchanged,
        None,
        Some(failure),
        None,
    )
}

fn delivery_domain_failure(detail: String) -> InstallFailure {
    delivery_failure(InstallDeliveryFailure::new(
        super::ports::InstallDeliveryFailureStage::ReadState,
        detail,
    ))
}

fn publication_delivery_failure(
    operation: Option<InstallFailure>,
    runtime_state: RuntimeStateEvidence,
    terminal: Option<InstallTransition>,
    delivery: Option<InstallDeliveryFailure>,
    audit: Option<AuditFailure>,
) -> InstallFailure {
    InstallFailure::Delivery(Box::new(PublicationDeliveryFailure {
        operation: operation.map(Box::new),
        runtime_state,
        terminal,
        delivery,
        audit,
    }))
}

fn formatter_delivery_failure(
    formatter: &mut fmt::Formatter<'_>,
    failure: &PublicationDeliveryFailure,
) -> fmt::Result {
    match (&failure.delivery, &failure.audit) {
        (Some(delivery), Some(audit)) => write!(
            formatter,
            "publication evidence is unresolved: {delivery}; audit also failed: {audit}"
        ),
        (Some(delivery), None) => write!(formatter, "{delivery}"),
        (None, Some(audit)) => write!(formatter, "publication audit failed: {audit}"),
        (None, None) => formatter.write_str("publication evidence is unresolved"),
    }
}

fn runtime_state_for_transition(transition: &InstallTransition) -> RuntimeStateEvidence {
    match transition.facts() {
        InstallTransitionFacts::Installed | InstallTransitionFacts::Replaced(_) => {
            RuntimeStateEvidence::TargetInstalled
        }
        InstallTransitionFacts::RolledBack(state) => match state {
            RollbackState::Restored(artifact) => RuntimeStateEvidence::Restored(artifact.clone()),
            RollbackState::NoInstalledRuntime => RuntimeStateEvidence::NoInstalledRuntime,
        },
        InstallTransitionFacts::RecoveryIncomplete { state, .. } => match state {
            RecoveryState::Confirmed(RollbackState::Restored(artifact)) => {
                RuntimeStateEvidence::Restored(artifact.clone())
            }
            RecoveryState::Confirmed(RollbackState::NoInstalledRuntime) => {
                RuntimeStateEvidence::NoInstalledRuntime
            }
            RecoveryState::Unconfirmed => RuntimeStateEvidence::Unconfirmed,
        },
        _ => RuntimeStateEvidence::Unconfirmed,
    }
}

#[cfg(test)]
#[path = "../../../tests/agent_install/install.rs"]
mod tests;
