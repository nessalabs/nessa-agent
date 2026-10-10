//! Submit and exact-turn Stop consume the SDK mutation coordinator.
use super::{
    conversation_session, ConversationCaller, ConversationDisposition, ConversationError,
    ConversationId, ConversationService, SubmissionMode, SubmissionReceipt, SubmittedMessage,
};
use nessa_sdk::application::agent_execution::agents::{AgentError, QueueRemoval};
use nessa_sdk::application::agent_execution::commands::{
    CreationFuture, CreationInitializationFailure, CreationStorage, CreationTaskFault,
    MutationBinding, MutationCoordinator, MutationFailure, MutationOperation, MutationOutcome,
    MutationPlan, MutationReceipt, MutationStage, MutationTarget,
};
use nessa_sdk::application::agent_execution::providers::ProviderSessionState;
use nessa_sdk::domain::agent_execution::executions::{ExecutionId, InvocationStage};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

impl ConversationService {
    /// Bind the shared principal command store. A second bind is ignored.
    /// The socket uses this store for creation, submit, and stop; it does not
    /// admit those commands when the store is absent.
    pub fn bind_commands(&self, storage: Arc<dyn CreationStorage>) {
        let _ = self.inner.commands.set(storage);
    }

    /// The bound command store, when composition or a test has supplied one.
    pub fn commands(&self) -> Option<Arc<dyn CreationStorage>> {
        self.inner.commands.get().cloned()
    }

    /// Queue one turn after its receipt is durable. An exact settled retry does
    /// not enqueue again. A restored attempt returns interrupted.
    ///
    /// `delivery` is the submission receipt from the call that actually
    /// enqueued. A settled retry leaves it empty; the socket then reads the
    /// saved turn without enqueueing again.
    pub async fn submit_command(
        &self,
        storage: Arc<dyn CreationStorage>,
        id: ConversationId,
        caller: ConversationCaller,
        execution_id: String,
        message: SubmittedMessage,
        mode: SubmissionMode,
    ) -> Result<SubmittedCommand, MutationFailure<ConversationError>> {
        let binding = submit_binding(&id, &caller, &execution_id, &message, mode)
            .map_err(MutationFailure::Target)?;
        // Shape is refused before any stage. A deleted target is named before a
        // conflicting receipt, so the surface lets the conversation go.
        Self::canonical_user_message(
            self.inner.limits.max_input_bytes,
            &execution_id,
            message.clone(),
        )
        .map_err(MutationFailure::Target)?;
        refuse_deleted(self, &id, &caller)
            .await
            .map_err(MutationFailure::Target)?;
        self.refuse_linked_files_elsewhere(&id, !message.files.is_empty())
            .await
            .map_err(MutationFailure::Target)?;
        let service = self.clone();
        let delivery = Arc::new(Mutex::new(None));
        let captured = delivery.clone();
        let progress = tokio::spawn(async move {
            let _admission = service
                .admit()
                .await
                .map_err(ConversationError::from)
                .map_err(MutationFailure::Target)?;
            let target = Arc::new(SubmitTarget {
                service: service.clone(),
                id,
                caller,
                execution_id,
                message: Mutex::new(Some(message)),
                mode,
                delivery: captured,
            });
            MutationCoordinator::new(storage)
                .execute(binding, target)
                .await
        })
        .await
        .map_err(|error| MutationFailure::TaskFault(CreationTaskFault::from_join_error(error)))??;
        let delivery = delivery
            .lock()
            .map_err(|_| MutationFailure::Target(ConversationError::Unavailable))?
            .take();
        Ok(SubmittedCommand { progress, delivery })
    }

    /// Describe a turn that a settled submit already admitted. This does not enqueue.
    pub async fn current_submission(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        execution_id: String,
    ) -> Result<SubmissionReceipt, ConversationError> {
        let execution =
            ExecutionId::new(&execution_id).map_err(|_| ConversationError::InvalidInput)?;
        let live = self.resolve(&id, &caller).await?;
        let snapshot = live
            .agent
            .session_manager()
            .snapshot()
            .await
            .ok_or(ConversationError::NotFound)?;
        let record = snapshot
            .invocations
            .iter()
            .find(|record| record.request.execution_id == execution)
            .ok_or(ConversationError::NotFound)?;
        let stage = record.scheduling.last().map(|event| event.stage);
        let disposition = if record.result.is_some()
            || matches!(
                stage,
                Some(InvocationStage::Settled | InvocationStage::Cancelled)
            ) {
            ConversationDisposition::Settled
        } else if matches!(stage, Some(InvocationStage::Injected)) {
            ConversationDisposition::Injected
        } else {
            ConversationDisposition::Queued
        };
        Ok(SubmissionReceipt {
            execution_id,
            disposition,
        })
    }

