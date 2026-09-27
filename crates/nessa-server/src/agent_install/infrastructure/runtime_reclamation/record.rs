use serde::{Deserialize, Serialize};

use crate::agent_install::domain::{
    AgentName, ArchiveDigest, ArchivePath, FileRole, InstallFailureEvidence, InstallFailureKind,
    InstallRequest, ManagedInstallation, PendingReclamation, ReclamationActivation,
    ReclamationAdmission, ReclamationAuditState, ReclamationEvent, ReclamationObligation,
    ReclamationOperation, ReclamationOperationId, ReclamationPhysicalOutcome, ReclamationTrigger,
    ReleaseContents, ReleaseFile, ReleaseVersion, ReplacementReceipt, ReplacementSettlementState,
    RuntimeArtifact,
};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::agent_install::infrastructure) struct StoredManagedInstallation {
    agent: String,
    current: StoredArtifact,
    pending: Vec<StoredPending>,
    replacement_receipt: Option<StoredReplacementReceipt>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredReplacementReceipt {
    delivery_id: String,
    obligation: StoredObligation,
    settled: bool,
}

#[derive(Clone, Deserialize, Serialize)]
struct StoredArtifact {
    version: String,
    digest: String,
    files: Vec<StoredFile>,
}

#[derive(Clone, Deserialize, Serialize)]
struct StoredFile {
    path: String,
    role: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredPending {
    obligation: StoredObligation,
    operation: Option<StoredOperation>,
    last_acknowledged_operation_id: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
struct StoredObligation {
    superseded: StoredArtifact,
    origin: StoredActivation,
    activation: StoredActivation,
}

#[derive(Clone, Deserialize, Serialize)]
struct StoredActivation {
    replacement: StoredArtifact,
    request: StoredRequest,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredAdmission {
    agent: String,
    operation_id: String,
    obligation: StoredObligation,
    current: StoredArtifact,
    trigger: StoredTrigger,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "phase", content = "evidence", rename_all = "snake_case")]
enum StoredOperation {
    EffectPending(StoredAdmission),
    ObservationPending(StoredAdmission),
    EffectRecorded(StoredEvent),
}

#[derive(Deserialize, Serialize)]
pub(in crate::agent_install::infrastructure) struct StoredEvent {
    admission: StoredAdmission,
    outcome: StoredOutcome,
    audit: StoredAudit,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", content = "request", rename_all = "snake_case")]
enum StoredTrigger {
    ReplacementFollowUp,
    LaterInstallation(StoredRequest),
    ProcessRecovery,
    CallerRetry(StoredRequest),
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", content = "failure", rename_all = "snake_case")]
enum StoredOutcome {
    Removed,
    AlreadyAbsent,
    StillPresent,
    RemovalFailed(StoredFailure),
    RemovalSyncUncertain(StoredFailure),
    ObservationFailed(StoredFailure),
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredRequest {
    account_id: String,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
struct StoredFailure {
    kind: StoredFailureKind,
    detail: String,
    truncated: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum StoredFailureKind {
    Unwritable,
    Unreadable,
    IncompleteArchive,
    MalformedArchive,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum StoredAudit {
    Pending,
    Acknowledged,
}

impl StoredManagedInstallation {
    pub(in crate::agent_install::infrastructure) fn from_domain(
        value: &ManagedInstallation,
    ) -> Self {
        Self {
            agent: value.agent().as_str().to_owned(),
            current: StoredArtifact::from(value.current()),
            pending: value.pending().iter().map(StoredPending::from).collect(),
            replacement_receipt: value
                .replacement_receipt()
                .map(StoredReplacementReceipt::from),
        }
    }

    pub(in crate::agent_install::infrastructure) fn restore(
        self,
    ) -> Result<ManagedInstallation, String> {
        ManagedInstallation::restore(
            AgentName::parse(&self.agent).map_err(|error| error.to_string())?,
            self.current.restore()?,
            self.pending
                .into_iter()
                .map(StoredPending::restore)
                .collect::<Result<Vec<_>, _>>()?,
            self.replacement_receipt
                .map(StoredReplacementReceipt::restore)
                .transpose()?,
        )
        .map_err(|error| error.to_string())
    }
}

impl From<&ReplacementReceipt> for StoredReplacementReceipt {
    fn from(value: &ReplacementReceipt) -> Self {
        Self {
            delivery_id: value.delivery_id().to_owned(),
            obligation: StoredObligation::from(value.obligation()),
            settled: value.settlement() == ReplacementSettlementState::Settled,
        }
    }
}

impl StoredReplacementReceipt {
    fn restore(self) -> Result<ReplacementReceipt, String> {
        ReplacementReceipt::restore(
            self.delivery_id,
            self.obligation.restore()?,
            if self.settled {
                ReplacementSettlementState::Settled
            } else {
                ReplacementSettlementState::Pending
            },
        )
        .map_err(|error| error.to_string())
    }
}

impl From<&RuntimeArtifact> for StoredArtifact {
    fn from(value: &RuntimeArtifact) -> Self {
        Self {
            version: value.version().as_str().to_owned(),
            digest: value.digest().as_str().to_owned(),
            files: value
                .contents()
                .files()
                .iter()
                .map(|file| StoredFile {
                    path: file.path().as_str().to_owned(),
                    role: file.role().as_str().to_owned(),
                })
                .collect(),
        }
    }
}

impl StoredArtifact {
    fn restore(self) -> Result<RuntimeArtifact, String> {
        let files = self
            .files
            .into_iter()
            .map(|file| {
                Ok(ReleaseFile::new(
                    ArchivePath::parse(&file.path).map_err(|error| error.to_string())?,
                    FileRole::parse(&file.role).map_err(|error| error.to_string())?,
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(RuntimeArtifact::new(
            ReleaseVersion::parse(&self.version).map_err(|error| error.to_string())?,
            ArchiveDigest::parse(&self.digest).map_err(|error| error.to_string())?,
            ReleaseContents::new(files).map_err(|error| error.to_string())?,
        ))
    }
}

impl From<&PendingReclamation> for StoredPending {
    fn from(value: &PendingReclamation) -> Self {
        Self {
            obligation: StoredObligation::from(value.obligation()),
            operation: value.operation().map(StoredOperation::from),
            last_acknowledged_operation_id: value
                .last_acknowledged_operation_id()
                .map(|id| id.as_str().to_owned()),
        }
    }
}

impl StoredPending {
    fn restore(self) -> Result<PendingReclamation, String> {
        PendingReclamation::restore(
            self.obligation.restore()?,
            self.operation.map(StoredOperation::restore).transpose()?,
            self.last_acknowledged_operation_id
                .map(ReclamationOperationId::new)
                .transpose()
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())
    }
}

impl From<&ReclamationObligation> for StoredObligation {
    fn from(value: &ReclamationObligation) -> Self {
        Self {
            superseded: StoredArtifact::from(value.superseded()),
            origin: StoredActivation::from(value.origin()),
            activation: StoredActivation::from(value.activation()),
        }
    }
}

impl StoredObligation {
    fn restore(self) -> Result<ReclamationObligation, String> {
        ReclamationObligation::restore(
            self.superseded.restore()?,
            self.origin.restore()?,
            self.activation.restore()?,
        )
        .map_err(|error| error.to_string())
    }
}

impl From<&ReclamationActivation> for StoredActivation {
    fn from(value: &ReclamationActivation) -> Self {
        Self {
            replacement: StoredArtifact::from(value.replacement()),
            request: StoredRequest::from(value.request()),
        }
    }
}

impl StoredActivation {
    fn restore(self) -> Result<ReclamationActivation, String> {
        Ok(ReclamationActivation::new(
            self.replacement.restore()?,
            self.request.restore()?,
        ))
    }
}

impl From<&ReclamationOperation> for StoredOperation {
    fn from(value: &ReclamationOperation) -> Self {
        match value {
            ReclamationOperation::EffectPending(admission) => {
                Self::EffectPending(StoredAdmission::from(admission))
            }
            ReclamationOperation::ObservationPending(admission) => {
                Self::ObservationPending(StoredAdmission::from(admission))
            }
            ReclamationOperation::EffectRecorded(event) => {
                Self::EffectRecorded(StoredEvent::from(event))
            }
        }
    }
}

impl StoredOperation {
    fn restore(self) -> Result<ReclamationOperation, String> {
        Ok(match self {
            Self::EffectPending(admission) => {
                ReclamationOperation::EffectPending(admission.restore()?)
            }
            Self::ObservationPending(admission) => {
                ReclamationOperation::ObservationPending(admission.restore()?)
            }
            Self::EffectRecorded(event) => ReclamationOperation::EffectRecorded(event.restore()?),
        })
    }
}

impl From<&ReclamationAdmission> for StoredAdmission {
    fn from(value: &ReclamationAdmission) -> Self {
        Self {
            agent: value.agent().as_str().to_owned(),
            operation_id: value.operation_id().as_str().to_owned(),
            obligation: StoredObligation::from(value.obligation()),
            current: StoredArtifact::from(value.current()),
            trigger: StoredTrigger::from(value.trigger()),
        }
    }
}

impl StoredAdmission {
    fn restore(self) -> Result<ReclamationAdmission, String> {
        ReclamationAdmission::restore(
            AgentName::parse(&self.agent).map_err(|error| error.to_string())?,
            ReclamationOperationId::new(self.operation_id).map_err(|error| error.to_string())?,
            self.obligation.restore()?,
            self.current.restore()?,
            self.trigger.restore()?,
        )
        .map_err(|error| error.to_string())
    }
}

impl From<&ReclamationTrigger> for StoredTrigger {
    fn from(value: &ReclamationTrigger) -> Self {
        match value {
            ReclamationTrigger::ReplacementFollowUp => Self::ReplacementFollowUp,
            ReclamationTrigger::LaterInstallation(request) => {
                Self::LaterInstallation(StoredRequest::from(request))
            }
            ReclamationTrigger::ProcessRecovery => Self::ProcessRecovery,
            ReclamationTrigger::CallerRetry(request) => {
                Self::CallerRetry(StoredRequest::from(request))
            }
        }
    }
}

impl StoredTrigger {
    fn restore(self) -> Result<ReclamationTrigger, String> {
        Ok(match self {
            Self::ReplacementFollowUp => ReclamationTrigger::ReplacementFollowUp,
            Self::LaterInstallation(request) => {
                ReclamationTrigger::LaterInstallation(request.restore()?)
            }
            Self::ProcessRecovery => ReclamationTrigger::ProcessRecovery,
            Self::CallerRetry(request) => ReclamationTrigger::CallerRetry(request.restore()?),
        })
    }
}

impl From<&ReclamationEvent> for StoredEvent {
    fn from(value: &ReclamationEvent) -> Self {
        Self {
            admission: StoredAdmission::from(value.admission()),
            outcome: StoredOutcome::from(value.outcome()),
            audit: match value.audit() {
                ReclamationAuditState::Pending => StoredAudit::Pending,
                ReclamationAuditState::Acknowledged => StoredAudit::Acknowledged,
            },
        }
    }
}

impl StoredEvent {
    pub(in crate::agent_install::infrastructure) fn from_domain(value: &ReclamationEvent) -> Self {
        Self::from(value)
    }

    pub(in crate::agent_install::infrastructure) fn restore(
        self,
    ) -> Result<ReclamationEvent, String> {
        Ok(ReclamationEvent::restore(
            self.admission.restore()?,
            self.outcome.restore()?,
            match self.audit {
                StoredAudit::Pending => ReclamationAuditState::Pending,
                StoredAudit::Acknowledged => ReclamationAuditState::Acknowledged,
            },
        ))
    }
}

impl From<&ReclamationPhysicalOutcome> for StoredOutcome {
    fn from(value: &ReclamationPhysicalOutcome) -> Self {
        match value {
            ReclamationPhysicalOutcome::Removed => Self::Removed,
            ReclamationPhysicalOutcome::AlreadyAbsent => Self::AlreadyAbsent,
            ReclamationPhysicalOutcome::StillPresent => Self::StillPresent,
            ReclamationPhysicalOutcome::RemovalFailed(failure) => {
                Self::RemovalFailed(StoredFailure::from(failure))
            }
            ReclamationPhysicalOutcome::RemovalSyncUncertain(failure) => {
                Self::RemovalSyncUncertain(StoredFailure::from(failure))
            }
            ReclamationPhysicalOutcome::ObservationFailed(failure) => {
                Self::ObservationFailed(StoredFailure::from(failure))
            }
        }
    }
}

impl StoredOutcome {
    fn restore(self) -> Result<ReclamationPhysicalOutcome, String> {
        Ok(match self {
            Self::Removed => ReclamationPhysicalOutcome::Removed,
            Self::AlreadyAbsent => ReclamationPhysicalOutcome::AlreadyAbsent,
            Self::StillPresent => ReclamationPhysicalOutcome::StillPresent,
            Self::RemovalFailed(failure) => {
                ReclamationPhysicalOutcome::RemovalFailed(failure.restore()?)
            }
            Self::RemovalSyncUncertain(failure) => {
                ReclamationPhysicalOutcome::RemovalSyncUncertain(failure.restore()?)
            }
            Self::ObservationFailed(failure) => {
                ReclamationPhysicalOutcome::ObservationFailed(failure.restore()?)
            }
        })
    }
}

impl From<&InstallRequest> for StoredRequest {
    fn from(value: &InstallRequest) -> Self {
        Self {
            account_id: value.account_id().to_owned(),
            request_id: value.request_id().to_owned(),
        }
    }
}

impl StoredRequest {
    fn restore(self) -> Result<InstallRequest, String> {
        InstallRequest::new(self.account_id, self.request_id).map_err(|error| error.to_string())
    }
}

impl From<&InstallFailureEvidence> for StoredFailure {
    fn from(value: &InstallFailureEvidence) -> Self {
        Self {
            kind: match value.kind() {
                InstallFailureKind::Unwritable => StoredFailureKind::Unwritable,
                InstallFailureKind::Unreadable => StoredFailureKind::Unreadable,
                InstallFailureKind::IncompleteArchive => StoredFailureKind::IncompleteArchive,
                InstallFailureKind::MalformedArchive => StoredFailureKind::MalformedArchive,
            },
            detail: value.detail().to_owned(),
            truncated: value.truncated(),
        }
    }
}

impl StoredFailure {
    fn restore(self) -> Result<InstallFailureEvidence, String> {
        InstallFailureEvidence::restore(
            match self.kind {
                StoredFailureKind::Unwritable => InstallFailureKind::Unwritable,
                StoredFailureKind::Unreadable => InstallFailureKind::Unreadable,
                StoredFailureKind::IncompleteArchive => InstallFailureKind::IncompleteArchive,
                StoredFailureKind::MalformedArchive => InstallFailureKind::MalformedArchive,
            },
            self.detail,
            self.truncated,
        )
        .map_err(|error| error.to_string())
    }
}
