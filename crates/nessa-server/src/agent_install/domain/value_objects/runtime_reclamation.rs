use std::fmt;

use super::{AgentName, InstallFailureEvidence, InstallRequest, RuntimeArtifact};

/// Whether the exact replacement publication has been durably settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplacementSettlementState {
    /// Cleanup ownership exists, but settlement evidence is not acknowledged.
    Pending,
    /// The exact delivery settlement has been acknowledged by the aggregate.
    Settled,
}

/// Immutable correlation between a replacement delivery and its cleanup obligation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplacementReceipt {
    delivery_id: String,
    obligation: ReclamationObligation,
    settlement: ReplacementSettlementState,
}

impl ReplacementReceipt {
    /// Create a pending receipt for the exact delivery identity and obligation.
    pub fn new(
        delivery_id: impl Into<String>,
        obligation: ReclamationObligation,
    ) -> Result<Self, ReclamationError> {
        Self::restore(delivery_id, obligation, ReplacementSettlementState::Pending)
    }

    /// Restore a receipt after validating its bounded delivery identity.
    pub fn restore(
        delivery_id: impl Into<String>,
        obligation: ReclamationObligation,
        settlement: ReplacementSettlementState,
    ) -> Result<Self, ReclamationError> {
        let delivery_id = delivery_id.into();
        if delivery_id.is_empty()
            || delivery_id.len() > 255
            || delivery_id.chars().any(char::is_control)
        {
            return Err(ReclamationError::DeliveryId);
        }
        Ok(Self {
            delivery_id,
            obligation,
            settlement,
        })
    }

    /// Return the durable delivery record identity.
    pub fn delivery_id(&self) -> &str {
        &self.delivery_id
    }

    /// Return the exact cleanup obligation bound to the replacement.
    pub fn obligation(&self) -> &ReclamationObligation {
        &self.obligation
    }

    /// Return whether publication settlement has been acknowledged.
    pub fn settlement(&self) -> ReplacementSettlementState {
        self.settlement
    }

    pub(crate) fn settled(&self) -> Self {
        Self {
            delivery_id: self.delivery_id.clone(),
            obligation: self.obligation.clone(),
            settlement: ReplacementSettlementState::Settled,
        }
    }
}

/// A durable obligation to reclaim one superseded physical runtime artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReclamationObligation {
    superseded: RuntimeArtifact,
    origin: ReclamationActivation,
    activation: ReclamationActivation,
}

impl ReclamationObligation {
    /// Retain the exact replacement facts that made an artifact superseded.
    pub fn after_replacement(
        superseded: RuntimeArtifact,
        replacement: RuntimeArtifact,
        replacement_request: InstallRequest,
    ) -> Result<Self, ReclamationError> {
        let activation = ReclamationActivation::new(replacement, replacement_request);
        Self::restore(superseded, activation.clone(), activation)
    }

    /// Restore immutable origin and current activation facts.
    pub fn restore(
        superseded: RuntimeArtifact,
        origin: ReclamationActivation,
        activation: ReclamationActivation,
    ) -> Result<Self, ReclamationError> {
        if superseded.physical_identity() == origin.replacement().physical_identity()
            || superseded.physical_identity() == activation.replacement().physical_identity()
        {
            return Err(ReclamationError::CurrentArtifact);
        }
        if origin.request().request_id() == activation.request().request_id()
            && origin != activation
        {
            return Err(ReclamationError::ConflictingActivation);
        }
        if origin.request().account_id() != activation.request().account_id() {
            return Err(ReclamationError::OwnerMismatch);
        }
        Ok(Self {
            superseded,
            origin,
            activation,
        })
    }

    pub fn superseded(&self) -> &RuntimeArtifact {
        &self.superseded
    }

    /// The replacement that first made this physical artifact superseded.
    pub fn origin(&self) -> &ReclamationActivation {
        &self.origin
    }

    /// The replacement that currently makes this physical artifact superseded.
    pub fn activation(&self) -> &ReclamationActivation {
        &self.activation
    }

    /// Create a replacement obligation value without losing its origin.
    pub fn reactivate(
        &self,
        replacement: RuntimeArtifact,
        request: InstallRequest,
    ) -> Result<(Self, ReclamationActivation), ReclamationError> {
        let activation = ReclamationActivation::new(replacement, request);
        let obligation = Self::restore(
            self.superseded.clone(),
            self.origin.clone(),
            activation.clone(),
        )?;
        Ok((obligation, activation))
    }
}

/// One replacement fact that activates a reclamation obligation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReclamationActivation {
    replacement: RuntimeArtifact,
    request: InstallRequest,
}

impl ReclamationActivation {
    pub fn new(replacement: RuntimeArtifact, request: InstallRequest) -> Self {
        Self {
            replacement,
            request,
        }
    }

    pub fn replacement(&self) -> &RuntimeArtifact {
        &self.replacement
    }

    pub fn request(&self) -> &InstallRequest {
        &self.request
    }
}

/// Stable identity of one physical reclamation attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReclamationOperationId(String);