    /// Stop the named turn. A queued turn is withdrawn. The active turn is
    /// cancelled without closing the attachment. A finished turn records
    /// `AlreadyFinal` and sends nothing.
    pub async fn stop_command(
        &self,
        storage: Arc<dyn CreationStorage>,
        id: ConversationId,
        caller: ConversationCaller,
        execution_id: String,
    ) -> Result<MutationReceipt, MutationFailure<ConversationError>> {
        let binding = stop_binding(&id, &caller, &execution_id).map_err(MutationFailure::Target)?;
        ExecutionId::new(&execution_id)
            .map_err(|_| MutationFailure::Target(ConversationError::InvalidInput))?;
        refuse_deleted(self, &id, &caller)
            .await
            .map_err(MutationFailure::Target)?;
        let service = self.clone();
        tokio::spawn(async move {
            let _admission = service
                .admit()
                .await
                .map_err(ConversationError::from)
                .map_err(MutationFailure::Target)?;
            let target = Arc::new(StopTarget {
                service: service.clone(),
                id,
                caller,
                execution_id,
            });
            MutationCoordinator::new(storage)
                .execute(binding, target)
                .await
        })
        .await
        .map_err(|error| MutationFailure::TaskFault(CreationTaskFault::from_join_error(error)))?
    }

    /// Read a submit or stop receipt. This does not enqueue, withdraw, or cancel.
    pub async fn lookup_command(
        &self,
        storage: Arc<dyn CreationStorage>,
        id: ConversationId,
        caller: ConversationCaller,
        binding: MutationBinding,
    ) -> Result<Option<MutationReceipt>, MutationFailure<ConversationError>> {
        let target = LookupTarget {
            service: self.clone(),
            id,
            caller,
        };
        MutationCoordinator::new(storage)
            .lookup(&binding, &target)
            .await
    }

    /// Rebuild the submit binding a read-only lookup presents.
    pub(crate) fn submit_lookup(
        id: &ConversationId,
        caller: &ConversationCaller,
        execution_id: &str,
        message: &SubmittedMessage,
        mode: SubmissionMode,
    ) -> Result<MutationBinding, ConversationError> {
        submit_lookup_binding(id, caller, execution_id, message, mode)
    }

    /// Rebuild the stop binding a read-only lookup presents.
    pub(crate) fn stop_lookup(
        id: &ConversationId,
        caller: &ConversationCaller,
        execution_id: &str,
    ) -> Result<MutationBinding, ConversationError> {
        stop_lookup_binding(id, caller, execution_id)
    }
}

/// Progress of one submit, plus the delivery receipt when this call enqueued it.
pub struct SubmittedCommand {
    /// Durable command progress.
    pub progress: MutationReceipt,
    /// Present only for the call that performed the original enqueue.
    pub delivery: Option<SubmissionReceipt>,
}

struct SubmitTarget {
    service: ConversationService,
    id: ConversationId,
    caller: ConversationCaller,
    execution_id: String,
    message: Mutex<Option<SubmittedMessage>>,
    mode: SubmissionMode,
    delivery: Arc<Mutex<Option<SubmissionReceipt>>>,
}
impl MutationTarget for SubmitTarget {
    type Error = ConversationError;
    fn check(
        &self,
        _: &MutationBinding,
        _: Option<MutationStage>,
    ) -> CreationFuture<'_, (), Self::Error> {
        let service = self.service.clone();
        let id = self.id.clone();
        let caller = self.caller.clone();
        Box::pin(async move { ensure_live(&service, &id, &caller).await })
    }
    fn classify(&self, _: &MutationBinding) -> CreationFuture<'_, MutationPlan, Self::Error> {
        Box::pin(async { Ok(MutationPlan::Effect) })
    }
    fn effect(
        &self,
        _: &MutationBinding,
    ) -> CreationFuture<'_, MutationOutcome, CreationInitializationFailure<Self::Error>> {
        let message = self
            .message
            .lock()
            .map_err(|_| ConversationError::Unavailable)
            .and_then(|mut message| message.take().ok_or(ConversationError::RequestConflict));
        let service = self.service.clone();
        let id = self.id.clone();
        let caller = self.caller.clone();
        let execution_id = self.execution_id.clone();
        let mode = self.mode;
        let delivery = self.delivery.clone();
        Box::pin(async move {
            let receipt = service
                .submit(
                    id,
                    caller,
                    execution_id,
                    message.map_err(CreationInitializationFailure::Target)?,
                    mode,
                )
                .await
                .map_err(CreationInitializationFailure::Target)?;
            delivery
                .lock()
                .map_err(|_| ConversationError::Unavailable)
                .map_err(CreationInitializationFailure::Target)?
                .replace(receipt);
            Ok(MutationOutcome::Dispatched)
        })
    }
}

