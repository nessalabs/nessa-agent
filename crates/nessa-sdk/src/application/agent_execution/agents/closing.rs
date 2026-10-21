//! The supervised close transaction's cleanup and scheduling evidence.
#![deny(missing_docs)]

use super::AgentError;
use crate::application::agent_execution::providers::{CleanupReport, CloseOutcome};

/// Immutable evidence from one supervised Agent close or suspend transaction.
///
/// The report retains the actual lifecycle cleanup, including executable-use
/// release still pending, separately from cancellation persistence/audit. A
/// cleanup diagnostic does not determine physical ownership. Obtain this value
/// from [`super::Agent::close_report`] or [`super::Agent::suspend_report`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCloseReport {
    cleanup: CleanupReport,
    scheduling: Result<(), AgentError>,
    decline: Result<(), AgentError>,
}

impl AgentCloseReport {
    pub(super) fn new(cleanup: CleanupReport, scheduling: Result<(), AgentError>) -> Self {
        Self {
            cleanup,
            scheduling: scheduling.map_err(AgentError::bounded),
            decline: Ok(()),
        }
    }

    pub(super) fn with_decline(self, decline: Result<(), AgentError>) -> Self {
        Self {
            decline: decline.map_err(AgentError::bounded),
            ..self
        }
    }
    /// Historical decline evidence, independent of provider cleanup and pending cancellation.
    pub fn decline(&self) -> &Result<(), AgentError> {
        &self.decline
    }

    /// This transaction's actual provider cleanup and independent evidence.
    /// Inspect the complete resources value: a physical outcome can coexist
    /// with a retained executable-use release owner.
    pub fn cleanup(&self) -> &CleanupReport {
        &self.cleanup
    }

    /// Pending-work cancellation persistence and audit acknowledgement.
    /// Failure here never substitutes for the cleanup resource observation.
    pub fn scheduling(&self) -> &Result<(), AgentError> {
        &self.scheduling
    }

    /// Project the transaction into the ordinary close result.
    /// Storage failures retain the cleanup result; other scheduling failures
    /// retain any independent cleanup failure alongside them. The cleanup
    /// report's historical operation diagnostic keeps its existing meaning.
    pub fn into_result(self) -> Result<CloseOutcome, AgentError> {
        let cleanup = self.cleanup.into_result();
        match join_acknowledgements(self.scheduling, self.decline) {
            Ok(()) => cleanup,
            Err(AgentError::Storage(error)) => Err(AgentError::StorageDuringClose {
                error,
                cleanup_result: Box::new(cleanup),
            }),
            Err(error) => Err(match cleanup {
                Ok(_) => error,
                Err(cleanup) => AgentError::MultipleOperationFailures {
                    first_error: Box::new(error),
                    subsequent_error: Box::new(cleanup),
                },
            }),
        }
    }

    /// Replace a physical resource observation with the real encompassing
    /// environment owner's answer, preserving scheduling and other evidence.
    /// This consumes the report; it does not mutate or release the Agent's
    /// retained cleanup owner. Callers must retain any independent SDK owner.
    pub fn with_cleanup(self, cleanup: CleanupReport) -> Self {
        Self { cleanup, ..self }
    }

    /// Join a later observation from the same cleanup owner. Only its actual
    /// resource report supersedes the earlier resource observation. Independent
    /// evidence failures from either attempt remain observable, with cached
    /// identical failures represented once. This value releases no owner.
    pub fn after_retry(self, retry: Self) -> Self {
        let audit =
            join_acknowledgements(self.cleanup.audit().clone(), retry.cleanup.audit().clone());
        let completion = join_failures(
            self.cleanup.completion_failure().cloned(),
            retry.cleanup.completion_failure().cloned(),
        );
        let operation = join_failures(
            self.cleanup.operation_failure().cloned(),
            retry.cleanup.operation_failure().cloned(),
        );
        let cleanup = self
            .cleanup
            .with_resources(retry.cleanup.resources().clone())
            .with_audit(audit)
            .with_completion_failure(completion)
            .with_operation_failure(operation);
        Self::new(
            cleanup,
            join_acknowledgements(self.scheduling, retry.scheduling),
        )
        .with_decline(join_acknowledgements(self.decline, retry.decline))
    }
}

fn join_failures(first: Option<AgentError>, second: Option<AgentError>) -> Option<AgentError> {
    match (first, second) {
        (None, other) | (other, None) => other,
        (Some(first), Some(second)) if first == second => Some(first),
        (Some(first), Some(second)) => Some(
            AgentError::MultipleOperationFailures {
                first_error: Box::new(first),
                subsequent_error: Box::new(second),
            }
            .bounded(),
        ),
    }
}

pub(super) fn join_acknowledgements(
    first: Result<(), AgentError>,
    second: Result<(), AgentError>,
) -> Result<(), AgentError> {
    join_failures(first.err(), second.err()).map_or(Ok(()), Err)
}
