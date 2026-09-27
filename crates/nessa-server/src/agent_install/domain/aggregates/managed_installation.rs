use std::fmt;

use crate::agent_install::domain::{
    AgentName, InstallFailureEvidence, InstallRequest, ReclamationActivation, ReclamationAdmission,
    ReclamationAuditState, ReclamationEvent, ReclamationObligation, ReclamationOperationId,
    ReclamationPhysicalOutcome, ReclamationTrigger, ReplacementReceipt, ReplacementSettlementState,
    RuntimeArtifact,
};

/// Persistable progress for one reclamation operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReclamationOperation {
    EffectPending(ReclamationAdmission),
    ObservationPending(ReclamationAdmission),
    EffectRecorded(ReclamationEvent),
}

impl ReclamationOperation {
    pub fn admission(&self) -> &ReclamationAdmission {
        match self {
            Self::EffectPending(admission) => admission,
            Self::ObservationPending(admission) => admission,
            Self::EffectRecorded(event) => event.admission(),
        }
    }
}

/// One pending obligation and its bounded replay state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingReclamation {
    obligation: ReclamationObligation,
    operation: Option<ReclamationOperation>,
    last_acknowledged_operation_id: Option<ReclamationOperationId>,
}

impl PendingReclamation {
    /// Restore one pending item while rejecting contradictory operation facts.
    pub fn restore(
        obligation: ReclamationObligation,
        operation: Option<ReclamationOperation>,
        last_acknowledged_operation_id: Option<ReclamationOperationId>,
    ) -> Result<Self, ManagedInstallationError> {
        if let Some(operation) = &operation {
            if operation.admission().obligation() != &obligation {
                return Err(ManagedInstallationError::ObligationMismatch);
            }
            if last_acknowledged_operation_id.as_ref() == Some(operation.admission().operation_id())
            {
                return Err(ManagedInstallationError::DuplicateOperationId);
            }
            if matches!(
                operation,
                ReclamationOperation::EffectRecorded(event)
                    if event.audit() == ReclamationAuditState::Acknowledged
                        && !event.outcome().completes_obligation()
            ) {
                return Err(ManagedInstallationError::AcknowledgedOperationRetained);
            }
        }
        Ok(Self {
            obligation,
            operation,
            last_acknowledged_operation_id,
        })
    }

    pub fn obligation(&self) -> &ReclamationObligation {
        &self.obligation
    }

    pub fn operation(&self) -> Option<&ReclamationOperation> {
        self.operation.as_ref()
    }

    /// Only the most recently acknowledged unsuccessful operation is retained.
    pub fn last_acknowledged_operation_id(&self) -> Option<&ReclamationOperationId> {
        self.last_acknowledged_operation_id.as_ref()
    }

    fn recover_interrupted_effect(&mut self) {
        self.operation = match self.operation.take() {
            Some(ReclamationOperation::EffectPending(admission)) => {
                Some(ReclamationOperation::ObservationPending(admission))
            }
            operation => operation,
        };
    }
}

/// Aggregate root for one agent's installed artifact and reclamation work.
pub struct ManagedInstallation {
    agent: AgentName,
    current: RuntimeArtifact,
    pending: Vec<PendingReclamation>,
    replacement_receipt: Option<ReplacementReceipt>,
}

impl ManagedInstallation {
    pub fn new(agent: AgentName, current: RuntimeArtifact) -> Self {
        Self {
            agent,
            current,
            pending: Vec::new(),
            replacement_receipt: None,
        }
    }

