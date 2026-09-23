use std::fmt;

use super::{AgentName, ArchiveDigest, PinnedRelease, ReleaseContents, ReleaseVersion};

/// The complete installed artifact an install changes or observes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeArtifact {
    version: ReleaseVersion,
    digest: ArchiveDigest,
    contents: ReleaseContents,
}

impl RuntimeArtifact {
    /// Build installed-artifact identity from its invariant-bearing values.
    pub fn new(version: ReleaseVersion, digest: ArchiveDigest, contents: ReleaseContents) -> Self {
        Self {
            version,
            digest,
            contents,
        }
    }

    /// Project the installed identity of one pinned release.
    pub fn for_release(release: &PinnedRelease) -> Self {
        Self::new(
            release.version().clone(),
            release.archive_digest().clone(),
            release.contents().clone(),
        )
    }

    /// Return the pinned version included in artifact identity.
    pub fn version(&self) -> &ReleaseVersion {
        &self.version
    }

    /// Return the verified archive digest included in artifact identity.
    pub fn digest(&self) -> &ArchiveDigest {
        &self.digest
    }

    /// Return every installed path and role included in artifact identity.
    pub fn contents(&self) -> &ReleaseContents {
        &self.contents
    }
}

/// The verified local account and command invocation that requested an install.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct InstallRequest {
    account_id: String,
    request_id: String,
}

impl InstallRequest {
    /// Validate the account and invocation identities for one install request.
    pub fn new(
        account_id: impl Into<String>,
        request_id: impl Into<String>,
    ) -> Result<Self, InstallRequestError> {
        let account_id = plain(account_id.into()).ok_or(InstallRequestError::AccountId)?;
        let request_id = plain(request_id.into()).ok_or(InstallRequestError::RequestId)?;
        Ok(Self {
            account_id,
            request_id,
        })
    }

    /// Return the verified local account identity.
    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    /// Return the invocation identity assigned by composition.
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}

fn plain(value: String) -> Option<String> {
    (!value.is_empty() && value.len() <= 255 && !value.chars().any(char::is_control))
        .then_some(value)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallRequestError {
    /// The account identity is empty, too long, or contains control characters.
    AccountId,
    /// The invocation identity is empty, too long, or contains control characters.
    RequestId,
}

impl fmt::Display for InstallRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AccountId => "install account identity must be non-empty plain text",
            Self::RequestId => "install request identity must be non-empty plain text",
        })
    }
}

impl std::error::Error for InstallRequestError {}

/// One logical position in an install attempt's evidence sequence.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum InstallEventSlot {
    /// The attempt was admitted before effects began.
    Started,
    /// Digest verification either succeeded or rejected the archive.
    VerificationOutcome,
    /// Publication succeeded, rolled back, or ended with incomplete recovery.
    CompletionOutcome,
}

/// Stable identity of one logical install event.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct InstallEventIdentity {
    request: InstallRequest,
    slot: InstallEventSlot,
}

impl InstallEventIdentity {
    /// Pair a request with the domain-owned lifecycle slot.
    pub fn new(request: InstallRequest, slot: InstallEventSlot) -> Self {
        Self { request, slot }
    }

    /// Return the account and invocation identity.
    pub fn request(&self) -> &InstallRequest {
        &self.request
    }

    /// Return this event's lifecycle slot.
    pub fn slot(&self) -> InstallEventSlot {
        self.slot
    }
}

/// The closed vocabulary of install transitions written to audit storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallTransitionKind {
    Started,
    Verified,
    DigestRejected,
    Installed,
    Replaced,
    RolledBack,
    RecoveryIncomplete,
}

impl InstallTransitionKind {
    /// Return the domain-owned logical slot for this kind.
    pub fn slot(self) -> InstallEventSlot {
        match self {
            Self::Started => InstallEventSlot::Started,
            Self::Verified | Self::DigestRejected => InstallEventSlot::VerificationOutcome,
            Self::Installed | Self::Replaced | Self::RolledBack | Self::RecoveryIncomplete => {
                InstallEventSlot::CompletionOutcome
            }
        }
    }
}

/// The installed state a failed publication actually restored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RollbackState {
    Restored(RuntimeArtifact),
    NoInstalledRuntime,
}

/// The stable category of one filesystem failure retained as audit evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallFailureKind {
    Unwritable,
    Unreadable,
    IncompleteArchive,
    MalformedArchive,
}

/// One immutable failure fact at the installation boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallFailureEvidence {
    kind: InstallFailureKind,
    detail: String,
}

impl InstallFailureEvidence {
    pub fn new(kind: InstallFailureKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub fn kind(&self) -> InstallFailureKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

/// The remaining installed state after cleanup did not fully succeed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryState {
    Confirmed(RollbackState),
    Unconfirmed,
}

/// Original publication cause and every cleanup failure retained together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryFailureEvidence {
    publication: InstallFailureEvidence,
    withdrawal: Option<InstallFailureEvidence>,
    restoration: Option<InstallFailureEvidence>,
    confirmation: Option<InstallFailureEvidence>,
}

