//! Resolve source authority before delegating to existing target operations.
use super::environment_tools::running_turn;
use super::{load_conversation, ConversationCaller, ConversationError, ConversationService};
use crate::conversation::application::{
    AccessAction, AccessAsk, AccessDecision, AccessDenial, AccessResource, AccessSubject,
    AgentToolAudit, AgentToolAuditRecord, AgentToolRequest, AgentToolSource, RequestShape,
};
use nessa_auth::domain::PrincipalId;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::executions::Initiator;
use std::sync::Arc;
use tokio::sync::watch;

impl ConversationService {
    pub fn bind_agent_tool_audit(&self, audit: Arc<dyn AgentToolAudit>) {
        let _ = self.inner.agent_tool_audit.set(audit);
    }

    /// The relay supplies the source identity; the saved running turn supplies
    /// its actor. Cedar decides whether it may represent the source owner.
    /// An admitted audit write is supervised even when its caller disappears.
    pub async fn admit_agent_tool(
        &self,
        source: ConversationId,
        call: String,
        tool: String,
        target: Option<ConversationId>,
        request: AgentToolRequest,
        stop: watch::Receiver<bool>,
    ) -> Result<AgentToolSource, ConversationError> {
        if *stop.borrow() {
            return Err(ConversationError::Unavailable);
        }
        // Acquire before spawning, and transfer ownership into the detached
        // task. Source cancellation may release an observation slot, but not a
        // physical source admission still waiting for metadata or audit I/O.
        let physical = self
            .inner
            .agent_tool_admissions
            .try_acquire(&source)
            .ok_or(ConversationError::Unavailable)?;
        let service = self.clone();
        tokio::spawn(async move {
            let _physical = physical;
            let _admission = service.admit().await?;
            // Preserve an early existence diagnostic; this record is not the
            // source authority used after waiting for runtime evidence.
            load_conversation(service.inner.metadata.as_ref(), &source)
                .await?
                .ok_or(ConversationError::NotFound)?;
            let live = service
                .live(&source)
                .await
                .ok_or(ConversationError::Unavailable)?;
            let snapshot = live
                .agent
                .session_manager()
                .snapshot()
                .await
                .ok_or(ConversationError::Unavailable)?;
            let (lease, execution, actor, initiation) =
                running_turn(&snapshot, &call).ok_or(ConversationError::Unavailable)?;
            let principal = PrincipalId::new(actor.principal_id())
                .map_err(|_| ConversationError::InvalidInput)?;
            let owner_initiated = matches!(initiation.initiator(), Initiator::Owner);
            // The snapshot fixes provenance; current metadata fixes resource
            // authority. In particular, a fence may have landed during live()
            // or snapshot(). Read it last, directly before synchronous Cedar.
            let record = load_conversation(service.inner.metadata.as_ref(), &source)
                .await?
                .ok_or(ConversationError::NotFound)?;
            let decision = service.inner.access_policy.evaluate(&AccessAsk {
                subject: AccessSubject::AgentTurn {
                    organization: record.organization(),
                    principal: &principal,
                    owner_initiated,
                },
                action: AccessAction::AgentTools,
                resource: AccessResource::of(&record),
                held: None,
                request: RequestShape::default(),
                target: None,
            });
            let (allowed, policies, reason) = match decision {
                AccessDecision::Allow { policies } => (true, policies, None),
                AccessDecision::Deny(AccessDenial::Forbidden { policies }) => {
                    (false, policies, Some("forbidden".into()))
                }
                AccessDecision::Deny(AccessDenial::NotPermitted) => {
                    (false, Vec::new(), Some("not_permitted".into()))
                }
                AccessDecision::Deny(AccessDenial::Unavailable) => {
                    (false, Vec::new(), Some("policy_unavailable".into()))
                }
            };
            let audit = service
                .inner
                .agent_tool_audit
                .get()
                .ok_or(ConversationError::Audit)?;
            audit
                .record(AgentToolAuditRecord {
                    source: source.clone(),
                    execution: execution.as_str().into(),
                    lease: lease.as_str().into(),
                    call,
                    request: request.logical,
                    command: request.command.clone(),
                    tool,
                    target,
                    principal: actor.principal_id().into(),
                    surface: actor.surface_id().into(),
                    owner_initiated,
                    allowed,
                    policies,
                    reason,
                })
                .await?;
            if !allowed {
                return Err(ConversationError::NotFound);
            }
            if *stop.borrow() {
                return Err(ConversationError::Unavailable);
            }
            let caller = ConversationCaller {
                organization_id: record.organization().clone(),
                principal_id: principal,
                surface_id: actor.surface_id().into(),
                action_id: request.command,
            };
            caller.actor()?;
            Ok(AgentToolSource {
                caller,
                source,
                execution: execution.as_str().into(),
            })
        })
        .await
        .map_err(|_| ConversationError::Unavailable)?
    }
}