    /// Restore persisted state and recheck cross-obligation relationships.
    pub fn restore(
        agent: AgentName,
        current: RuntimeArtifact,
        mut pending: Vec<PendingReclamation>,
        replacement_receipt: Option<ReplacementReceipt>,
    ) -> Result<Self, ManagedInstallationError> {
        for item in &pending {
            validate_restored_operation(&agent, &current, item.operation.as_ref())?;
        }
        for item in &mut pending {
            item.recover_interrupted_effect();
        }
        for (index, item) in pending.iter().enumerate() {
            for other in &pending[index + 1..] {
                if item.obligation.superseded().physical_identity()
                    == other.obligation.superseded().physical_identity()
                {
                    return Err(ManagedInstallationError::DuplicatePhysicalTarget);
                }
                if obligation_correlations(&item.obligation)
                    .iter()
                    .any(|correlation| {
                        obligation_correlations(&other.obligation).contains(correlation)
                    })
                {
                    return Err(ManagedInstallationError::DuplicateCorrelation);
                }
            }
        }
        let mut operation_ids: Vec<&ReclamationOperationId> = Vec::new();
        for item in &pending {
            if let Some(operation) = &item.operation {
                operation_ids.push(operation.admission().operation_id());
            }
            if let Some(operation_id) = &item.last_acknowledged_operation_id {
                operation_ids.push(operation_id);
            }
        }
        for (index, operation_id) in operation_ids.iter().enumerate() {
            if operation_ids[index + 1..].contains(operation_id) {
                return Err(ManagedInstallationError::DuplicateOperationId);
            }
        }
        if let Some(receipt) = &replacement_receipt {
            if receipt.obligation().activation().replacement() != &current {
                return Err(ManagedInstallationError::ReceiptCurrentMismatch);
            }
            if receipt.settlement() == ReplacementSettlementState::Pending
                && !pending
                    .iter()
                    .any(|item| item.obligation() == receipt.obligation())
            {
                return Err(ManagedInstallationError::ReceiptObligationMissing);
            }
        }
        Ok(Self {
            agent,
            current,
            pending,
            replacement_receipt,
        })
    }

    pub fn agent(&self) -> &AgentName {
        &self.agent
    }

    pub fn current(&self) -> &RuntimeArtifact {
        &self.current
    }

    pub fn pending(&self) -> &[PendingReclamation] {
        &self.pending
    }

    /// Return the latest accepted replacement's independent delivery receipt.
    pub fn replacement_receipt(&self) -> Option<&ReplacementReceipt> {
        self.replacement_receipt.as_ref()
    }

    /// Record a replacement while retaining the prior artifact and its cause.
    ///
    /// An effect-pending operation must be resolved first. The application must
    /// also keep its publication lock across the actual effect so the current
    /// snapshot cannot change between admission and removal.
    pub fn record_replacement(
        &mut self,
        replacement: RuntimeArtifact,
        request: InstallRequest,
    ) -> Result<ReclamationUpdate, ManagedInstallationError> {
        if self
            .replacement_receipt
            .as_ref()
            .is_some_and(|receipt| receipt.settlement() == ReplacementSettlementState::Pending)
        {
            return Err(ManagedInstallationError::SettlementPending);
        }
        if self.pending.iter().any(|pending| {
            matches!(
                pending.operation.as_ref(),
                Some(
                    ReclamationOperation::EffectPending(_)
                        | ReclamationOperation::ObservationPending(_)
                )
            )
        }) {
            return Err(ManagedInstallationError::EffectPending);
        }
        if self.current.physical_identity() == replacement.physical_identity() {
            return Err(ManagedInstallationError::CurrentArtifact);
        }
        if self.pending.iter().any(|pending| {
            obligation_correlations(&pending.obligation).contains(&request.request_id())
        }) {
            return Err(ManagedInstallationError::DuplicateCorrelation);
        }
        if let Some(index) = self.pending.iter().position(|pending| {
            pending.obligation.superseded().physical_identity() == self.current.physical_identity()
        }) {
            if self.pending[index].operation.is_some() {
                return Err(ManagedInstallationError::OperationInProgress);
            }
            let (obligation, activation) = self.pending[index]
                .obligation
                .reactivate(replacement.clone(), request)
                .map_err(|_| ManagedInstallationError::CurrentArtifact)?;
            self.pending[index].obligation = obligation.clone();
            self.current = replacement;
            return Ok(ReclamationUpdate::Reactivated {
                obligation,
                activation,
            });
        }
        let obligation = ReclamationObligation::after_replacement(
            self.current.clone(),
            replacement.clone(),
            request,
        )
        .map_err(|_| ManagedInstallationError::CurrentArtifact)?;
        self.current = replacement;
        self.pending.push(PendingReclamation {
            obligation: obligation.clone(),
            operation: None,
            last_acknowledged_operation_id: None,
        });
        Ok(ReclamationUpdate::Created(obligation))
    }

