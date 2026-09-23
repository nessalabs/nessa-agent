//! Validated recipes retain finalized execution failures without conflating sources.
//!
//! ```text
//! adapter facts -> FinalizedExecutionProjection -> ExecutionReport -> storage
//! ```
//! The arrows preserve one ordered recipe. Consumer and cleanup diagnostics are
//! derived views; neither becomes lifecycle or resource authority.

use crate::application::agent_execution::agents::AgentError;
use crate::domain::agent_execution::executions::ExecutionOutcome;

const MAX_FINALIZED_FACTS_PER_CATEGORY: usize = 128;
const MAX_PROJECTED_FAILURES: usize = 32;

/// One authoritative finalized execution failure fact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FinalizedFailureComponent {
    /// An operation failed independently of audit delivery.
    Operation(AgentError),
    /// One required audit attempt failed.
    Audit,
    /// One permission write and its correlated delivery audit both failed.
    PermissionDeliveryAndAudit { delivery_error: AgentError },
    /// More operation facts existed than the retained recipe can hold.
    OperationOverflow,
    /// More audit facts existed than the retained recipe can hold.
    AuditOverflow,
}

/// Validated, ordered authority for a finalized execution's secondary failures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FinalizedExecutionProjection {
    components: Vec<FinalizedFailureComponent>,
}

impl FinalizedExecutionProjection {
    /// Validate and normalize a finalized recipe without replacing it by a
    /// smaller consumer projection.
    pub(crate) fn new(components: Vec<FinalizedFailureComponent>) -> Result<Self, &'static str> {
        let components = components
            .into_iter()
            .map(|component| match component {
                FinalizedFailureComponent::Operation(error) => {
                    FinalizedFailureComponent::Operation(error.bounded())
                }
                FinalizedFailureComponent::PermissionDeliveryAndAudit { delivery_error } => {
                    FinalizedFailureComponent::PermissionDeliveryAndAudit {
                        delivery_error: delivery_error.bounded(),
                    }
                }
                component => component,
            })
            .collect::<Vec<_>>();
        let mut operations = 0usize;
        let mut audits = 0usize;
        let mut operation_overflow = false;
        let mut audit_overflow = false;
        for component in &components {
            let (operation, audit) = match component {
                FinalizedFailureComponent::Operation(_) => (true, false),
                FinalizedFailureComponent::Audit => (false, true),
                FinalizedFailureComponent::PermissionDeliveryAndAudit { .. } => (true, true),
                FinalizedFailureComponent::OperationOverflow => {
                    if operation_overflow {
                        return Err("finalized operation overflow is repeated");
                    }
                    operation_overflow = true;
                    (false, false)
                }
                FinalizedFailureComponent::AuditOverflow => {
                    if audit_overflow {
                        return Err("finalized audit overflow is repeated");
                    }
                    audit_overflow = true;
                    (false, false)
                }
            };
            if operation && operation_overflow {
                return Err("finalized operation follows its overflow marker");
            }
            if audit && audit_overflow {
                return Err("finalized audit follows its overflow marker");
            }
            operations = operations.saturating_add(usize::from(operation));
            audits = audits.saturating_add(usize::from(audit));
            if operations > MAX_FINALIZED_FACTS_PER_CATEGORY
                || audits > MAX_FINALIZED_FACTS_PER_CATEGORY
            {
                return Err("finalized execution failure facts exceed their category budget");
            }
        }
        Ok(Self { components })
    }

    /// Ordered authoritative components for storage at the current contract.
    pub(crate) fn components(&self) -> &[FinalizedFailureComponent] {
        &self.components
    }

    pub(super) fn operation_failure(&self) -> Option<AgentError> {
        Self::project_category(
            self.components
                .iter()
                .filter_map(|component| match component {
                    FinalizedFailureComponent::Operation(error) => Some(error.clone()),
                    FinalizedFailureComponent::PermissionDeliveryAndAudit { delivery_error } => {
                        Some(delivery_error.clone())
                    }
                    FinalizedFailureComponent::OperationOverflow => {
                        Some(AgentError::DiagnosticLimit)
                    }
                    _ => None,
                }),
        )
    }

    pub(super) fn audit_result(&self) -> Result<(), AgentError> {
        Self::project_category(
            self.components
                .iter()
                .filter_map(|component| match component {
                    FinalizedFailureComponent::Audit
                    | FinalizedFailureComponent::PermissionDeliveryAndAudit { .. } => {
                        Some(AgentError::AuditFailure)
                    }
                    FinalizedFailureComponent::AuditOverflow => Some(AgentError::DiagnosticLimit),
                    _ => None,
                }),
        )
        .map_or(Ok(()), Err)
    }

    pub(super) fn consumer_failure(&self) -> Option<AgentError> {
        let errors = self
            .components
            .iter()
            .map(|component| match component {
                FinalizedFailureComponent::Operation(error) => error.clone(),
                FinalizedFailureComponent::Audit => AgentError::AuditFailure,
                FinalizedFailureComponent::PermissionDeliveryAndAudit { delivery_error } => {
                    AgentError::PermissionAnswerDeliveryAndAuditFailure {
                        delivery_error: Box::new(delivery_error.clone()),
                        cleanup_error: None,
                    }
                }
                FinalizedFailureComponent::OperationOverflow
                | FinalizedFailureComponent::AuditOverflow => AgentError::DiagnosticLimit,
            })
            .collect::<Vec<_>>();
        if errors.len() > MAX_PROJECTED_FAILURES {
            Some(AgentError::DiagnosticLimit)
        } else {
            Self::balanced(&errors).map(AgentError::bounded)
        }
    }

    fn project_category(errors: impl IntoIterator<Item = AgentError>) -> Option<AgentError> {
        let mut errors = errors.into_iter().collect::<Vec<_>>();
        if errors.len() >= MAX_PROJECTED_FAILURES {
            errors.truncate(MAX_PROJECTED_FAILURES - 1);
            errors.push(AgentError::DiagnosticLimit);
        }
        Self::balanced(&errors).map(AgentError::bounded)
    }

    fn balanced(errors: &[AgentError]) -> Option<AgentError> {
        match errors {
            [] => None,
            [error] => Some(error.clone()),
            errors => {
                let middle = errors.len() / 2;
                Some(AgentError::MultipleOperationFailures {
                    first_error: Box::new(Self::balanced(&errors[..middle])?),
                    subsequent_error: Box::new(Self::balanced(&errors[middle..])?),
                })
            }
        }
    }
}

/// Finalized origin retained with its authoritative recipe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FinalizedExecutionSource {
    /// Actual provider outcome, or `None` while it remains unknown.
    Provider(Option<Result<ExecutionOutcome, AgentError>>),
    /// The owning Agent settled the invocation locally.
    LocalCancellation,
}
