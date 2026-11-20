//! Resolve source authority before delegating to existing target operations.
use super::environment_tools::running_turn;
use super::{load_conversation, ConversationCaller, ConversationError, ConversationService};
use crate::application::{
    AccessAction, AccessAsk, AccessDecision, AccessDenial, AccessResource, AccessSubject,
    AgentToolAudit, AgentToolAuditRecord, AgentToolRequest, AgentToolSource, RequestShape,
};
use nessa_auth::domain::PrincipalId;
use nessa_protocol::conversation::domain::{ConversationId, ConversationSummary};
use nessa_protocol::conversation::view::{OwnerAsk, OwnerDecision};
use nessa_sdk::domain::agent_execution::executions::Initiator;
use std::sync::Arc;
use tokio::sync::watch;

/// The one agent tool that only reads, and so is admitted under
/// [`AccessAction::ReadTurns`] in any turn of the owner's, one another agent
/// started included.
pub const READ_TURN_TOOL: &str = "read_turn";

/// The agent tool that asks the owner a question, and so is admitted under
/// [`AccessAction::AskOwner`] in any turn of the owner's.
pub const ASK_OWNER_TOOL: &str = "ask_owner";

/// The agent tool that, given another thread, closes it, and so is also
/// asked under [`AccessAction::CloseCreated`] on that thread.
pub const CLOSE_THREAD_TOOL: &str = "close_thread";

impl ConversationService {
    /// Keep the question an admitted turn asked its conversation's owner, in
    /// place of any earlier one, until the owner writes in the conversation,
    /// dated now. It is kept on the conversation's summary, written as every
    /// summary is, under the summary lock a delete erases it under: the list
    /// shows it from the next read, at the top and out of the archive, and
    /// the summary write that records the owner's answer takes it down.
    pub async fn ask_owner(
        &self,
        conversation: &ConversationId,
        mut ask: OwnerAsk,
    ) -> Result<(), ConversationError> {
        ask.asked_at_ms = self.inner.clock.unix_milliseconds();
        ask.check().map_err(|_| ConversationError::InvalidInput)?;
        let _writes = self.inner.records.summary_writes().lock(conversation).await;
        match load_conversation(self.inner.metadata.as_ref(), conversation).await? {
            Some(record) if record.deletion().is_none() => {}
            Some(_) => return Err(ConversationError::Deleted),
            None => return Err(ConversationError::NotFound),
        }
        let previous = self.inner.summaries.load(conversation).await?;
        self.inner
            .summaries
            .record(
                conversation,
                ConversationSummary::after_asking(previous.as_ref(), ask),
            )
            .await?;
        self.inner.live_changes.publish(conversation);
        Ok(())
    }

    /// What the conversation's agents asked its owner (`ask_owner`) and
    /// what became of each, newest first: for the owner alone, as the ask
    /// on a list row is.
    pub async fn decisions(
        &self,
        conversation: ConversationId,
        caller: ConversationCaller,
    ) -> Result<Vec<OwnerDecision>, ConversationError> {
        let _admission = self.admit().await?;
        caller.actor()?;
        let record = load_conversation(self.inner.metadata.as_ref(), &conversation)
            .await?
            .ok_or(ConversationError::NotFound)?;
        self.access().admit(&record, &caller)?;
        self.inner.summaries.decisions(&conversation).await
    }

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
            service
                .authorize_admitted_agent_tool(
                    source,
                    call,
                    tool,
                    target,
                    request,
                    stop,
                    _physical,
                    &_admission,
                )
                .await
        })
        .await
        .map_err(|_| ConversationError::Unavailable)?
    }

    /// Resolve and audit authority under the caller's existing admission. The
    /// held read prevents export/retirement from cutting this operation in two.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn authorize_admitted_agent_tool(
        &self,
        source: ConversationId,
        call: String,
        tool: String,
        target: Option<ConversationId>,
        request: AgentToolRequest,
        stop: watch::Receiver<bool>,
        _physical: crate::application::SourcePermit,
        _admission: &tokio::sync::RwLockReadGuard<'_, ()>,
    ) -> Result<AgentToolSource, ConversationError> {
        // Preserve an early existence diagnostic; this record is not the
        // source authority used after waiting for runtime evidence.
        load_conversation(self.inner.metadata.as_ref(), &source)
            .await?
            .ok_or(ConversationError::NotFound)?;
        let live = self
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
        let principal =
            PrincipalId::new(actor.principal_id()).map_err(|_| ConversationError::InvalidInput)?;
        let owner_initiated = matches!(initiation.initiator(), Initiator::Owner);
        // The snapshot fixes provenance; current metadata fixes resource
        // authority. In particular, a fence may have landed during live()
        // or snapshot(). Read it last, directly before synchronous Cedar.
        let record = load_conversation(self.inner.metadata.as_ref(), &source)
            .await?
            .ok_or(ConversationError::NotFound)?;
        // Closing another thread is also asked on that thread, with the
        // thread that created it, as the SDK's record of the spawn says, as
        // a fact: the policy decides whether this turn's thread is it.
        let closed = match (&target, tool.as_str()) {
            (Some(target), CLOSE_THREAD_TOOL) => Some((
                load_conversation(self.inner.metadata.as_ref(), target).await?,
                self.thread_creator(target),
            )),
            _ => None,
        };
        let conversation = source.to_string();
        let subject = AccessSubject::AgentTurn {
            organization: record.organization(),
            principal: &principal,
            owner_initiated,
            conversation: &conversation,
        };
        let decision = self.inner.access_policy.evaluate(&AccessAsk {
            subject,
            action: match tool.as_str() {
                READ_TURN_TOOL => AccessAction::ReadTurns,
                ASK_OWNER_TOOL => AccessAction::AskOwner,
                _ => AccessAction::AgentTools,
            },
            resource: AccessResource::of(&record),
            held: None,
            request: RequestShape::default(),
            target: None,
        });
        let decision = match (decision, &closed) {
            (AccessDecision::Allow { mut policies }, Some((child, creator))) => {
                // A thread that is not saved is never another's to close.
                let Some(child) = child else {
                    return Err(ConversationError::NotFound);
                };
                match self.inner.access_policy.evaluate(&AccessAsk {
                    subject,
                    action: AccessAction::CloseCreated,
                    resource: AccessResource::created(child, creator.as_deref()),
                    held: None,
                    request: RequestShape::default(),
                    target: None,
                }) {
                    AccessDecision::Allow { policies: closing } => {
                        policies.extend(closing);
                        AccessDecision::Allow { policies }
                    }
                    refused => refused,
                }
            }
            (decision, _) => decision,
        };
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
        let audit = self
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
            execution,
        })
    }
}
