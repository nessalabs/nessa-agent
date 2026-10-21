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
}

impl AgentCloseReport {
    pub(super) fn new(cleanup: CleanupReport, scheduling: Result<(), AgentError>) -> Self {
        Self {
            cleanup,
            scheduling: scheduling.map_err(AgentError::bounded),
        }
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
        match self.scheduling {
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
}