    /// Bind the authoritative obligation for a replacement to its delivery identity.
    pub fn retain_replacement_receipt(
        &mut self,
        delivery_id: impl Into<String>,
        replacement_request: &InstallRequest,
    ) -> Result<ReplacementReceipt, ManagedInstallationError> {
        let obligation = self
            .pending
            .iter()
            .find(|pending| pending.obligation.activation().request() == replacement_request)
            .map(|pending| pending.obligation.clone())
            .ok_or(ManagedInstallationError::UnknownObligation)?;
        let receipt = ReplacementReceipt::new(delivery_id, obligation)
            .map_err(|_| ManagedInstallationError::DeliveryIdentity)?;
        if let Some(existing) = &self.replacement_receipt {
            if existing == &receipt {
                return Ok(existing.clone());
            }
            if existing.settlement() == ReplacementSettlementState::Pending {
                return Err(ManagedInstallationError::SettlementPending);
            }
        }
        self.replacement_receipt = Some(receipt.clone());
        Ok(receipt)
    }

    /// Confirm that exact cleanup and audit evidence permits publication settlement.
    pub fn replacement_settlement_ready(
        &self,
        delivery_id: &str,
        replacement_request: &InstallRequest,
    ) -> Result<(), ManagedInstallationError> {
        let receipt = self
            .replacement_receipt
            .as_ref()
            .ok_or(ManagedInstallationError::ReceiptObligationMissing)?;
        if receipt.delivery_id() != delivery_id
            || receipt.obligation().activation().request() != replacement_request
        {
            return Err(ManagedInstallationError::DeliveryIdentity);
        }
        if receipt.settlement() == ReplacementSettlementState::Settled {
            return Ok(());
        }
        let pending = self
            .pending
            .iter()
            .find(|pending| pending.obligation() == receipt.obligation())
            .ok_or(ManagedInstallationError::ReceiptObligationMissing)?;
        let acknowledged = matches!(
            pending.operation(),
            Some(ReclamationOperation::EffectRecorded(event))
                if event.audit() == ReclamationAuditState::Acknowledged
        ) || pending.last_acknowledged_operation_id().is_some();
        if !acknowledged {
            return Err(ManagedInstallationError::SettlementBeforeCleanup);
        }
        Ok(())
    }

    /// Acknowledge exact settlement and retire completed cleanup state.
    pub fn acknowledge_replacement_settlement(
        &mut self,
        delivery_id: &str,
        replacement_request: &InstallRequest,
    ) -> Result<ReplacementReceipt, ManagedInstallationError> {
        self.replacement_settlement_ready(delivery_id, replacement_request)?;
        let receipt = self
            .replacement_receipt
            .as_ref()
            .ok_or(ManagedInstallationError::ReceiptObligationMissing)?
            .clone();
        if receipt.delivery_id() != delivery_id
            || receipt.obligation().activation().request() != replacement_request
        {
            return Err(ManagedInstallationError::DeliveryIdentity);
        }
        if receipt.settlement() == ReplacementSettlementState::Settled {
            return Ok(receipt);
        }
        let index = self
            .pending
            .iter()
            .position(|pending| pending.obligation() == receipt.obligation())
            .ok_or(ManagedInstallationError::ReceiptObligationMissing)?;
        let complete = matches!(
            self.pending[index].operation(),
            Some(ReclamationOperation::EffectRecorded(event))
                if event.audit() == ReclamationAuditState::Acknowledged
                    && event.outcome().completes_obligation()
        );
        if complete {
            self.pending.remove(index);
        }
        let settled = receipt.settled();
        self.replacement_receipt = Some(settled.clone());
        Ok(settled)
    }

