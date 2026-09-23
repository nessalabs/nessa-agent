//! Joins one attachment attempt's required audit facts with each cleanup attempt.
//!
//! ```text
//! attachment transitions -> evidence slots -> cleanup attempt -> final stop report
//! ```
//! Arrows show ownership. Slot identity prevents repeat finalizers from folding the
//! same fact twice, while each physical retry retains its own immutable result.

use super::AgentError;
use crate::application::agent_execution::providers::{CleanupReport, SessionCloseRequest};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AttachmentEvidenceTransition {
    Started,
    ContextPublished,
    Failed,
    AuthorizationAbandoned,
    Closed,
}

#[derive(Clone)]
pub(super) struct AttachmentEvidenceSlot {
    pub(super) generation: u64,
    pub(super) transition: AttachmentEvidenceTransition,
    pub(super) result: watch::Receiver<Option<Result<(), AgentError>>>,
}

#[derive(Clone)]
pub(super) struct CloseAttempt {
    pub(super) id: u64,
    pub(super) attempt_id: u64,
    pub(super) request: SessionCloseRequest,
    pub(super) physical: watch::Receiver<Option<CleanupReport>>,
    pub(super) result: watch::Receiver<Option<CleanupReport>>,
    pub(super) completion: watch::Sender<Option<CleanupReport>>,
    pub(super) evidence: Vec<AttachmentEvidenceSlot>,
    pub(super) folded_evidence: Vec<(u64, AttachmentEvidenceTransition)>,
}
impl CloseAttempt {
    pub(super) fn from_physical_report(
        id: u64,
        attempt_id: u64,
        request: SessionCloseRequest,
        report: CleanupReport,
        evidence: Vec<AttachmentEvidenceSlot>,
    ) -> Self {
        let (physical_completion, physical) = watch::channel(Some(report.clone()));
        let (completion, result) = watch::channel(None);
        drop(physical_completion);
        Self {
            id,
            attempt_id,
            request,
            physical,
            result,
            completion,
            evidence,
            folded_evidence: Vec::new(),
        }
    }
    pub(super) async fn wait_physical(mut self) -> CleanupReport {
        loop {
            if let Some(result) = self.physical.borrow().clone() {
                return result;
            }
            if self.physical.changed().await.is_err() {
                return CleanupReport::unconfirmed(AgentError::CleanupUncertain);
            }
        }
    }
    pub(super) async fn wait_evidence(&self) -> Result<(), AgentError> {
        let mut failure = None;
        for slot in &self.evidence {
            let identity = (slot.generation, slot.transition);
            if self.folded_evidence.contains(&identity) {
                continue;
            }
            let mut result = slot.result.clone();
            let outcome = loop {
                if let Some(outcome) = result.borrow().clone() {
                    break outcome;
                }
                if result.changed().await.is_err() {
                    break Err(AgentError::AuditFailure);
                }
            };
            if let Err(error) = outcome {
                failure = Some(match failure {
                    Some(first) => AgentError::MultipleOperationFailures {
                        first_error: Box::new(first),
                        subsequent_error: Box::new(error),
                    },
                    None => error,
                });
            }
        }
        failure.map_or(Ok(()), Err)
    }
}
