use std::fmt;

use super::ports::{Publication, PublicationLease, RuntimeReclamationEffect, StoreFailure};
use crate::agent_install::domain::{
    AdmissionResult, AgentName, InstallFailureEvidence, InstallFailureKind, InstallRequest,
    InstallTransition, ManagedInstallation, ReclamationEvent, ReclamationObservation,
    ReclamationOperationId, ReclamationTrigger, ReclamationWork, RuntimeArtifact,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReclamationPersistenceStage {
    Read,
    RetainObligation,
    RetainAdmission,
    RetainOutcome,
    RetainAuditAcknowledgement,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReclamationPersistenceFailure {
    stage: ReclamationPersistenceStage,
    detail: String,
}

impl ReclamationPersistenceFailure {
    pub fn new(stage: ReclamationPersistenceStage, detail: String) -> Self {
        Self { stage, detail }
    }

    pub fn stage(&self) -> ReclamationPersistenceStage {
        self.stage
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for ReclamationPersistenceFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "runtime reclamation persistence failed at {:?}: {}",
            self.stage, self.detail
        )
    }
}

impl std::error::Error for ReclamationPersistenceFailure {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReclamationAuditFailure {
    detail: String,
}

impl ReclamationAuditFailure {
    pub fn new(detail: String) -> Self {
        Self { detail }
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for ReclamationAuditFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "runtime reclamation audit failed: {}",
            self.detail
        )
    }
}

impl std::error::Error for ReclamationAuditFailure {}

pub trait ReclamationAudit: Send + Sync {
    fn record(&self, event: &ReclamationEvent) -> Result<(), ReclamationAuditFailure>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReclamationWarning {
    Persistence(ReclamationPersistenceFailure),
    Outcome(ReclamationEvent),
    Audit {
        event: ReclamationEvent,
        failure: ReclamationAuditFailure,
    },
}

pub(crate) fn retain_replacement_and_reclaim(
    publication: &mut Publication,
    audit: &dyn ReclamationAudit,
    terminal: &InstallTransition,
) -> Result<Vec<ReclamationWarning>, ReclamationPersistenceFailure> {
    let mut adapter = PublicationAdapter(publication);
    retain_replacement_and_reclaim_with_lease(&mut adapter, audit, terminal)
}

pub(crate) fn retain_replacement_and_reclaim_with_lease(
    lease: &mut dyn PublicationLease,
    audit: &dyn ReclamationAudit,
    terminal: &InstallTransition,
) -> Result<Vec<ReclamationWarning>, ReclamationPersistenceFailure> {
    let previous = terminal.previous().ok_or_else(|| {
        ReclamationPersistenceFailure::new(
            ReclamationPersistenceStage::RetainObligation,
            "replacement cleanup requires the exact superseded artifact".into(),
        )
    })?;
    let mut installation = match lease.load_reclamation()? {
        Some(installation) => installation,
        None => ManagedInstallation::new(terminal.agent().clone(), previous.clone()),
    };
    if installation.agent() != terminal.agent() {
        return Err(ReclamationPersistenceFailure::new(
            ReclamationPersistenceStage::RetainObligation,
            "retained reclamation state belongs to another agent".into(),
        ));
    }
    let already_retained = installation.current() == terminal.target()
        && installation.pending().iter().any(|pending| {
            pending.obligation().origin().request() == terminal.request()
                && pending.obligation().superseded() == previous
                && pending.obligation().activation().replacement() == terminal.target()
        });
    if !already_retained {
        if installation.current() != previous {
            return Err(ReclamationPersistenceFailure::new(
                ReclamationPersistenceStage::RetainObligation,
                "retained current artifact contradicts the replacement predecessor".into(),
            ));
        }
        installation
            .record_replacement(terminal.target().clone(), terminal.request().clone())
            .map_err(|error| {
                ReclamationPersistenceFailure::new(
                    ReclamationPersistenceStage::RetainObligation,
                    error.to_string(),
                )
            })?;
        lease.retain_reclamation(&installation, ReclamationPersistenceStage::RetainObligation)?;
    }
    Ok(process_obligation_with_lease(
        lease,
        audit,
        &mut installation,
        terminal.request(),
        ReclamationTrigger::ReplacementFollowUp,
    ))
}

pub(crate) fn recover_reclamation(
    lease: &mut dyn PublicationLease,
    audit: &dyn ReclamationAudit,
    triggering_request: &InstallRequest,
) -> Vec<ReclamationWarning> {
    let mut installation = match lease.load_reclamation() {
        Ok(Some(installation)) => installation,
        Ok(None) => return Vec::new(),
        Err(error) => return vec![ReclamationWarning::Persistence(error)],
    };
    let requests: Vec<InstallRequest> = installation
        .pending()
        .iter()
        .map(|pending| pending.obligation().origin().request().clone())
        .collect();
    let mut warnings = Vec::new();
    for request in requests {
        warnings.extend(process_obligation_with_lease(
            lease,
            audit,
            &mut installation,
            &request,
            ReclamationTrigger::LaterInstallation(triggering_request.clone()),
        ));
    }
    warnings
}

struct PublicationAdapter<'a>(&'a mut Publication);

impl PublicationLease for PublicationAdapter<'_> {
    fn load_reclamation(
        &mut self,
    ) -> Result<Option<ManagedInstallation>, ReclamationPersistenceFailure> {
        self.0.load_reclamation()
    }

    fn retain_reclamation(
        &mut self,
        installation: &ManagedInstallation,
        stage: ReclamationPersistenceStage,
    ) -> Result<(), ReclamationPersistenceFailure> {
        self.0.retain_reclamation(installation, stage)
    }

    fn remove_superseded(
        &mut self,
        agent: &AgentName,
        current: &RuntimeArtifact,
        superseded: &RuntimeArtifact,
    ) -> RuntimeReclamationEffect {
        self.0.remove_superseded(agent, current, superseded)
    }

    fn observe_superseded(
        &mut self,
        agent: &AgentName,
        current: &RuntimeArtifact,
        superseded: &RuntimeArtifact,
    ) -> RuntimeReclamationEffect {
        self.0.observe_superseded(agent, current, superseded)
    }
}

