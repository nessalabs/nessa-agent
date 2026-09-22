use std::fmt;

use super::{AgentName, ArchiveDigest, ArchivePath, PinnedRelease, ReleaseVersion};

/// The pinned artifact an install changes or observes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeArtifact {
    version: ReleaseVersion,
    digest: ArchiveDigest,
    executable: ArchivePath,
}

impl RuntimeArtifact {
    pub fn new(version: ReleaseVersion, digest: ArchiveDigest, executable: ArchivePath) -> Self {
        Self {
            version,
            digest,
            executable,
        }
    }

    pub fn for_release(release: &PinnedRelease) -> Self {
        Self::new(
            release.version().clone(),
            release.archive_digest().clone(),
            release.executable().clone(),
        )
    }

    pub fn version(&self) -> &ReleaseVersion {
        &self.version
    }
    pub fn digest(&self) -> &ArchiveDigest {
        &self.digest
    }
    pub fn executable(&self) -> &ArchivePath {
        &self.executable
    }
}

/// The verified local account and command invocation that requested an install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallRequest {
    account_id: String,
    request_id: String,
}

impl InstallRequest {
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

    pub fn account_id(&self) -> &str {
        &self.account_id
    }
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
    AccountId,
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

/// The closed vocabulary of install transitions written to audit storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallTransitionKind {
    Started,
    Verified,
    DigestRejected,
    Installed,
    Replaced,
    RolledBack,
    SupersededArtifactRemoved,
}

/// The installed state a failed publication actually restored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RollbackState {
    Restored(RuntimeArtifact),
    NoInstalledRuntime,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TransitionDetail {
    None,
    ActualDigest(ArchiveDigest),
    Previous(RuntimeArtifact),
    Rollback(RollbackState),
    Current(RuntimeArtifact),
}

/// Immutable evidence produced by the domain owner of one install attempt.
#[must_use]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallTransition {
    kind: InstallTransitionKind,
    agent: AgentName,
    target: RuntimeArtifact,
    request: InstallRequest,
    detail: TransitionDetail,
}

impl InstallTransition {
    pub(crate) fn simple(
        kind: InstallTransitionKind,
        agent: AgentName,
        target: RuntimeArtifact,
        request: InstallRequest,
    ) -> Self {
        Self {
            kind,
            agent,
            target,
            request,
            detail: TransitionDetail::None,
        }
    }

    pub(crate) fn rejected(
        agent: AgentName,
        target: RuntimeArtifact,
        actual: ArchiveDigest,
        request: InstallRequest,
    ) -> Result<Self, InstallTransitionError> {
        if &actual == target.digest() {
            return Err(InstallTransitionError::MatchingRejectedDigest);
        }
        Ok(Self {
            kind: InstallTransitionKind::DigestRejected,
            agent,
            target,
            request,
            detail: TransitionDetail::ActualDigest(actual),
        })
    }

    pub(crate) fn replaced(
        agent: AgentName,
        previous: RuntimeArtifact,
        target: RuntimeArtifact,
        request: InstallRequest,
    ) -> Result<Self, InstallTransitionError> {
        if previous == target {
            return Err(InstallTransitionError::UnchangedReplacement);
        }
        Ok(Self {
            kind: InstallTransitionKind::Replaced,
            agent,
            target,
            request,
            detail: TransitionDetail::Previous(previous),
        })
    }

    pub(crate) fn rolled_back(
        agent: AgentName,
        target: RuntimeArtifact,
        rollback: RollbackState,
        request: InstallRequest,
    ) -> Result<Self, InstallTransitionError> {
        if matches!(&rollback, RollbackState::Restored(restored) if restored == &target) {
            return Err(InstallTransitionError::TargetReportedRestored);
        }
        Ok(Self {
            kind: InstallTransitionKind::RolledBack,
            agent,
            target,
            request,
            detail: TransitionDetail::Rollback(rollback),
        })
    }

    pub(crate) fn removed(
        agent: AgentName,
        removed: RuntimeArtifact,
        current: RuntimeArtifact,
        request: InstallRequest,
    ) -> Result<Self, InstallTransitionError> {
        if removed == current {
            return Err(InstallTransitionError::CurrentArtifactRemoved);
        }
        Ok(Self {
            kind: InstallTransitionKind::SupersededArtifactRemoved,
            agent,
            target: removed,
            request,
            detail: TransitionDetail::Current(current),
        })
    }

    pub fn kind(&self) -> InstallTransitionKind {
        self.kind
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
    pub fn actual_digest(&self) -> Option<&ArchiveDigest> {
        match &self.detail {
            TransitionDetail::ActualDigest(value) => Some(value),
            _ => None,
        }
    }
    pub fn previous(&self) -> Option<&RuntimeArtifact> {
        match &self.detail {
            TransitionDetail::Previous(value) => Some(value),
            _ => None,
        }
    }
    pub fn rollback(&self) -> Option<&RollbackState> {
        match &self.detail {
            TransitionDetail::Rollback(value) => Some(value),
            _ => None,
        }
    }
    pub fn current(&self) -> Option<&RuntimeArtifact> {
        match &self.detail {
            TransitionDetail::Current(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallTransitionError {
    MatchingRejectedDigest,
    UnchangedReplacement,
    TargetReportedRestored,
    CurrentArtifactRemoved,
}

impl fmt::Display for InstallTransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MatchingRejectedDigest => "a matching digest cannot be rejected",
            Self::UnchangedReplacement => "an artifact cannot replace itself",
            Self::TargetReportedRestored => "a rolled-back target cannot be the restored runtime",
            Self::CurrentArtifactRemoved => "the current artifact cannot be removed as superseded",
        })
    }
}

impl std::error::Error for InstallTransitionError {}