    /// Admit a fresh effect, or return the work already owned by an exact replay.
    pub fn admit_reclamation(
        &mut self,
        replacement_request: &InstallRequest,
        operation_id: ReclamationOperationId,
        trigger: ReclamationTrigger,
    ) -> Result<AdmissionResult, ManagedInstallationError> {
        let index = self
            .pending
            .iter()
            .position(|pending| pending.obligation.activation().request() == replacement_request)
            .ok_or(ManagedInstallationError::UnknownObligation)?;
        if let Some(operation) = &self.pending[index].operation {
            if operation.admission().operation_id() == &operation_id
                && operation.admission().trigger() == &trigger
            {
                return Ok(AdmissionResult::Existing(work_for(operation)));
            }
            return Err(ManagedInstallationError::OperationInProgress);
        }
        if self.pending[index].last_acknowledged_operation_id.as_ref() == Some(&operation_id)
            || self.operation_id_in_use(&operation_id)
        {
            return Err(ManagedInstallationError::DuplicateOperationId);
        }
        if self.pending[index]
            .obligation
            .superseded()
            .physical_identity()
            == self.current.physical_identity()
        {
            return Err(ManagedInstallationError::CurrentArtifact);
        }
        let admission = ReclamationAdmission::restore(
            self.agent.clone(),
            operation_id.clone(),
            self.pending[index].obligation.clone(),
            self.current.clone(),
            trigger,
        )
        .map_err(|_| ManagedInstallationError::CurrentArtifact)?;
        self.pending[index].operation =
            Some(ReclamationOperation::EffectPending(admission.clone()));
        let permit = RemovalPermit {
            admission: admission.clone(),
        };
        Ok(AdmissionResult::Fresh {
            admission: Box::new(admission),
            permit: Box::new(permit),
        })
    }

    pub fn work(
        &self,
        replacement_request: &InstallRequest,
    ) -> Result<ReclamationWork, ManagedInstallationError> {
        let pending = self
            .pending
            .iter()
            .find(|pending| pending.obligation.activation().request() == replacement_request)
            .ok_or(ManagedInstallationError::UnknownObligation)?;
        Ok(match &pending.operation {
            Some(operation) => work_for(operation),
            None if pending.obligation.superseded().physical_identity()
                == self.current.physical_identity() =>
            {
                ReclamationWork::BlockedCurrent
            }
            None => ReclamationWork::Ready,
        })
    }

    pub fn confirm_removed(
        &mut self,
        permit: RemovalPermit,
    ) -> Result<ReclamationEvent, ManagedInstallationError> {
        self.record_effect(permit, ReclamationPhysicalOutcome::Removed)
    }

    pub fn confirm_already_absent(
        &mut self,
        permit: RemovalPermit,
    ) -> Result<ReclamationEvent, ManagedInstallationError> {
        self.record_effect(permit, ReclamationPhysicalOutcome::AlreadyAbsent)
    }

    pub fn record_still_present(
        &mut self,
        permit: RemovalPermit,
    ) -> Result<ReclamationEvent, ManagedInstallationError> {
        self.record_effect(permit, ReclamationPhysicalOutcome::StillPresent)
    }

    pub fn record_removal_failure(
        &mut self,
        permit: RemovalPermit,
        failure: InstallFailureEvidence,
    ) -> Result<ReclamationEvent, ManagedInstallationError> {
        self.record_effect(permit, ReclamationPhysicalOutcome::RemovalFailed(failure))
    }

    pub fn record_removal_sync_uncertain(
        &mut self,
        permit: RemovalPermit,
        failure: InstallFailureEvidence,
    ) -> Result<ReclamationEvent, ManagedInstallationError> {
        self.record_effect(
            permit,
            ReclamationPhysicalOutcome::RemovalSyncUncertain(failure),
        )
    }

    /// Resolve an interrupted pending effect by observing physical state.
    pub fn record_observation(
        &mut self,
        operation_id: &ReclamationOperationId,
        observation: ReclamationObservation,
    ) -> Result<ReclamationEvent, ManagedInstallationError> {
        let outcome = match observation {
            ReclamationObservation::AlreadyAbsent => ReclamationPhysicalOutcome::AlreadyAbsent,
            ReclamationObservation::StillPresent => ReclamationPhysicalOutcome::StillPresent,
            ReclamationObservation::Failed(failure) => {
                ReclamationPhysicalOutcome::ObservationFailed(failure)
            }
        };
        let index = self
            .pending
            .iter()
            .position(|pending| {
                matches!(
                    &pending.operation,
                    Some(ReclamationOperation::ObservationPending(admission))
                        if admission.operation_id() == operation_id
                )
            })
            .ok_or(ManagedInstallationError::UnknownOperation)?;
        let Some(ReclamationOperation::ObservationPending(admission)) =
            self.pending[index].operation.take()
        else {
            unreachable!("the match above selected a pending effect");
        };
        let event = ReclamationEvent::restore(admission, outcome, ReclamationAuditState::Pending);
        self.pending[index].operation = Some(ReclamationOperation::EffectRecorded(event.clone()));
        Ok(event)
    }

