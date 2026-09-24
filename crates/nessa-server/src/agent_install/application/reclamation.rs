use std::fmt;

use super::ports::{
    InstallationDeliverySession, PreparedInstallation, Publication, PublicationLease,
    RuntimeReclamationEffect, StoreFailure,
};
use crate::agent_install::domain::{
    AdmissionResult, AgentName, InstallFailureEvidence, InstallFailureKind, InstallRequest,
    InstallTransition, ManagedInstallation, ReclamationEvent, ReclamationObservation,
    ReclamationOperationId, ReclamationTrigger, ReclamationWork, ReplacementSettlementState,
    RuntimeArtifact,
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

    fn event_for(
        &self,
        operation_id: &ReclamationOperationId,
    ) -> Result<Option<ReclamationEvent>, ReclamationAuditFailure>;
}

/// Supplies fresh stable identities before a runtime-removal admission is retained.
pub trait ReclamationOperationIds: Send + Sync {
    /// Create one identity, or report why admission cannot safely begin.
    fn next(&self) -> Result<ReclamationOperationId, ReclamationPersistenceFailure>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReclamationWarning {
    Persistence(ReclamationPersistenceFailure),
    OutcomePersistence {
        event: ReclamationEvent,
        failure: ReclamationPersistenceFailure,
    },
    Outcome(ReclamationEvent),
    Audit {
        event: ReclamationEvent,
        failure: ReclamationAuditFailure,
    },
    AuditLookup {
        operation_id: ReclamationOperationId,
        failure: ReclamationAuditFailure,
    },
}

pub(crate) fn retain_replacement_and_reclaim(
    publication: &mut Publication,
    audit: &dyn ReclamationAudit,
    operation_ids: &dyn ReclamationOperationIds,
    terminal: &InstallTransition,
    prepared: &PreparedInstallation,
) -> Result<Vec<ReclamationWarning>, ReclamationPersistenceFailure> {
    let mut adapter = PublicationAdapter(publication);
    retain_replacement_and_reclaim_with_lease(
        &mut adapter,
        audit,
        operation_ids,
        terminal,
        prepared,
    )
}

pub(crate) fn retain_replacement_and_reclaim_with_lease(
    lease: &mut dyn PublicationLease,
    audit: &dyn ReclamationAudit,
    operation_ids: &dyn ReclamationOperationIds,
    terminal: &InstallTransition,
    prepared: &PreparedInstallation,
) -> Result<Vec<ReclamationWarning>, ReclamationPersistenceFailure> {
    let verified = prepared.preparation().verified();
    if verified.agent() != terminal.agent()
        || verified.target() != terminal.target()
        || verified.request() != terminal.request()
    {
        return Err(ReclamationPersistenceFailure::new(
            ReclamationPersistenceStage::RetainObligation,
            "replacement delivery preparation disagrees with its terminal".into(),
        ));
    }
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
    if installation.pending().iter().any(|pending| {
        pending.obligation().activation().request().account_id() != terminal.request().account_id()
    }) || installation.replacement_receipt().is_some_and(|receipt| {
        receipt.obligation().activation().request().account_id() != terminal.request().account_id()
    }) {
        return Err(ReclamationPersistenceFailure::new(
            ReclamationPersistenceStage::RetainObligation,
            "retained reclamation state belongs to another owner account".into(),
        ));
    }
    let already_retained = installation.current() == terminal.target()
        && installation.pending().iter().any(|pending| {
            pending.obligation().activation().request() == terminal.request()
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
    }
    installation
        .retain_replacement_receipt(prepared.record_id().to_owned(), terminal.request())
        .map_err(|error| {
            ReclamationPersistenceFailure::new(
                ReclamationPersistenceStage::RetainObligation,
                error.to_string(),
            )
        })?;
    lease.retain_reclamation(&installation, ReclamationPersistenceStage::RetainObligation)?;
    Ok(process_obligation_with_lease(
        lease,
        audit,
        operation_ids,
        &mut installation,
        terminal.request(),
        ReclamationTrigger::ReplacementFollowUp,
    ))
}

pub(crate) fn recover_reclamation(
    lease: &mut dyn PublicationLease,
    audit: &dyn ReclamationAudit,
    operation_ids: &dyn ReclamationOperationIds,
    delivery: &mut dyn InstallationDeliverySession,
    selected_agent: &AgentName,
    triggering_request: &InstallRequest,
) -> Result<Vec<ReclamationWarning>, Vec<ReclamationWarning>> {
    let mut installation = match lease.load_reclamation() {
        Ok(Some(installation)) => installation,
        Ok(None) => return Ok(Vec::new()),
        Err(error) => return Err(vec![ReclamationWarning::Persistence(error)]),
    };
    if installation.agent() != selected_agent
        || installation.pending().iter().any(|pending| {
            pending.obligation().activation().request().account_id()
                != triggering_request.account_id()
        })
    {
        return Err(vec![ReclamationWarning::Persistence(
            ReclamationPersistenceFailure::new(
                ReclamationPersistenceStage::Read,
                "retained reclamation state disagrees with the selected agent or owner account"
                    .into(),
            ),
        )]);
    }
    let unsettled = installation
        .replacement_receipt()
        .filter(|receipt| receipt.settlement() == ReplacementSettlementState::Pending)
        .cloned();
    let settled = if let Some(receipt) = &unsettled {
        match delivery.settled(receipt.delivery_id()) {
            Ok(Some((prepared, settlement))) => {
                let valid = prepared.record_id() == receipt.delivery_id()
                    && settlement
                        .outcome()
                        .terminal_transition()
                        .is_some_and(|terminal| {
                            terminal.agent() == installation.agent()
                                && terminal.request() == receipt.obligation().activation().request()
                                && terminal.target() == installation.current()
                                && terminal.previous() == Some(receipt.obligation().superseded())
                        });
                if !valid {
                    return Err(vec![ReclamationWarning::Persistence(
                        ReclamationPersistenceFailure::new(
                            ReclamationPersistenceStage::Read,
                            "settled delivery contradicts the retained replacement receipt".into(),
                        ),
                    )]);
                }
                Some((prepared, settlement))
            }
            Ok(None) => None,
            Err(error) => {
                return Err(vec![ReclamationWarning::Persistence(
                    ReclamationPersistenceFailure::new(
                        ReclamationPersistenceStage::Read,
                        error.to_string(),
                    ),
                )]);
            }
        }
    } else {
        None
    };
    let requests: Vec<InstallRequest> = installation
        .pending()
        .iter()
        .map(|pending| pending.obligation().activation().request().clone())
        .collect();
    let mut warnings = Vec::new();
    for request in requests {
        warnings.extend(process_obligation_with_lease(
            lease,
            audit,
            operation_ids,
            &mut installation,
            &request,
            ReclamationTrigger::LaterInstallation(triggering_request.clone()),
        ));
    }
    if let Some(receipt) = unsettled {
        let Some((prepared, settlement)) = settled else {
            warnings.push(ReclamationWarning::Persistence(
                ReclamationPersistenceFailure::new(
                    ReclamationPersistenceStage::Read,
                    "replacement cleanup is retained but its exact publication settlement is missing"
                        .into(),
                ),
            ));
            return Err(warnings);
        };
        let terminal = settlement
            .outcome()
            .terminal_transition()
            .expect("the settled replacement was validated above");
        if let Err(error) = installation
            .acknowledge_replacement_settlement(receipt.delivery_id(), terminal.request())
        {
            warnings.push(ReclamationWarning::Persistence(
                ReclamationPersistenceFailure::new(
                    ReclamationPersistenceStage::RetainAuditAcknowledgement,
                    error.to_string(),
                ),
            ));
            return Err(warnings);
        }
        if let Err(error) = lease.retain_reclamation(
            &installation,
            ReclamationPersistenceStage::RetainAuditAcknowledgement,
        ) {
            warnings.push(ReclamationWarning::Persistence(error));
            return Err(warnings);
        }
        debug_assert_eq!(prepared.record_id(), receipt.delivery_id());
    }
    Ok(warnings)
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
    operation_ids: &dyn ReclamationOperationIds,
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
            let operation_id = match operation_ids.next() {
                Ok(operation_id) => operation_id,
                Err(error) => return vec![ReclamationWarning::Persistence(error)],
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
                RuntimeReclamationEffect::Removed => installation.confirm_removed(*admission.1),
                RuntimeReclamationEffect::AlreadyAbsent => {
                    installation.confirm_already_absent(*admission.1)
                }
                RuntimeReclamationEffect::DeferredCurrent
                | RuntimeReclamationEffect::DeferredInUse
                | RuntimeReclamationEffect::StillPresent => {
                    installation.record_still_present(*admission.1)
                }
                RuntimeReclamationEffect::Failed(failure) => {
                    installation.record_removal_failure(*admission.1, failure_evidence(&failure))
                }
                RuntimeReclamationEffect::SyncUncertain(failure) => installation
                    .record_removal_sync_uncertain(*admission.1, failure_evidence(&failure)),
            }
            .map_err(|error| error.to_string())
        }
        ReclamationWork::Observe(admission) => {
            match audit.event_for(admission.operation_id()) {
                Ok(Some(event)) => {
                    return continue_known_event(
                        lease,
                        audit,
                        installation,
                        admission.as_ref(),
                        event,
                    )
                }
                Ok(None) => {}
                Err(failure) => {
                    return vec![ReclamationWarning::AuditLookup {
                        operation_id: admission.operation_id().clone(),
                        failure,
                    }]
                }
            }
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
        ReclamationWork::SettlementPending(_) => return Vec::new(),
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
    let retention = lease
        .retain_reclamation(installation, ReclamationPersistenceStage::RetainOutcome)
        .err();
    let audit_failure = audit.record(&event).err();
    if retention.is_some() || audit_failure.is_some() {
        let mut warnings = Vec::new();
        if let Some(failure) = retention {
            warnings.push(ReclamationWarning::OutcomePersistence {
                event: event.clone(),
                failure,
            });
        }
        if let Some(failure) = audit_failure {
            warnings.push(ReclamationWarning::Audit {
                event: event.clone(),
                failure,
            });
        }
        return warnings;
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

pub(crate) fn acknowledge_replacement_settlement(
    lease: &mut dyn PublicationLease,
    prepared: &PreparedInstallation,
    terminal: &InstallTransition,
) -> Result<(), ReclamationPersistenceFailure> {
    let mut installation = validated_replacement_settlement(lease, prepared, terminal)?;
    installation
        .acknowledge_replacement_settlement(prepared.record_id(), terminal.request())
        .map_err(|error| {
            ReclamationPersistenceFailure::new(
                ReclamationPersistenceStage::RetainAuditAcknowledgement,
                error.to_string(),
            )
        })?;
    lease.retain_reclamation(
        &installation,
        ReclamationPersistenceStage::RetainAuditAcknowledgement,
    )
}

pub(crate) fn require_replacement_cleanup_acknowledgement(
    lease: &mut dyn PublicationLease,
    prepared: &PreparedInstallation,
    terminal: &InstallTransition,
) -> Result<(), ReclamationPersistenceFailure> {
    validated_replacement_settlement(lease, prepared, terminal)?;
    Ok(())
}

fn validated_replacement_settlement(
    lease: &mut dyn PublicationLease,
    prepared: &PreparedInstallation,
    terminal: &InstallTransition,
) -> Result<ManagedInstallation, ReclamationPersistenceFailure> {
    let installation = lease.load_reclamation()?.ok_or_else(|| {
        ReclamationPersistenceFailure::new(
            ReclamationPersistenceStage::RetainAuditAcknowledgement,
            "replacement settlement has no retained reclamation receipt".into(),
        )
    })?;
    if installation.agent() != terminal.agent() || installation.current() != terminal.target() {
        return Err(ReclamationPersistenceFailure::new(
            ReclamationPersistenceStage::RetainAuditAcknowledgement,
            "replacement settlement disagrees with retained installation identity".into(),
        ));
    }
    let receipt = installation.replacement_receipt().ok_or_else(|| {
        ReclamationPersistenceFailure::new(
            ReclamationPersistenceStage::RetainAuditAcknowledgement,
            "replacement settlement has no retained reclamation receipt".into(),
        )
    })?;
    if prepared.preparation().verified().agent() != terminal.agent()
        || prepared.preparation().verified().target() != terminal.target()
        || prepared.preparation().verified().request() != terminal.request()
        || receipt.obligation().activation().replacement() != terminal.target()
        || receipt.obligation().activation().request() != terminal.request()
        || terminal.previous() != Some(receipt.obligation().superseded())
    {
        return Err(ReclamationPersistenceFailure::new(
            ReclamationPersistenceStage::RetainAuditAcknowledgement,
            "replacement settlement contradicts its retained receipt".into(),
        ));
    }
    installation
        .replacement_settlement_ready(prepared.record_id(), terminal.request())
        .map_err(|error| {
            ReclamationPersistenceFailure::new(
                ReclamationPersistenceStage::RetainAuditAcknowledgement,
                error.to_string(),
            )
        })?;
    Ok(installation)
}

pub(crate) fn acknowledge_replacement_settlement_on_publication(
    publication: &mut Publication,
    prepared: &PreparedInstallation,
    terminal: &InstallTransition,
) -> Result<(), ReclamationPersistenceFailure> {
    acknowledge_replacement_settlement(&mut PublicationAdapter(publication), prepared, terminal)
}

pub(crate) fn require_replacement_cleanup_acknowledgement_on_publication(
    publication: &mut Publication,
    prepared: &PreparedInstallation,
    terminal: &InstallTransition,
) -> Result<(), ReclamationPersistenceFailure> {
    require_replacement_cleanup_acknowledgement(
        &mut PublicationAdapter(publication),
        prepared,
        terminal,
    )
}

fn continue_known_event(
    lease: &mut dyn PublicationLease,
    audit: &dyn ReclamationAudit,
    installation: &mut ManagedInstallation,
    admission: &crate::agent_install::domain::ReclamationAdmission,
    event: ReclamationEvent,
) -> Vec<ReclamationWarning> {
    let event = match installation.recover_audited_event(admission.operation_id(), event) {
        Ok(event) => event,
        Err(error) => {
            return vec![ReclamationWarning::Persistence(
                ReclamationPersistenceFailure::new(
                    ReclamationPersistenceStage::Read,
                    error.to_string(),
                ),
            )]
        }
    };
    if let Err(error) =
        lease.retain_reclamation(installation, ReclamationPersistenceStage::RetainOutcome)
    {
        return vec![ReclamationWarning::OutcomePersistence {
            event,
            failure: error,
        }];
    }
    finish_audit(lease, audit, installation, event)
}

fn finish_audit(
    lease: &mut dyn PublicationLease,
    audit: &dyn ReclamationAudit,
    installation: &mut ManagedInstallation,
    event: ReclamationEvent,
) -> Vec<ReclamationWarning> {
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