fn process_obligation_with_lease(
    lease: &mut dyn PublicationLease,
    audit: &dyn ReclamationAudit,
    installation: &mut ManagedInstallation,
    replacement_request: &InstallRequest,
    trigger: ReclamationTrigger,
) -> Vec<ReclamationWarning> {
    let work = match installation.work(replacement_request) {
        Ok(work) => work,
        Err(error) => {
            return vec![ReclamationWarning::Persistence(
                ReclamationPersistenceFailure::new(
                    ReclamationPersistenceStage::Read,
                    error.to_string(),
                ),
            )]
        }
    };
    let event = match work {
        ReclamationWork::Ready => {
            let operation_id = match ReclamationOperationId::new(uuid::Uuid::new_v4().to_string()) {
                Ok(operation_id) => operation_id,
                Err(error) => {
                    return vec![ReclamationWarning::Persistence(
                        ReclamationPersistenceFailure::new(
                            ReclamationPersistenceStage::RetainAdmission,
                            error.to_string(),
                        ),
                    )]
                }
            };
            let admission =
                match installation.admit_reclamation(replacement_request, operation_id, trigger) {
                    Ok(AdmissionResult::Fresh { admission, permit }) => (admission, permit),
                    Ok(AdmissionResult::Existing(_)) => {
                        return vec![ReclamationWarning::Persistence(
                            ReclamationPersistenceFailure::new(
                                ReclamationPersistenceStage::RetainAdmission,
                                "fresh reclamation admission returned existing work".into(),
                            ),
                        )]
                    }
                    Err(error) => {
                        return vec![ReclamationWarning::Persistence(
                            ReclamationPersistenceFailure::new(
                                ReclamationPersistenceStage::RetainAdmission,
                                error.to_string(),
                            ),
                        )]
                    }
                };
            if let Err(error) =
                lease.retain_reclamation(installation, ReclamationPersistenceStage::RetainAdmission)
            {
                return vec![ReclamationWarning::Persistence(error)];
            }
            let effect = lease.remove_superseded(
                admission.0.agent(),
                admission.0.current(),
                admission.0.obligation().superseded(),
            );
            match effect {
                RuntimeReclamationEffect::Removed => installation.confirm_removed(admission.1),
                RuntimeReclamationEffect::AlreadyAbsent => {
                    installation.confirm_already_absent(admission.1)
                }
                RuntimeReclamationEffect::DeferredCurrent
                | RuntimeReclamationEffect::DeferredInUse
                | RuntimeReclamationEffect::StillPresent => {
                    installation.record_still_present(admission.1)
                }
                RuntimeReclamationEffect::Failed(failure) => {
                    installation.record_removal_failure(admission.1, failure_evidence(&failure))
                }
                RuntimeReclamationEffect::SyncUncertain(failure) => installation
                    .record_removal_sync_uncertain(admission.1, failure_evidence(&failure)),
            }
            .map_err(|error| error.to_string())
        }
        ReclamationWork::Observe(admission) => {
            let effect = lease.observe_superseded(
                admission.agent(),
                admission.current(),
                admission.obligation().superseded(),
            );
            let observation = match effect {
                RuntimeReclamationEffect::AlreadyAbsent => ReclamationObservation::AlreadyAbsent,
                RuntimeReclamationEffect::DeferredCurrent
                | RuntimeReclamationEffect::DeferredInUse
                | RuntimeReclamationEffect::StillPresent => ReclamationObservation::StillPresent,
                RuntimeReclamationEffect::Failed(failure)
                | RuntimeReclamationEffect::SyncUncertain(failure) => {
                    ReclamationObservation::Failed(failure_evidence(&failure))
                }
                RuntimeReclamationEffect::Removed => ReclamationObservation::AlreadyAbsent,
            };
            installation
                .record_observation(admission.operation_id(), observation)
                .map_err(|error| error.to_string())
        }
        ReclamationWork::Audit(event) => Ok(*event),
        ReclamationWork::BlockedCurrent => return Vec::new(),
        ReclamationWork::EffectInProgress(_) => {
            return vec![ReclamationWarning::Persistence(
                ReclamationPersistenceFailure::new(
                    ReclamationPersistenceStage::Read,
                    "live reclamation effect cannot be recovered concurrently".into(),
                ),
            )]
        }
    };
    let event = match event {
        Ok(event) => event,
        Err(detail) => {
            return vec![ReclamationWarning::Persistence(
                ReclamationPersistenceFailure::new(
                    ReclamationPersistenceStage::RetainOutcome,
                    detail,
                ),
            )]
        }
    };
    if let Err(error) =
        lease.retain_reclamation(installation, ReclamationPersistenceStage::RetainOutcome)
    {
        return vec![ReclamationWarning::Persistence(error)];
    }
    if let Err(failure) = audit.record(&event) {
        return vec![ReclamationWarning::Audit { event, failure }];
    }
    let acknowledged = match installation.acknowledge_audit(event.admission().operation_id()) {
        Ok(event) => event,
        Err(error) => {
            return vec![ReclamationWarning::Persistence(
                ReclamationPersistenceFailure::new(
                    ReclamationPersistenceStage::RetainAuditAcknowledgement,
                    error.to_string(),
                ),
            )]
        }
    };
    if let Err(error) = lease.retain_reclamation(
        installation,
        ReclamationPersistenceStage::RetainAuditAcknowledgement,
    ) {
        return vec![ReclamationWarning::Persistence(error)];
    }
    (!acknowledged.outcome().completes_obligation())
        .then_some(ReclamationWarning::Outcome(acknowledged))
        .into_iter()
        .collect()
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

#[cfg(test)]
#[path = "../../../tests/agent_install/reclamation.rs"]
mod tests;