    /// Restore an exact event found in the immutable audit before observing an
    /// interrupted physical effect. Audit evidence is accepted only for the
    /// same admission retained by this aggregate.
    pub fn recover_audited_event(
        &mut self,
        operation_id: &ReclamationOperationId,
        event: ReclamationEvent,
    ) -> Result<ReclamationEvent, ManagedInstallationError> {
        let index = self
            .pending
            .iter()
            .position(|pending| {
                matches!(
                    &pending.operation,
                    Some(ReclamationOperation::ObservationPending(admission))
                        if admission.operation_id() == operation_id
                )
            })
            .ok_or(ManagedInstallationError::UnknownOperation)?;
        let Some(ReclamationOperation::ObservationPending(admission)) =
            self.pending[index].operation.as_ref()
        else {
            unreachable!("the search above selected an interrupted effect");
        };
        if event.admission() != admission || event.audit() != ReclamationAuditState::Pending {
            return Err(ManagedInstallationError::ObligationMismatch);
        }
        self.pending[index].operation = Some(ReclamationOperation::EffectRecorded(event.clone()));
        Ok(event)
    }

    /// Acknowledge one event and retire or reopen its bounded obligation state.
    pub fn acknowledge_audit(
        &mut self,
        operation_id: &ReclamationOperationId,
    ) -> Result<ReclamationEvent, ManagedInstallationError> {
        let index = self
            .pending
            .iter()
            .position(|pending| {
                matches!(
                    &pending.operation,
                    Some(ReclamationOperation::EffectRecorded(event))
                        if event.admission().operation_id() == operation_id
                )
            })
            .ok_or(ManagedInstallationError::UnknownOperation)?;
        let Some(ReclamationOperation::EffectRecorded(event)) =
            self.pending[index].operation.take()
        else {
            unreachable!("the search above selected a recorded effect");
        };
        let acknowledged = event.acknowledged();
        let retire = self.pending[index]
            .obligation
            .superseded()
            .physical_identity()
            == self.current.physical_identity();
        if retire {
            self.pending.remove(index);
        } else if acknowledged.outcome().completes_obligation() {
            self.pending[index].operation =
                Some(ReclamationOperation::EffectRecorded(acknowledged.clone()));
        } else {
            self.pending[index].last_acknowledged_operation_id = Some(operation_id.clone());
        }
        Ok(acknowledged)
    }

    fn record_effect(
        &mut self,
        permit: RemovalPermit,
        outcome: ReclamationPhysicalOutcome,
    ) -> Result<ReclamationEvent, ManagedInstallationError> {
        let index = self
            .pending
            .iter()
            .position(|pending| {
                matches!(
                    &pending.operation,
                    Some(ReclamationOperation::EffectPending(admission))
                        if admission == &permit.admission
                )
            })
            .ok_or(ManagedInstallationError::UnknownOperation)?;
        let Some(ReclamationOperation::EffectPending(admission)) =
            self.pending[index].operation.take()
        else {
            unreachable!("the match above selected a pending effect");
        };
        let event = ReclamationEvent::restore(admission, outcome, ReclamationAuditState::Pending);
        self.pending[index].operation = Some(ReclamationOperation::EffectRecorded(event.clone()));
        Ok(event)
    }

    fn operation_id_in_use(&self, operation_id: &ReclamationOperationId) -> bool {
        self.pending.iter().any(|pending| {
            pending
                .operation
                .as_ref()
                .is_some_and(|operation| operation.admission().operation_id() == operation_id)
                || pending.last_acknowledged_operation_id.as_ref() == Some(operation_id)
        })
    }
}

fn work_for(operation: &ReclamationOperation) -> ReclamationWork {
    match operation {
        ReclamationOperation::EffectPending(admission) => {
            ReclamationWork::EffectInProgress(Box::new(admission.clone()))
        }
        ReclamationOperation::ObservationPending(admission) => {
            ReclamationWork::Observe(Box::new(admission.clone()))
        }
        ReclamationOperation::EffectRecorded(event) => {
            if event.audit() == ReclamationAuditState::Acknowledged {
                ReclamationWork::SettlementPending(Box::new(event.clone()))
            } else {
                ReclamationWork::Audit(Box::new(event.clone()))
            }
        }
    }
}

