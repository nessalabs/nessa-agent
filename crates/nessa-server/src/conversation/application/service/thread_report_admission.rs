//! Report provenance, exact Stop receipts, and person admission delivery fences.
//!
//! ```text
//! durable row + validated delivery -> SDK canonical successor -> capture/delivery
//! captured Stop receipt -> withdrawal or uncertainty -> retained obligation
//! later person command -> release report pause -> new delivery identity
//! ```
//!
//! Arrows mean facts consumed here, not another execution-state authority.
//! Provider results remain distinct from withdrawal and report acknowledgement.
use super::super::{SubmittedFile, SubmittedImage, SubmittedMessage, SubmittedUpload};
use super::{
    conversation_session, ConversationCaller, ConversationError, ConversationId,
    ConversationService, SubmissionMode,
};
use nessa_sdk::application::agent_execution::{
    agents::resumed_turn_id,
    commands::{MutationOperation, MutationOutcome, MutationStage},
    sessions::InvocationRecord,
};
use nessa_sdk::domain::agent_execution::{
    executions::{ExecutionId, SubmissionMode as InvocationSubmission},
    prompts::MessageSender,
};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

/// A new delivery after withdrawal belongs to the next explicit person turn.
/// The withdrawn turn stays final. The existing waiting report row owns the
/// obligation; the parent transcript owns each exact delivery identity.
fn delivery_id(original: &str, person: &str) -> String {
    let bytes = serde_json::to_vec(&(original, person)).expect("strings serialize");
    format!("thread-report-{:x}", Sha256::digest(bytes))
}
/// One delivery and the successors admitted by the SDK. Keeping the original
/// separately makes the current identity available before any successor exists.
pub(super) struct ReportDelivery {
    original: ExecutionId,
    successors: Vec<ExecutionId>,
}
impl ReportDelivery {
    pub(super) fn current(&self) -> &ExecutionId {
        self.successors.last().unwrap_or(&self.original)
    }

    pub(super) fn execution_ids(&self) -> impl Iterator<Item = &ExecutionId> {
        std::iter::once(&self.original).chain(self.successors.iter())
    }
}

/// Iterate continuations established by the SDK's canonical admission owner.
fn resumed_executions<'history>(
    original: &ExecutionId,
    history: &'history [InvocationRecord],
) -> impl Iterator<Item = ExecutionId> + 'history {
    std::iter::successors(resumed_turn_id(history, original), move |execution| {
        resumed_turn_id(history, execution)
    })
}

/// Missing source or selected Stop admissions cannot establish an unstopped
/// source chain. History is observed after the Stop receipts it must cover.
pub(super) fn source_chain_is_unstopped(
    source: &str,
    history: Option<&[InvocationRecord]>,
    stopped: &HashSet<String>,
) -> bool {
    let Some(history) = history else {
        return false;
    };
    let admitted: HashSet<_> = history
        .iter()
        .map(|record| record.request.execution_id.as_str())
        .collect();
    if !stopped
        .iter()
        .all(|execution| admitted.contains(execution.as_str()))
    {
        return false;
    }
    let Some(record) = history
        .iter()
        .find(|record| record.request.execution_id.as_str() == source)
    else {
        return false;
    };
    !stopped.contains(source)
        && resumed_executions(&record.request.execution_id, history)
            .all(|execution| !stopped.contains(execution.as_str()))
}

/// Admitted execution chains descended from validated report deliveries.
/// The SDK alone validates each canonical successor; continuation notice text
/// and caller-chosen ID prefixes cannot confer report provenance.
pub(super) fn report_deliveries(
    original: &str,
    history: &[InvocationRecord],
    mut is_delivery: impl FnMut(&InvocationRecord) -> bool,
) -> Vec<ReportDelivery> {
    let candidates: HashSet<_> = std::iter::once(original.to_owned())
        .chain(
            history
                .iter()
                .map(|record| delivery_id(original, record.request.execution_id.as_str())),
        )
        .collect();
    let mut deliveries = Vec::new();
    for record in history {
        if !candidates.contains(record.request.execution_id.as_str()) || !is_delivery(record) {
            continue;
        }
        let delivery = ReportDelivery {
            original: record.request.execution_id.clone(),
            successors: resumed_executions(&record.request.execution_id, history).collect(),
        };
        deliveries.push(delivery);
    }
    deliveries
}
/// Both durable source binding and saved submission identify a report. A
/// caller-chosen execution ID alone cannot confer ownership over queued work.
pub(super) fn is_report_record(
    pending: &super::thread_reports::PendingReport,
    record: &InvocationRecord,
) -> bool {
    let header = format!(
        "[Thread report] Thread {} finished the turn you sent it (execution {}): ",
        pending.child, pending.execution
    );
    let source_matches = match record.request.user_message.sender() {
        MessageSender::Agent(source) => {
            source.conversation() == pending.child.to_string()
                && source.execution().as_str() == pending.execution.as_str()
        }
        // Retain reports persisted before typed agent authorship. Canonical
        // person-command receipts are excluded separately during Stop capture.
        MessageSender::Person => true,
        _ => false,
    };
    pending
        .report
        .caller
        .actor()
        .is_ok_and(|actor| actor == record.actor)
        && source_matches
        && record.request.user_message.text_str().starts_with(&header)
        && record.request.user_message.images().is_empty()
        && record.request.user_message.files().is_empty()
}