struct StopTarget {
    service: ConversationService,
    id: ConversationId,
    caller: ConversationCaller,
    execution_id: String,
}
impl MutationTarget for StopTarget {
    type Error = ConversationError;
    fn check(
        &self,
        _: &MutationBinding,
        _: Option<MutationStage>,
    ) -> CreationFuture<'_, (), Self::Error> {
        let service = self.service.clone();
        let id = self.id.clone();
        let caller = self.caller.clone();
        Box::pin(async move { ensure_live(&service, &id, &caller).await })
    }
    fn classify(&self, binding: &MutationBinding) -> CreationFuture<'_, MutationPlan, Self::Error> {
        let service = self.service.clone();
        let id = self.id.clone();
        let caller = self.caller.clone();
        let turn = binding.turn_id().clone();
        Box::pin(async move {
            if self.execution_id != turn.as_str() {
                return Err(ConversationError::RequestConflict);
            }
            let live = service.resolve(&id, &caller).await?;
            if live
                .agent
                .queued_ids()
                .await
                .iter()
                .any(|queued| queued == &turn)
            {
                return Ok(MutationPlan::Effect);
            }
            if live.agent.active_execution_id().as_ref() == Some(&turn) {
                if live.agent.supports_turn_cancel() {
                    return Ok(MutationPlan::Effect);
                }
                return Err(ConversationError::Agent(AgentError::Unsupported(
                    "turn cancel".into(),
                )));
            }
            match turn_is_final(&live, &turn).await {
                Some(true) => Ok(MutationPlan::AlreadyFinal),
                Some(false) => Err(ConversationError::Agent(AgentError::SubmissionUnresolved)),
                None => Err(ConversationError::NotFound),
            }
        })
    }
    fn effect(
        &self,
        binding: &MutationBinding,
    ) -> CreationFuture<'_, MutationOutcome, CreationInitializationFailure<Self::Error>> {
        let service = self.service.clone();
        let id = self.id.clone();
        let caller = self.caller.clone();
        let turn = binding.turn_id().clone();
        Box::pin(async move {
            let live = service
                .resolve(&id, &caller)
                .await
                .map_err(CreationInitializationFailure::Target)?;
            let actor = caller
                .actor()
                .map_err(CreationInitializationFailure::Target)?;
            if live
                .agent
                .queued_ids()
                .await
                .iter()
                .any(|queued| queued == &turn)
            {
                match live
                    .agent
                    .remove_queued(turn.clone(), actor)
                    .await
                    .map_err(|error| {
                        CreationInitializationFailure::Target(ConversationError::Agent(error))
                    })? {
                    QueueRemoval::Removed => return Ok(MutationOutcome::Withdrawn),
                    QueueRemoval::NotPending => {}
                }
            }
            if live.agent.active_execution_id().as_ref() == Some(&turn) {
                return match live.agent.cancel_turn(turn.clone()).await {
                    Ok(()) => Ok(MutationOutcome::Cancelled),
                    Err(failure) => {
                        let (error, state) = failure.into_parts();
                        if matches!(
                            (&error, state),
                            (AgentError::Unsupported(_), ProviderSessionState::Usable)
                        ) && live.agent.active_execution_id().as_ref() != Some(&turn)
                        {
                            Ok(MutationOutcome::AlreadyFinal)
                        } else {
                            Err(CreationInitializationFailure::Target(
                                ConversationError::Agent(error),
                            ))
                        }
                    }
                };
            }
            match turn_is_final(&live, &turn).await {
                Some(true) => Ok(MutationOutcome::AlreadyFinal),
                _ => Err(CreationInitializationFailure::Target(
                    ConversationError::NotFound,
                )),
            }
        })
    }
}