impl RecoveryFailureEvidence {
    pub fn new(
        publication: InstallFailureEvidence,
        withdrawal: Option<InstallFailureEvidence>,
        restoration: Option<InstallFailureEvidence>,
        confirmation: Option<InstallFailureEvidence>,
    ) -> Result<Self, InstallTransitionError> {
        if withdrawal.is_none() && restoration.is_none() && confirmation.is_none() {
            return Err(InstallTransitionError::MissingCleanupFailure);
        }
        Ok(Self {
            publication,
            withdrawal,
            restoration,
            confirmation,
        })
    }

    pub fn publication(&self) -> &InstallFailureEvidence {
        &self.publication
    }

    pub fn withdrawal(&self) -> Option<&InstallFailureEvidence> {
        self.withdrawal.as_ref()
    }

    pub fn restoration(&self) -> Option<&InstallFailureEvidence> {
        self.restoration.as_ref()
    }

    pub fn confirmation(&self) -> Option<&InstallFailureEvidence> {
        self.confirmation.as_ref()
    }
}

/// Semantic facts for one transition, independent of journal metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallTransitionFacts {
    Started,
    Verified,
    DigestRejected(ArchiveDigest),
    Installed,
    Replaced(RuntimeArtifact),
    RolledBack(RollbackState),
    RecoveryIncomplete {
        state: RecoveryState,
        failures: RecoveryFailureEvidence,
    },
}

/// Immutable evidence produced by the domain owner of one install attempt.
#[must_use]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallTransition {
    kind: InstallTransitionKind,
    agent: AgentName,
    target: RuntimeArtifact,
    request: InstallRequest,
    facts: InstallTransitionFacts,
}

impl InstallTransition {
    /// Restore or create transition evidence through the same domain invariants.
    pub fn restore(
        agent: AgentName,
        target: RuntimeArtifact,
        request: InstallRequest,
        facts: InstallTransitionFacts,
    ) -> Result<Self, InstallTransitionError> {
        let kind = match &facts {
            InstallTransitionFacts::Started => InstallTransitionKind::Started,
            InstallTransitionFacts::Verified => InstallTransitionKind::Verified,
            InstallTransitionFacts::DigestRejected(actual) => {
                if actual == target.digest() {
                    return Err(InstallTransitionError::MatchingRejectedDigest);
                }
                InstallTransitionKind::DigestRejected
            }
            InstallTransitionFacts::Installed => InstallTransitionKind::Installed,
            InstallTransitionFacts::Replaced(previous) => {
                if previous == &target {
                    return Err(InstallTransitionError::UnchangedReplacement);
                }
                InstallTransitionKind::Replaced
            }
            InstallTransitionFacts::RolledBack(rollback) => {
                verify_restored_target(&target, rollback)?;
                InstallTransitionKind::RolledBack
            }
            InstallTransitionFacts::RecoveryIncomplete { state, .. } => {
                if let RecoveryState::Confirmed(rollback) = state {
                    verify_restored_target(&target, rollback)?;
                }
                InstallTransitionKind::RecoveryIncomplete
            }
        };
        Ok(Self {
            kind,
            agent,
            target,
            request,
            facts,
        })
    }

    pub fn kind(&self) -> InstallTransitionKind {
        self.kind
    }

    pub fn event_identity(&self) -> InstallEventIdentity {
        InstallEventIdentity::new(self.request.clone(), self.kind.slot())
    }

    pub fn agent(&self) -> &AgentName {
        &self.agent
    }

    pub fn target(&self) -> &RuntimeArtifact {
        &self.target
    }

    pub fn request(&self) -> &InstallRequest {
        &self.request
    }

    pub fn facts(&self) -> &InstallTransitionFacts {
        &self.facts
    }

    pub fn actual_digest(&self) -> Option<&ArchiveDigest> {
        match &self.facts {
            InstallTransitionFacts::DigestRejected(value) => Some(value),
            _ => None,
        }
    }

    pub fn previous(&self) -> Option<&RuntimeArtifact> {
        match &self.facts {
            InstallTransitionFacts::Replaced(value) => Some(value),
            _ => None,
        }
    }

    pub fn rollback(&self) -> Option<&RollbackState> {
        match &self.facts {
            InstallTransitionFacts::RolledBack(value) => Some(value),
            _ => None,
        }
    }

    pub fn recovery(&self) -> Option<(&RecoveryState, &RecoveryFailureEvidence)> {
        match &self.facts {
            InstallTransitionFacts::RecoveryIncomplete { state, failures } => {
                Some((state, failures))
            }
            _ => None,
        }
    }
}

fn verify_restored_target(
    target: &RuntimeArtifact,
    rollback: &RollbackState,
) -> Result<(), InstallTransitionError> {
    if matches!(rollback, RollbackState::Restored(restored) if restored == target) {
        Err(InstallTransitionError::TargetReportedRestored)
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallTransitionError {
    MatchingRejectedDigest,
    UnchangedReplacement,
    TargetReportedRestored,
    MissingCleanupFailure,
}

impl fmt::Display for InstallTransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MatchingRejectedDigest => "a matching digest cannot be rejected",
            Self::UnchangedReplacement => "an artifact cannot replace itself",
            Self::TargetReportedRestored => "a rolled-back target cannot be the restored runtime",
            Self::MissingCleanupFailure => "incomplete recovery requires a cleanup failure",
        })
    }
}

impl std::error::Error for InstallTransitionError {}
