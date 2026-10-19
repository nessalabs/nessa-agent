//! Submit and exact-turn Stop consume the SDK mutation coordinator.
use super::{
    conversation_session, load_conversation, ConversationCaller, ConversationDisposition,
    ConversationError, ConversationId, ConversationService, SubmissionMode, SubmissionReceipt,
    SubmitFailure, SubmittedMessage, Writer,
};
use crate::conversation::domain::ConversationRefusal;
use nessa_sdk::application::agent_execution::agents::{AgentError, AttachmentPhase, QueueRemoval};
use nessa_sdk::application::agent_execution::commands::{
    CreationFuture, CreationInitializationFailure, CreationStorage, CreationTaskFault,
    MutationBinding, MutationCoordinator, MutationFailure, MutationOperation, MutationOutcome,
    MutationPlan, MutationReceipt, MutationStage, MutationTarget,
};
use nessa_sdk::application::agent_execution::providers::ProviderSessionState;
use nessa_sdk::domain::agent_execution::executions::{ExecutionId, Initiation, InvocationStage};
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
    /// saved turn without enqueueing again. A submit settled
    /// [`MutationOutcome::NoEffect`] is returned as [`ConversationError::Revoked`]:
    /// the effect refused before anything was enqueued, and the same request
    /// answers that refusal again. A submit settled
    /// [`MutationOutcome::RefusedAfterSave`] is returned as
    /// [`ConversationError::RevokedAfterSave`]: the turn was saved and then
    /// refused before injection, and the same request answers that refusal
    /// again. A submit settled [`MutationOutcome::UnavailableAfterSave`] is
    /// returned as [`ConversationError::UnavailableAfterSave`]: the turn was
    /// saved and then could not be checked, and the same request answers
    /// that failure again.
    ///
    /// `initiation` is who the turn is saved as started by: the owner, or the
    /// share grant that admitted it. A retry of the same request is answered
    /// from its saved receipt, so the turn keeps the initiation it was first
    /// admitted with. The same turn sent again as a new request under another
    /// initiation (the grant's revision moved) is a conflict, as any other
    /// message under that turn's id is.
    ///
    /// `offers_remember` is whether this membership may use a remember choice.
    /// An owner turn is offered one only when it is true. A grant turn is not.
    #[allow(clippy::too_many_arguments)]
    pub async fn submit_command(
        &self,
        storage: Arc<dyn CreationStorage>,
        id: ConversationId,
        caller: ConversationCaller,
        initiation: Initiation,
        offers_remember: bool,
        execution_id: String,
        message: SubmittedMessage,
        mode: SubmissionMode,
    ) -> Result<SubmittedCommand, MutationFailure<ConversationError>> {
        let service = self.clone();
        tokio::spawn(async move {
            let _threads = service.inner.thread_admission.clone().lock_owned().await;
            service
                .submit_command_origin(
                    storage,
                    id,
                    caller,
                    initiation,
                    offers_remember,
                    execution_id,
                    message,
                    mode,
                    false,
                )
                .await
        })
        .await
        .map_err(|error| MutationFailure::TaskFault(CreationTaskFault::from_join_error(error)))?
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn submit_command_origin(
        &self,
        storage: Arc<dyn CreationStorage>,
        id: ConversationId,
        caller: ConversationCaller,
        initiation: Initiation,
        offers_remember: bool,
        execution_id: String,
        message: SubmittedMessage,
        mode: SubmissionMode,
        from_agent: bool,
    ) -> Result<SubmittedCommand, MutationFailure<ConversationError>> {
        let binding = submit_binding(&id, &caller, &execution_id, &message, mode)
            .map_err(MutationFailure::Target)?;
        let binding = if from_agent {
            let mut digest = Sha256::new();
            digest.update(b"nessa.gateway.agent-submit\0");
            digest.update(binding.fingerprint());
            MutationBinding::new(
                binding.actor().clone(),
                binding.operation(),
                binding.target().clone(),
                binding.turn_id().as_str(),
                digest.finalize().into(),
            )
            .map_err(|_| MutationFailure::Target(ConversationError::InvalidInput))?
        } else {
            binding
        };
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
                initiation,
                offers_remember,
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
        match progress.outcome() {
            Some(MutationOutcome::NoEffect) => {
                return Err(MutationFailure::Target(ConversationError::Revoked));
            }
            Some(MutationOutcome::RefusedAfterSave) => {
                return Err(MutationFailure::Target(ConversationError::RevokedAfterSave));
            }
            Some(MutationOutcome::UnavailableAfterSave) => {
                return Err(MutationFailure::Target(
                    ConversationError::UnavailableAfterSave,
                ));
            }
            _ => {}
        }
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

    /// Stop the named turn: withdraw queued work or decline its owed
    /// continuation, or cancel its active execution without closing the
    /// attachment. A final turn with no owed continuation records `AlreadyFinal`.
    pub async fn stop_command(
        &self,
        storage: Arc<dyn CreationStorage>,
        id: ConversationId,
        caller: ConversationCaller,
        execution_id: String,
    ) -> Result<MutationReceipt, MutationFailure<ConversationError>> {
        if self.inner.ownership.get().is_some() {
            return self
                .stop_owned_command(storage, id, caller, execution_id)
                .await;
        }
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
        let binding = if binding.operation() == MutationOperation::Stop
            && self.inner.ownership.get().is_some()
        {
            ensure_live(self, &id, &caller)
                .await
                .map_err(MutationFailure::Target)?;
            self.selected_stop_lookup(storage.clone(), &binding).await?
        } else {
            binding
        };
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
    initiation: Initiation,
    offers_remember: bool,
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
        let initiation = self.initiation.clone();
        let offers_remember = self.offers_remember;
        let execution_id = self.execution_id.clone();
        let mode = self.mode;
        let delivery = self.delivery.clone();
        Box::pin(async move {
            // `submit_command` holds admission through this effect.
            let submitted = service
                .submit_admitted_as(
                    id,
                    caller,
                    execution_id,
                    message.map_err(CreationInitializationFailure::Target)?,
                    mode,
                    Writer::Person(initiation, offers_remember),
                    true,
                )
                .await
                .map_err(SubmitFailure::into_error);
            let receipt = match submitted {
                // The revision hold refused after this command was recorded
                // Attempted, or the second hold could not be read. Settle it,
                // so the same request is answered again instead of
                // `submission_unresolved`. Before the save, nothing was
                // enqueued. After the save, the SDK already cached
                // `RefusedAfterSave` or `AfterSaveUnavailable`; the receipt
                // keeps that answer.
                Err(ConversationError::Revoked) => return Ok(MutationOutcome::NoEffect),
                Err(ConversationError::RevokedAfterSave) => {
                    return Ok(MutationOutcome::RefusedAfterSave)
                }
                Err(ConversationError::UnavailableAfterSave) => {
                    return Ok(MutationOutcome::UnavailableAfterSave)
                }
                Err(error) => return Err(CreationInitializationFailure::Target(error)),
                Ok(receipt) => receipt,
            };
            delivery
                .lock()
                .map_err(|_| ConversationError::Unavailable)
                .map_err(CreationInitializationFailure::Target)?
                .replace(receipt);
            Ok(MutationOutcome::Dispatched)
        })
    }
}

pub(super) struct StopTarget {
    pub(super) service: ConversationService,
    pub(super) id: ConversationId,
    pub(super) caller: ConversationCaller,
    pub(super) execution_id: String,
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
            // A turn leaving the queue is neither queued nor running for a
            // moment: asked again until it is one or ends, as in `effect`.
            let deadline = std::time::Instant::now() + MOVING_TURN_WAIT;
            loop {
                if live
                    .agent
                    .owed_turns()
                    .await
                    .iter()
                    .any(|owed| owed.execution_id() == &turn)
                {
                    return Ok(MutationPlan::Effect);
                }
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
                    // Only a provider that cannot cancel one turn is refused
                    // here. One whose attachment is ending (a close, or a
                    // failure stopping it) with its turn still listed is
                    // answered by how the turn ends, below
                    // (`a_stop_that_meets_a_close_is_answered_by_the_turns_end`).
                    if matches!(
                        live.agent.attachment_status().phase(),
                        AttachmentPhase::Attached
                    ) {
                        return Err(ConversationError::Agent(AgentError::Unsupported(
                            "turn cancel".into(),
                        )));
                    }
                }
                match turn_is_final(&live, &turn).await {
                    Some(true) => return Ok(MutationPlan::AlreadyFinal),
                    Some(false) if std::time::Instant::now() < deadline => {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                    Some(false) => {
                        return Err(ConversationError::Agent(AgentError::SubmissionUnresolved))
                    }
                    None => return Err(ConversationError::NotFound),
                }
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
            // Checked queued or running, and still one or the other unless it
            // ended: a turn leaving the queue is neither for a moment, between
            // its selection and its start. It is asked again until it is one
            // or ends, rather than answered as not found
            // (`a_stop_that_meets_a_turn_leaving_the_queue_stops_it`).
            let deadline = std::time::Instant::now() + MOVING_TURN_WAIT;
            // A cancel that met the agent's lifecycle ending (a close, or a
            // failure stopping it) while its turn was still listed as running.
            let mut ending: Option<AgentError> = None;
            // A cancel the provider refused with its session still usable
            // while the turn is listed running: the turn has not reached the
            // provider yet, or has just left it. Asked again until it reaches
            // the turn or the turn ends; at the deadline this refusal answers.
            let mut refused: Option<AgentError> = None;
            loop {
                if let Some(owed) = live
                    .agent
                    .owed_turns()
                    .await
                    .into_iter()
                    .find(|owed| owed.execution_id() == &turn)
                {
                    live.agent
                        .decline_turn(&owed, actor.clone())
                        .await
                        .map_err(|error| {
                            CreationInitializationFailure::Target(ConversationError::Agent(error))
                        })?;
                    service.wake_thread_reports(&id);
                    return Ok(MutationOutcome::Withdrawn);
                }
                if live
                    .agent
                    .queued_ids()
                    .await
                    .iter()
                    .any(|queued| queued == &turn)
                {
                    match live
                        .agent
                        .remove_queued(turn.clone(), actor.clone())
                        .await
                        .map_err(|error| {
                            CreationInitializationFailure::Target(ConversationError::Agent(error))
                        })? {
                        QueueRemoval::Removed => {
                            service.wake_thread_reports(&id);
                            return Ok(MutationOutcome::Withdrawn);
                        }
                        QueueRemoval::NotPending => {}
                    }
                }
                if ending.is_none() && live.agent.active_execution_id().as_ref() == Some(&turn) {
                    match live.agent.cancel_turn(turn.clone()).await {
                        Ok(()) => return Ok(MutationOutcome::Cancelled),
                        // The turn ended as the cancel reached it, its agent
                        // stopping with it after a failure, say: the stop is
                        // answered by how the turn ends, below.
                        Err(_) if live.agent.active_execution_id().as_ref() != Some(&turn) => {}
                        // The agent no longer takes control because it is
                        // ending, and its turn ends with it: answered by how
                        // the turn ends, below, not by the cancel's refusal
                        // (`a_stop_whose_cancel_meets_the_agent_ending_is_answered_by_the_turns_end`).
                        Err(failure)
                            if !matches!(failure.session_state(), ProviderSessionState::Usable) =>
                        {
                            ending = Some(failure.into_parts().0);
                        }
                        // (`a_stop_whose_cancel_meets_the_turn_not_yet_in_the_provider_stops_it`)
                        Err(failure) => refused = Some(failure.into_parts().0),
                    }
                }
                match turn_is_final(&live, &turn).await {
                    Some(true) => return Ok(MutationOutcome::AlreadyFinal),
                    Some(false) if std::time::Instant::now() < deadline => {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                    Some(false) => {
                        return Err(CreationInitializationFailure::Target(
                            ConversationError::Agent(
                                ending
                                    .or(refused)
                                    .unwrap_or(AgentError::SubmissionUnresolved),
                            ),
                        ))
                    }
                    None => {
                        return Err(CreationInitializationFailure::Target(
                            ConversationError::NotFound,
                        ))
                    }
                }
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
    let Some(record) = load_conversation(service.inner.metadata.as_ref(), id).await? else {
        return Ok(());
    };
    match service.access().admit(&record, caller) {
        Err(ConversationRefusal::Deleted) => Err(ConversationError::Deleted),
        // A policy that cannot decide refuses, rather than letting a
        // deleted conversation through as one it could not tell about.
        Err(ConversationRefusal::Unavailable) => Err(ConversationError::Metadata),
        Ok(()) | Err(ConversationRefusal::NotFound) => Ok(()),
    }
}

pub(super) async fn ensure_live(
    service: &ConversationService,
    id: &ConversationId,
    caller: &ConversationCaller,
) -> Result<(), ConversationError> {
    let record = load_conversation(service.inner.metadata.as_ref(), id)
        .await?
        .ok_or(ConversationError::NotFound)?;
    service.access().admit(&record, caller)?;
    Ok(())
}

/// How long a stop waits for a turn between leaving the queue and starting
/// to be one or the other.
const MOVING_TURN_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

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

pub(super) fn submit_binding(
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

pub(super) fn stop_binding(
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
