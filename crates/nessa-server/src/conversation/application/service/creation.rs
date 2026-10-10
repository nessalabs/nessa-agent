//! Creation command receipts consume the SDK ordering owner and this service's
//! existing target access, metadata, audit, provider and cleanup path.
use super::{
    conversation_session, AgentError, AttachmentPhase, ConversationCaller, ConversationError,
    ConversationId, ConversationService, RequestedAgent, RequestedConversation,
};
use nessa_sdk::application::agent_execution::commands::{
    CreationBinding, CreationCoordinator, CreationFailure, CreationFuture,
    CreationInitializationFailure, CreationReceipt, CreationStage, CreationStorage,
    CreationStorageError, CreationTarget, CreationTaskFault,
};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

impl ConversationService {
    /// Create through the durable principal command owner. The caller has already
    /// been authenticated by the host. The same request returns its original
    /// saved readiness or a typed interrupted attempt, without opening it again.
    ///
    /// Configuration stays in the target's existing metadata/deletion owner;
    /// only its fixed canonical fingerprint enters command storage. This entry
    /// point is supplied explicitly with the shared record storage. It
    /// waits for the original provider attachment and required publication before
    /// saving readiness; preparation of the live conversation is not completion.
    /// It preserves the original admitted owner through that wait. The product
    /// socket calls this entry point for create.
    pub async fn create_command(
        &self,
        storage: Arc<dyn CreationStorage>,
        id: ConversationId,
        caller: ConversationCaller,
        requested: RequestedConversation,
    ) -> Result<CreationReceipt, CreationFailure<ConversationError>> {
        let binding = binding(&id, &caller, &requested).map_err(CreationFailure::Target)?;
        if let Some(reopened) = self
            .reopen_owned(storage.clone(), &id, &caller, &requested, &binding)
            .await?
        {
            return Ok(reopened);
        }
        let service = self.clone();
        tokio::spawn(async move {
            let _admission = service
                .admit()
                .await
                .map_err(ConversationError::from)
                .map_err(CreationFailure::Target)?;
            let target = Arc::new(Target {
                service: service.clone(),
                id,
                caller,
                requested: Mutex::new(Some(requested)),
            });
            CreationCoordinator::new(storage)
                .create(binding, target)
                .await
        })
        .await
        .map_err(|error| CreationFailure::TaskFault(CreationTaskFault::from_join_error(error)))?
    }

    /// Read saved creation progress after checking this caller's current target
    /// access and deletion. No provider or command is admitted; missing principal
    /// control history is not created. `Attempted` means outcome is unconfirmed.
    pub async fn lookup_creation(
        &self,
        storage: Arc<dyn CreationStorage>,
        id: ConversationId,
        caller: ConversationCaller,
        requested: RequestedConversation,
    ) -> Result<Option<CreationReceipt>, CreationFailure<ConversationError>> {
        let binding = binding(&id, &caller, &requested).map_err(CreationFailure::Target)?;
        let target = Target {
            service: self.clone(),
            id,
            caller,
            requested: Mutex::new(None),
        };
        CreationCoordinator::new(storage)
            .lookup(&binding, &target)
            .await
    }