/// Result of attempting to admit a reclamation operation.
pub enum AdmissionResult {
    Fresh {
        admission: Box<ReclamationAdmission>,
        permit: Box<RemovalPermit>,
    },
    Existing(ReclamationWork),
}

/// Work remaining for a pending obligation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReclamationWork {
    Ready,
    BlockedCurrent,
    EffectInProgress(Box<ReclamationAdmission>),
    Observe(Box<ReclamationAdmission>),
    Audit(Box<ReclamationEvent>),
    SettlementPending(Box<ReclamationEvent>),
}

/// Recovery observation of an effect admitted before a restart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReclamationObservation {
    AlreadyAbsent,
    StillPresent,
    Failed(InstallFailureEvidence),
}

/// Fresh-only authority to report the result of an actual removal attempt.
///
/// This permit is deliberately not cloneable or persistable. Infrastructure
/// must hold the publication lock from admission through the physical effect.
#[must_use]
pub struct RemovalPermit {
    admission: ReclamationAdmission,
}

/// Evidence returned when an installation creates or reactivates reclamation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReclamationUpdate {
    Created(ReclamationObligation),
    Reactivated {
        obligation: ReclamationObligation,
        activation: ReclamationActivation,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagedInstallationError {
    CurrentArtifact,
    EffectPending,
    UnknownObligation,
    UnknownOperation,
    OperationInProgress,
    DuplicatePhysicalTarget,
    DuplicateCorrelation,
    DuplicateOperationId,
    ObligationMismatch,
    AcknowledgedOperationRetained,
    AdmissionOwnerMismatch,
    AdmissionCurrentMismatch,
    SettlementPending,
    DeliveryIdentity,
    SettlementBeforeCleanup,
    ReceiptCurrentMismatch,
    ReceiptObligationMissing,
}

impl fmt::Display for ManagedInstallationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::CurrentArtifact => "the current physical runtime cannot be reclaimed",
            Self::EffectPending => "a physical reclamation effect is still pending",
            Self::UnknownObligation => "the reclamation obligation is unknown",
            Self::UnknownOperation => "the reclamation operation is unknown",
            Self::OperationInProgress => "another reclamation operation is already in progress",
            Self::DuplicatePhysicalTarget => "one physical runtime has conflicting obligations",
            Self::DuplicateCorrelation => "one replacement request has conflicting obligations",
            Self::DuplicateOperationId => "the reclamation operation identity is already in use",
            Self::ObligationMismatch => "reclamation operation and obligation disagree",
            Self::AcknowledgedOperationRetained => {
                "acknowledged reclamation history does not belong in pending state"
            }
            Self::AdmissionOwnerMismatch => {
                "the reclamation admission belongs to another installation"
            }
            Self::AdmissionCurrentMismatch => {
                "pending reclamation admission does not match the current artifact"
            }
            Self::SettlementPending => "the prior replacement settlement is still pending",
            Self::DeliveryIdentity => "replacement settlement delivery identity disagrees",
            Self::SettlementBeforeCleanup => {
                "replacement cannot settle before cleanup and audit acknowledgement"
            }
            Self::ReceiptCurrentMismatch => {
                "replacement receipt activation does not match the current artifact"
            }
            Self::ReceiptObligationMissing => {
                "pending replacement receipt has no matching cleanup obligation"
            }
        })
    }
}

impl std::error::Error for ManagedInstallationError {}

fn obligation_correlations(obligation: &ReclamationObligation) -> [&str; 2] {
    [
        obligation.origin().request().request_id(),
        obligation.activation().request().request_id(),
    ]
}

fn validate_restored_operation(
    agent: &AgentName,
    current: &RuntimeArtifact,
    operation: Option<&ReclamationOperation>,
) -> Result<(), ManagedInstallationError> {
    let Some(operation) = operation else {
        return Ok(());
    };
    if operation.admission().agent() != agent {
        return Err(ManagedInstallationError::AdmissionOwnerMismatch);
    }
    if matches!(
        operation,
        ReclamationOperation::EffectPending(admission)
            | ReclamationOperation::ObservationPending(admission)
            if admission.current() != current
    ) {
        return Err(ManagedInstallationError::AdmissionCurrentMismatch);
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/agent_install/managed_installation.rs"]
mod tests;