impl ConversationService {
    /// Consume exact mutation receipts, not a provider result's spelling.
    /// Declining owed work preserves its known failed result. That result
    /// does not pay the report obligation. Unconfirmed effects retain it too.
    pub(super) async fn stopped_report_executions(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<HashSet<String>, ConversationError> {
        if self.inner.ownership.get().is_none() {
            return Ok(HashSet::new());
        }
        let selected: Vec<_> = self
            .captured_stops(caller)
            .await?
            .into_iter()
            .map(|(_, target)| target)
            .filter(|target| target.target() == &conversation_session(id))
            .collect();
        if selected.is_empty() {
            return Ok(HashSet::new());
        }
        let storage = self.commands().ok_or(ConversationError::Unavailable)?;
        let receipts = storage
            .mutations(caller.principal_id.as_str().into())
            .await
            .map_err(|_| ConversationError::Unavailable)?;
        let mut stopped = HashSet::new();
        for binding in selected {
            let receipt = receipts
                .iter()
                .find(|receipt| receipt.binding() == &binding)
                .ok_or(ConversationError::Unavailable)?;
            if receipt.stage() != MutationStage::Settled
                || matches!(
                    receipt.outcome(),
                    Some(MutationOutcome::Withdrawn | MutationOutcome::Cancelled)
                )
            {
                stopped.insert(binding.turn_id().as_str().to_owned());
            }
        }
        Ok(stopped)
    }

    pub(super) async fn person_turns(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
        history: &[InvocationRecord],
    ) -> Result<Vec<(usize, String)>, ConversationError> {
        let Some(storage) = self.commands() else {
            return Ok(Vec::new());
        };
        let receipts = storage
            .mutations(caller.principal_id.as_str().into())
            .await
            .map_err(|_| ConversationError::Unavailable)?;
        let mut people = Vec::new();
        for record in history {
            if record.request.user_message.sender() != &MessageSender::Person {
                continue;
            }
            let message = SubmittedMessage {
                text: record.request.user_message.text_str().into(),
                images: record
                    .request
                    .user_message
                    .images()
                    .iter()
                    .map(|image| SubmittedImage {
                        digest: image.digest().to_string(),
                        media_type: image.media_type().as_str().into(),
                        size: image.size(),
                    })
                    .collect(),
                uploads: record
                    .request
                    .user_message
                    .uploads()
                    .iter()
                    .map(|file| SubmittedUpload {
                        digest: file.digest().to_string(),
                        media_type: file.media_type().as_str().into(),
                        size: file.size(),
                        name: file.name().into(),
                    })
                    .collect(),
                files: record
                    .request
                    .user_message
                    .files()
                    .iter()
                    .map(|file| SubmittedFile {
                        path: file.path().into(),
                    })
                    .collect(),
            };
            let mut sender = caller.clone();
            sender.surface_id = record.actor.surface_id().into();
            sender.action_id = record.actor.request_id().into();
            let mode = if record.submission == InvocationSubmission::Queued {
                SubmissionMode::Queue
            } else {
                SubmissionMode::Steer
            };
            let expected = super::mutation::submit_binding(
                id,
                &sender,
                record.request.execution_id.as_str(),
                &message,
                mode,
            )?;
            if let Some(index) = receipts.iter().position(|receipt| {
                receipt.binding().operation() == MutationOperation::Submit
                    && receipt.binding() == &expected
                    && receipt.outcome() == Some(MutationOutcome::Dispatched)
            }) {
                if let Some(store) = self.inner.scheduled_messages.get() {
                    if let Some(evidence) = store
                        .command_evidence(&id.to_string(), record.request.execution_id.as_str())
                        .await?
                    {
                        if evidence.binding(id, record.request.execution_id.as_str())? == expected {
                            continue;
                        }
                    }
                }
                people.push((index, record.request.execution_id.as_str().to_owned()));
            }
        }
        people.sort();
        Ok(people)
    }
    pub(super) async fn reports_paused(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
        history: &[InvocationRecord],
    ) -> Result<bool, ConversationError> {
        if self.inner.ownership.get().is_none() {
            return Ok(false);
        }
        let roots: HashSet<_> = self
            .captured_stops(caller)
            .await?
            .into_iter()
            .filter_map(|(root, target)| {
                (target.target() == &conversation_session(id))
                    .then(|| root.actor().request_id().to_owned())
            })
            .collect();
        if roots.is_empty() {
            return Ok(false);
        }
        let Some(storage) = self.commands() else {
            return Ok(true);
        };
        let receipts = storage
            .mutations(caller.principal_id.as_str().into())
            .await
            .map_err(|_| ConversationError::Unavailable)?;
        let stopped_at = receipts
            .iter()
            .enumerate()
            .filter_map(|(index, receipt)| {
                roots
                    .contains(receipt.binding().actor().request_id())
                    .then_some(index)
            })
            .max();
        let Some(stopped_at) = stopped_at else {
            return Ok(true);
        };
        Ok(!self
            .person_turns(id, caller, history)
            .await?
            .iter()
            .any(|(index, _)| *index > stopped_at))
    }
    pub(super) async fn next_report_delivery(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
        original: &str,
        history: &[InvocationRecord],
    ) -> Result<Option<String>, ConversationError> {
        Ok(self
            .person_turns(id, caller, history)
            .await?
            .last()
            .map(|(_, person)| delivery_id(original, person)))
    }
}

#[cfg(all(test, unix))]
#[path = "../../../../tests/conversation/thread_report_admission.rs"]
mod tests;
