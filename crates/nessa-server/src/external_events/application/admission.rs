use super::super::domain::EventId;
use crate::conversation::application::{
    ConversationCaller, ConversationError, ConversationService, SubmissionMode, SubmittedMessage,
};
use nessa_auth::{
    application::{
        authorization::AuthorizeAction,
        ports::{AccessError, AccessReader, Clock, Decision, PolicyEvaluator},
        session::AuthenticatedSession,
    },
    domain::{Action, Resource},
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::{
    application::agent_execution::commands::{CreationStorage, MutationFailure, MutationOutcome},
    domain::agent_execution::executions::Initiation,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Server-selected binding. Authenticate `session` through Nessa Auth before
/// construction. Each event rechecks current authority; no cached allow is used.
/// Keep the identity and target stable for this binding's command-store lifetime.
pub struct EventBinding {
    /// Stable trigger identity; principal/target rebinding requires a new identity.
    pub id: EventId,
    /// Existing conversation selected by trusted composition, never the event.
    pub conversation: ConversationId,
    /// Authenticated binding identity whose current authority each event rechecks.
    pub session: AuthenticatedSession,
    /// Server-resolved gateway resource for the existing conversation.write grant.
    pub resource: Resource,
    /// Coherent current credential and membership reader.
    pub access: Arc<dyn AccessReader>,
    /// Existing Auth policy port, composed with Cedar in a real gateway.
    pub policy: Arc<dyn PolicyEvaluator>,
    /// Absolute clock for credential expiry and HTTP signature freshness.
    pub clock: Arc<dyn Clock>,
    /// Existing owner of target admission, execution and permissions.
    pub service: ConversationService,
    /// Durable request store preserving exclusive command-lease ownership.
    pub commands: Arc<dyn CreationStorage>,
}
/// A durable dispatch receipt; this says nothing about execution completion.
pub struct Admission {
    /// Stable turn identity for observation or the existing exact-turn stop API.
    pub execution_id: String,
}
/// Current authority or durable dispatch failed; errors do not prove no effect.
#[derive(Debug)]
pub enum AdmissionFailure {
    /// Current identity or Cedar refusal before command handoff.
    Access(AccessError),
    /// Existing command failure, preserving conflict and interrupted evidence.
    Submit(MutationFailure<ConversationError>),
    /// A returned command receipt did not acknowledge dispatch.
    NotDispatched,
}
impl EventBinding {
    /// Admit a verified event as one queued conversation turn. `text` is the
    /// adapter-mapped event message; the conversation service validates its bound.
    /// Stable event identities select existing command receipts, so retries do
    /// not request a fresh execution. The receipt acknowledges dispatch, not the
    /// turn result. Reconcile an interrupted attempt before choosing a new ID.
    pub async fn admit(
        &self,
        event: &EventId,
        text: String,
    ) -> Result<Admission, AdmissionFailure> {
        let decision = AuthorizeAction {
            access: self.access.as_ref(),
            clock: self.clock.as_ref(),
            policy: self.policy.as_ref(),
        }
        .execute(
            &self.session,
            &Action::new("conversation.write").expect("static action"),
            &self.resource,
        )
        .await
        .map_err(AdmissionFailure::Access)?;
        if decision != Decision::Allow {
            return Err(AdmissionFailure::Access(AccessError::Denied));
        }
        let context = self.session.context();
        let digest = Sha256::digest(format!("{}\n{}", self.id.as_str(), event.as_str()).as_bytes());
        let request = format!(
            "event-{}",
            digest
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        let execution_id = uuid::Uuid::from_bytes(bytes).to_string();
        let caller = ConversationCaller {
            organization_id: context.organization_id().clone(),
            principal_id: context.principal_id().clone(),
            surface_id: format!("event:{}", self.id.as_str()),
            action_id: request,
        };
        let submitted = self
            .service
            .submit_command(
                self.commands.clone(),
                self.conversation.clone(),
                caller,
                Initiation::owner(),
                false,
                execution_id.clone(),
                SubmittedMessage {
                    text,
                    ..SubmittedMessage::default()
                },
                SubmissionMode::Queue,
            )
            .await
            .map_err(AdmissionFailure::Submit)?;
        if submitted.progress.outcome() != Some(MutationOutcome::Dispatched) {
            return Err(AdmissionFailure::NotDispatched);
        }
        Ok(Admission { execution_id })
    }
}