struct LookupTarget {
    service: ConversationService,
    id: ConversationId,
    caller: ConversationCaller,
}
impl MutationTarget for LookupTarget {
    type Error = ConversationError;
    fn check(
        &self,
        _: &MutationBinding,
        _: Option<MutationStage>,
    ) -> CreationFuture<'_, (), Self::Error> {
        let service = self.service.clone();
        let id = self.id.clone();
        let caller = self.caller.clone();
        Box::pin(async move { ensure_live(&service, &id, &caller).await })
    }
    fn classify(&self, _: &MutationBinding) -> CreationFuture<'_, MutationPlan, Self::Error> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn effect(
        &self,
        _: &MutationBinding,
    ) -> CreationFuture<'_, MutationOutcome, CreationInitializationFailure<Self::Error>> {
        Box::pin(async {
            Err(CreationInitializationFailure::Target(
                ConversationError::Unavailable,
            ))
        })
    }
}

async fn refuse_deleted(
    service: &ConversationService,
    id: &ConversationId,
    caller: &ConversationCaller,
) -> Result<(), ConversationError> {
    let Some(record) = service.inner.metadata.load(id).await? else {
        return Ok(());
    };
    match record.check_access(&caller.organization_id, &caller.principal_id) {
        Err(crate::conversation::domain::ConversationRefusal::Deleted) => {
            Err(ConversationError::Deleted)
        }
        _ => Ok(()),
    }
}

async fn ensure_live(
    service: &ConversationService,
    id: &ConversationId,
    caller: &ConversationCaller,
) -> Result<(), ConversationError> {
    let record = service
        .inner
        .metadata
        .load(id)
        .await?
        .ok_or(ConversationError::NotFound)?;
    record.check_access(&caller.organization_id, &caller.principal_id)?;
    Ok(())
}

async fn turn_is_final(live: &super::LiveConversation, turn: &ExecutionId) -> Option<bool> {
    let snapshot = live.agent.session_manager().snapshot().await?;
    let record = snapshot
        .invocations
        .iter()
        .find(|record| &record.request.execution_id == turn)?;
    let stage = record.scheduling.last().map(|event| event.stage);
    Some(
        record.result.is_some()
            || matches!(
                stage,
                Some(InvocationStage::Settled | InvocationStage::Cancelled)
            ),
    )
}

fn submit_binding(
    id: &ConversationId,
    caller: &ConversationCaller,
    execution_id: &str,
    message: &SubmittedMessage,
    mode: SubmissionMode,
) -> Result<MutationBinding, ConversationError> {
    let canonical = serde_json::to_vec(&(
        execution_id,
        match mode {
            SubmissionMode::Queue => "queue",
            SubmissionMode::Steer => "steer",
        },
        message.text.as_str(),
        message
            .images
            .iter()
            .map(|image| (image.digest.as_str(), image.media_type.as_str(), image.size))
            .collect::<Vec<_>>(),
        message
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
    ))
    .map_err(|_| ConversationError::InvalidInput)?;
    let mut digest = Sha256::new();
    digest.update(b"nessa.conversation.submit\0");
    digest.update(canonical);
    MutationBinding::new(
        caller.actor()?,
        MutationOperation::Submit,
        conversation_session(id),
        execution_id,
        digest.finalize().into(),
    )
    .map_err(|_| ConversationError::InvalidInput)
}

fn stop_binding(
    id: &ConversationId,
    caller: &ConversationCaller,
    execution_id: &str,
) -> Result<MutationBinding, ConversationError> {
    let canonical = serde_json::to_vec(&(id.to_string(), execution_id))
        .map_err(|_| ConversationError::InvalidInput)?;
    let mut digest = Sha256::new();
    digest.update(b"nessa.conversation.stop\0");
    digest.update(canonical);
    MutationBinding::new(
        caller.actor()?,
        MutationOperation::Stop,
        conversation_session(id),
        execution_id,
        digest.finalize().into(),
    )
    .map_err(|_| ConversationError::InvalidInput)
}

/// Rebuild the stop binding a read-only lookup presents. The host owns this
/// encoding; lookup does not admit a stop.
pub(crate) fn stop_lookup_binding(
    id: &ConversationId,
    caller: &ConversationCaller,
    execution_id: &str,
) -> Result<MutationBinding, ConversationError> {
    stop_binding(id, caller, execution_id)
}

pub(crate) fn submit_lookup_binding(
    id: &ConversationId,
    caller: &ConversationCaller,
    execution_id: &str,
    message: &SubmittedMessage,
    mode: SubmissionMode,
) -> Result<MutationBinding, ConversationError> {
    submit_binding(id, caller, execution_id, message, mode)
}

#[cfg(test)]
#[path = "../../../../tests/conversation/mutation_commands.rs"]
mod tests;
