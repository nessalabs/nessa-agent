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

pub(super) type AttachmentEvidenceCompletion = watch::Sender<Option<Result<(), AgentError>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AttachmentEvidenceTransition {
    Started,
    ContextPublished,
    Failed,
    AuthorizationAbandoned,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AttachmentFailureSource {
    Independent,
    Transition(u64, AttachmentEvidenceTransition),
    FailedOpenCleanup(u64),
}

pub(super) struct AttachmentAttemptFailure {
    facts: Vec<(AgentError, AttachmentFailureSource)>,
}
impl AttachmentAttemptFailure {
    pub(super) fn new(error: AgentError, source: AttachmentFailureSource) -> Self {
        Self {
            facts: vec![(error, source)],
        }
    }
    pub(super) fn error(&self) -> AgentError {
        Self::combine(self.facts.iter().map(|(error, _)| error.clone()))
            .expect("attachment failure has at least one fact")
    }
    pub(super) fn retain_independent(&mut self, error: AgentError) {
        self.retain(error, AttachmentFailureSource::Independent);
    }
    pub(super) fn retain(&mut self, error: AgentError, source: AttachmentFailureSource) {
        self.facts.push((error, source));
    }
    pub(super) fn with_cleanup(
        self,
        ticket: &CloseAttempt,
        cleanup: Result<(), AgentError>,
    ) -> AgentError {
        let cleanup_failed = cleanup.is_err();
        let independent = self.facts.into_iter().filter_map(|(error, source)| {
            let represented = match source {
                AttachmentFailureSource::Independent => false,
                AttachmentFailureSource::Transition(generation, transition) => ticket
                    .evidence
                    .iter()
                    .any(|slot| slot.generation == generation && slot.transition == transition),
                AttachmentFailureSource::FailedOpenCleanup(generation) => ticket
                    .evidence
                    .iter()
                    .any(|slot| slot.generation == generation),
            };
            (!cleanup_failed || !represented).then_some(error)
        });
        Self::combine(independent.chain(cleanup.err()))
            .expect("attachment failure or cleanup remains observable")
    }
    fn combine(errors: impl IntoIterator<Item = AgentError>) -> Option<AgentError> {
        errors.into_iter().fold(None, |combined, error| {
            Some(match combined {
                Some(first_error) => AgentError::MultipleOperationFailures {
                    first_error: Box::new(first_error),
                    subsequent_error: Box::new(error),
                },
                None => error,
            })
        })
    }
}
impl From<AgentError> for AttachmentAttemptFailure {
    fn from(error: AgentError) -> Self {
        Self::new(error, AttachmentFailureSource::Independent)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::agent_execution::providers::CloseOutcome;

    fn started_ticket() -> CloseAttempt {
        let (completion, result) = watch::channel(Some(Err(AgentError::AuditFailure)));
        drop(completion);
        CloseAttempt::from_physical_report(
            1,
            1,
            SessionCloseRequest::SessionFailed,
            CleanupReport::confirmed(CloseOutcome { forced: false }),
            vec![AttachmentEvidenceSlot {
                generation: 7,
                transition: AttachmentEvidenceTransition::Started,
                result,
            }],
        )
    }

    #[test]
    fn source_identity_suppresses_only_the_fact_folded_by_cleanup() {
        let ticket = started_ticket();
        let same_source = AttachmentAttemptFailure::new(
            AgentError::AuditFailure,
            AttachmentFailureSource::Transition(7, AttachmentEvidenceTransition::Started),
        );
        assert_eq!(
            same_source.with_cleanup(&ticket, Err(AgentError::AuditFailure)),
            AgentError::AuditFailure
        );

        let mut independent_equal = AttachmentAttemptFailure::new(
            AgentError::AuditFailure,
            AttachmentFailureSource::Transition(7, AttachmentEvidenceTransition::Started),
        );
        independent_equal.retain_independent(AgentError::AuditFailure);
        assert_eq!(
            independent_equal.with_cleanup(&ticket, Err(AgentError::AuditFailure)),
            AgentError::MultipleOperationFailures {
                first_error: Box::new(AgentError::AuditFailure),
                subsequent_error: Box::new(AgentError::AuditFailure),
            }
        );
    }
}