impl ReclamationOperationId {
    /// Validate an application-supplied operation identity.
    pub fn new(value: impl Into<String>) -> Result<Self, ReclamationError> {
        let value = value.into();
        if value.is_empty() || value.len() > 255 || value.chars().any(char::is_control) {
            return Err(ReclamationError::OperationId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why reclamation work was admitted, with its initiator constrained by shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReclamationTrigger {
    ReplacementFollowUp,
    LaterInstallation(InstallRequest),
    ProcessRecovery,
    CallerRetry(InstallRequest),
}

/// Stable cause projected from a reclamation trigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReclamationCause {
    ReplacementFollowUp,
    LaterInstallation,
    ProcessRecovery,
    Retry,
}

impl ReclamationTrigger {
    pub fn cause(&self) -> ReclamationCause {
        match self {
            Self::ReplacementFollowUp => ReclamationCause::ReplacementFollowUp,
            Self::LaterInstallation(_) => ReclamationCause::LaterInstallation,
            Self::ProcessRecovery => ReclamationCause::ProcessRecovery,
            Self::CallerRetry(_) => ReclamationCause::Retry,
        }
    }

    /// The verified caller for an explicit retry, or none for automatic work.
    pub fn caller(&self) -> Option<&InstallRequest> {
        match self {
            Self::CallerRetry(request) => Some(request),
            Self::ReplacementFollowUp | Self::LaterInstallation(_) | Self::ProcessRecovery => None,
        }
    }

    pub fn triggering_install(&self) -> Option<&InstallRequest> {
        match self {
            Self::LaterInstallation(request) => Some(request),
            _ => None,
        }
    }
}

/// Durable admission recorded before a physical reclamation attempt starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReclamationAdmission {
    agent: AgentName,
    operation_id: ReclamationOperationId,
    obligation: ReclamationObligation,
    current: RuntimeArtifact,
    trigger: ReclamationTrigger,
}

impl ReclamationAdmission {
    /// Restore admission facts while rechecking their physical relationship.
    pub fn restore(
        agent: AgentName,
        operation_id: ReclamationOperationId,
        obligation: ReclamationObligation,
        current: RuntimeArtifact,
        trigger: ReclamationTrigger,
    ) -> Result<Self, ReclamationError> {
        if obligation.superseded().physical_identity() == current.physical_identity() {
            return Err(ReclamationError::CurrentArtifact);
        }
        let owner = obligation.origin().request().account_id();
        if trigger
            .triggering_install()
            .or_else(|| trigger.caller())
            .is_some_and(|request| request.account_id() != owner)
        {
            return Err(ReclamationError::OwnerMismatch);
        }
        Ok(Self {
            agent,
            operation_id,
            obligation,
            current,
            trigger,
        })
    }

    pub fn agent(&self) -> &AgentName {
        &self.agent
    }

    pub fn operation_id(&self) -> &ReclamationOperationId {
        &self.operation_id
    }

    pub fn obligation(&self) -> &ReclamationObligation {
        &self.obligation
    }

    pub fn current(&self) -> &RuntimeArtifact {
        &self.current
    }

    pub fn trigger(&self) -> &ReclamationTrigger {
        &self.trigger
    }
}

/// What one effect or recovery observation established.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReclamationPhysicalOutcome {
    Removed,
    AlreadyAbsent,
    StillPresent,
    RemovalFailed(InstallFailureEvidence),
    RemovalSyncUncertain(InstallFailureEvidence),
    ObservationFailed(InstallFailureEvidence),
}

impl ReclamationPhysicalOutcome {
    pub fn completes_obligation(&self) -> bool {
        matches!(self, Self::Removed | Self::AlreadyAbsent)
    }
}

/// Whether durable audit storage acknowledged a recorded outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReclamationAuditState {
    Pending,
    Acknowledged,
}

/// Immutable evidence for one admitted reclamation operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReclamationEvent {
    admission: ReclamationAdmission,
    outcome: ReclamationPhysicalOutcome,
    audit: ReclamationAuditState,
}

impl ReclamationEvent {
    pub fn restore(
        admission: ReclamationAdmission,
        outcome: ReclamationPhysicalOutcome,
        audit: ReclamationAuditState,
    ) -> Self {
        Self {
            admission,
            outcome,
            audit,
        }
    }

    pub fn admission(&self) -> &ReclamationAdmission {
        &self.admission
    }

    pub fn outcome(&self) -> &ReclamationPhysicalOutcome {
        &self.outcome
    }

    pub fn audit(&self) -> ReclamationAuditState {
        self.audit
    }

    pub(crate) fn acknowledged(&self) -> Self {
        Self {
            admission: self.admission.clone(),
            outcome: self.outcome.clone(),
            audit: ReclamationAuditState::Acknowledged,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReclamationError {
    CurrentArtifact,
    ConflictingActivation,
    OwnerMismatch,
    DeliveryId,
    OperationId,
}

impl fmt::Display for ReclamationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::CurrentArtifact => "the current physical runtime cannot be reclaimed",
            Self::ConflictingActivation => {
                "one replacement request cannot describe conflicting obligation activations"
            }
            Self::OwnerMismatch => {
                "reclamation origin, activation, and trigger must have one owner account"
            }
            Self::DeliveryId => "replacement delivery identity must be non-empty plain text",
            Self::OperationId => "reclamation operation identity must be non-empty plain text",
        })
    }
}

impl std::error::Error for ReclamationError {}

#[cfg(test)]
#[path = "../../../../tests/agent_install/runtime_reclamation.rs"]
mod tests;