    /// A new request for a conversation that already exists is the existing
    /// creation owner's reopen. It attaches when the process has no live
    /// slot, and it does not write a second creation receipt: a stored
    /// `Ready` would skip that attach on the next restart. An existing
    /// receipt stays with the coordinator, so an exact retry still does not
    /// open the provider again.
    async fn reopen_owned(
        &self,
        storage: Arc<dyn CreationStorage>,
        id: &ConversationId,
        caller: &ConversationCaller,
        requested: &RequestedConversation,
        binding: &CreationBinding,
    ) -> Result<Option<CreationReceipt>, CreationFailure<ConversationError>> {
        let saved = match storage
            .open_creation(binding.actor().principal_id().into(), false)
            .await
        {
            Ok(Some(lease)) => match lease.load(binding.actor().request_id()).await {
                Ok(saved) => saved,
                Err(CreationStorageError::ForeignRequest) => return Err(CreationFailure::Conflict),
                Err(error) => return Err(CreationFailure::Storage(error)),
            },
            Ok(None) => None,
            Err(error) => return Err(CreationFailure::Storage(error)),
        };
        if saved.is_some() {
            return Ok(None);
        }
        let Some(record) = self
            .inner
            .metadata
            .load(id)
            .await
            .map_err(CreationFailure::Target)?
        else {
            return Ok(None);
        };
        record
            .check_access(&caller.organization_id, &caller.principal_id)
            .map_err(ConversationError::from)
            .map_err(CreationFailure::Target)?;
        self.create(
            id.clone(),
            caller.clone(),
            RequestedConversation {
                agent: requested.agent,
                model: requested.model.clone(),
                approval_mode: requested.approval_mode,
                environment: None,
            },
        )
        .await
        .map_err(CreationFailure::Target)?;
        let accepted = CreationReceipt::accepted(binding.clone());
        let attempted = accepted
            .advance(CreationStage::Attempted)
            .map_err(|error| CreationFailure::Storage(CreationStorageError::Storage(error)))?;
        Ok(Some(attempted.advance(CreationStage::Ready).map_err(
            |error| CreationFailure::Storage(CreationStorageError::Storage(error)),
        )?))
    }
}
struct Target {
    service: ConversationService,
    id: ConversationId,
    caller: ConversationCaller,
    requested: Mutex<Option<RequestedConversation>>,
}
impl CreationTarget for Target {
    type Error = ConversationError;
    fn check(
        &self,
        _: &CreationBinding,
        stage: Option<CreationStage>,
    ) -> CreationFuture<'_, (), Self::Error> {
        Box::pin(async move {
            if let Some(record) = self.service.inner.metadata.load(&self.id).await? {
                record.check_access(&self.caller.organization_id, &self.caller.principal_id)?;
                // A missing receipt is not a conflict. The creator comparison
                // applies to a saved stage for this request. Lookup of an
                // unknown request on a live conversation is absent evidence.
                if stage.is_some()
                    && (record.creation_action() != self.caller.action_id
                        || record.creator_surface() != self.caller.surface_id)
                {
                    return Err(ConversationError::RequestConflict);
                }
            } else if stage == Some(CreationStage::Ready) {
                return Err(ConversationError::NotFound);
            }
            Ok(())
        })
    }
    fn initialize(
        &self,
        _: &CreationBinding,
    ) -> CreationFuture<'_, (), CreationInitializationFailure<Self::Error>> {
        let requested = self
            .requested
            .lock()
            .map_err(|_| ConversationError::Unavailable)
            .and_then(|mut requested| requested.take().ok_or(ConversationError::RequestConflict));
        Box::pin(async move {
            let live = self
                .service
                .create_admitted(
                    self.id.clone(),
                    self.caller.clone(),
                    requested.map_err(CreationInitializationFailure::Target)?,
                )
                .await
                .map_err(CreationInitializationFailure::Target)?;
            live.join_attachment_owner()
                .await
                .map_err(CreationInitializationFailure::TaskFault)?;
            let status = live.agent.attachment_status();
            if let Some(failure) = status.failure().or(status.evidence_failure()) {
                return Err(CreationInitializationFailure::Target(
                    ConversationError::Agent(failure.error().clone()),
                ));
            }
            if status.phase() != AttachmentPhase::Attached {
                return Err(CreationInitializationFailure::Target(
                    ConversationError::Agent(AgentError::AttachmentUnavailable(status.phase())),
                ));
            }
            Ok(())
        })
    }
}
fn binding(
    id: &ConversationId,
    caller: &ConversationCaller,
    requested: &RequestedConversation,
) -> Result<CreationBinding, ConversationError> {
    let agent = match requested.agent {
        None => None,
        Some(RequestedAgent::Known(agent)) => Some(agent.name()),
        Some(RequestedAgent::Unknown) => return Err(ConversationError::InvalidInput),
    };
    // serde's tuple encoding supplies lengths, null and escaping; concatenating
    // strings would let distinct field boundaries yield the same fingerprint.
    // Where it runs is part of what was asked, as its model is: the same
    // request naming another environment is another request. Named only
    // when one is, so a request that names none keeps the fingerprint it
    // always had; the array's length tells the two forms apart.
    let canonical = match requested.environment.as_deref() {
        None => serde_json::to_vec(&(
            caller.organization_id.as_str(),
            agent,
            requested.model.as_deref(),
            requested.approval_mode.map(|mode| mode.as_str()),
        )),
        Some(environment) => serde_json::to_vec(&(
            caller.organization_id.as_str(),
            agent,
            requested.model.as_deref(),
            requested.approval_mode.map(|mode| mode.as_str()),
            environment,
        )),
    }
    .map_err(|_| ConversationError::InvalidInput)?;
    let mut digest = Sha256::new();
    digest.update(b"nessa.conversation.create\0");
    digest.update(canonical);
    Ok(CreationBinding::new(
        caller.actor()?,
        conversation_session(id),
        digest.finalize().into(),
    ))
}

#[cfg(test)]
#[path = "../../../../tests/conversation/creation_commands.rs"]
mod tests;
