//! Resolve source authority before delegating to existing target operations.
use super::running::running_turn;
use super::{load_conversation, ConversationCaller, ConversationError, ConversationService};
use crate::application::{
    AccessAction, AccessAsk, AccessDecision, AccessDenial, AccessResource, AccessSubject,
    AgentToolAudit, AgentToolAuditRecord, AgentToolRequest, AgentToolSource, RequestShape,
};
use nessa_auth::domain::PrincipalId;
use nessa_protocol::conversation::domain::{ConversationId, ConversationSummary};
use nessa_protocol::conversation::view::{DecisionSettings, OwnerAsk, OwnerDecision, RaisedAccess};
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

/// The agent tool that asks the owner to raise the chat's access. It only
/// asks, as `ask_owner` does, and the owner's own access command is what
/// raises it; it is admitted under [`AccessAction::RequestAccess`], in a
/// turn the owner started only.
pub const REQUEST_ACCESS_TOOL: &str = "request_access";

/// What became of a request for more access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessRequestOutcome {
    /// The owner is asked, on the red card.
    Asked,
    /// The chat already runs at that access or more: nothing to ask.
    Held,
}

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
        ask: OwnerAsk,
    ) -> Result<(), ConversationError> {
        self.keep_ask(conversation, ask, None).await.map(|_| ())
    }

    /// Ask the owner, as [`Self::ask_owner`] does, to raise the chat to
    /// `access`, for the agent's one-line `reason`. Nessa writes the question
    /// and the choices. A chat whose saved mode is already that access or
    /// more is not asked. That read is a courtesy, not a guard: the mode
    /// changes under its own lock, so a change landing at the same moment
    /// leaves at most one card too many or too few. Asking never raises the
    /// access, so neither grants anything.
    pub async fn request_access(
        &self,
        conversation: &ConversationId,
        access: RaisedAccess,
        ask_id: String,
        reason: String,
    ) -> Result<AccessRequestOutcome, ConversationError> {
        self.keep_ask(conversation, access.ask(ask_id, reason), Some(access))
            .await
    }

    async fn keep_ask(
        &self,
        conversation: &ConversationId,
        mut ask: OwnerAsk,
        raising: Option<RaisedAccess>,
    ) -> Result<AccessRequestOutcome, ConversationError> {
        ask.asked_at_ms = self.inner.clock.unix_milliseconds();
        ask.check().map_err(|_| ConversationError::InvalidInput)?;
        let _writes = self.inner.records.summary_writes().lock(conversation).await;
        let record = match load_conversation(self.inner.metadata.as_ref(), conversation).await? {
            Some(record) if record.deletion().is_none() => {
                if raising.is_some_and(|access| access.held_at(record.approval_mode())) {
                    return Ok(AccessRequestOutcome::Held);
                }
                record
            }
            Some(_) => return Err(ConversationError::Deleted),
            None => return Err(ConversationError::NotFound),
        };
        let previous = self.inner.summaries.load(conversation).await?;
        let next = ConversationSummary::after_asking(previous.as_ref(), ask.clone());
        let replaced = next.settled().cloned();
        self.inner.summaries.record(conversation, next).await?;
        self.inner.live_changes.publish(conversation);
        // Measured only: settings that cannot be read leave them off the trace.
        let settings = self
            .inner
            .summaries
            .decision_settings(record.organization(), record.owner())
            .await
            .ok();
        if let Some(replaced) = replaced {
            self.inner
                .decision_traces
                .settled(&self.telemetry(), conversation, &replaced);
        }
        self.inner.decision_traces.asked(
            &self.telemetry(),
            conversation,
            record.agent(),
            &ask,
            settings,
        );
        self.inner.auto_pick_wakeup.notify_one();
        Ok(AccessRequestOutcome::Asked)
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
        let mut decisions = self.inner.summaries.decisions(&conversation).await?;
        if decisions.iter().any(|decision| decision.outcome.is_none()) {
            let settings = self
                .inner
                .summaries
                .decision_settings(record.organization(), record.owner())
                .await?;
            for decision in &mut decisions {
                decision.auto_pick_due_at_ms = decision.auto_pick_due(settings);
            }
        }
        Ok(decisions)
    }

    /// How the caller's decisions behave, as they last set it.
    pub async fn decision_settings(
        &self,
        caller: ConversationCaller,
    ) -> Result<DecisionSettings, ConversationError> {
        let _admission = self.admit().await?;
        caller.actor()?;
        self.inner
            .summaries
            .decision_settings(&caller.organization_id, &caller.principal_id)
            .await
    }

    /// Keep how the caller's decisions behave, and return it as kept. A wait
    /// outside the bounds is refused.
    pub async fn set_decision_settings(
        &self,
        caller: ConversationCaller,
        settings: DecisionSettings,
    ) -> Result<DecisionSettings, ConversationError> {
        let _admission = self.admit().await?;
        caller.actor()?;
        let settings = settings.checked().ok_or(ConversationError::InvalidInput)?;
        self.inner
            .summaries
            .save_decision_settings(&caller.organization_id, &caller.principal_id, settings)
            .await?;
        self.inner.auto_pick_wakeup.notify_one();
        Ok(settings)
    }

    /// Pause auto-pick on the caller's waiting ask `ask_id`, or resume it.
    /// The owner's alone, as the decisions are. False when that ask is not
    /// waiting, or was already so.
    pub async fn pause_auto_pick(
        &self,
        conversation: ConversationId,
        caller: ConversationCaller,
        ask_id: &str,
        paused: bool,
    ) -> Result<bool, ConversationError> {
        let _admission = self.admit().await?;
        caller.actor()?;
        let record = load_conversation(self.inner.metadata.as_ref(), &conversation)
            .await?
            .ok_or(ConversationError::NotFound)?;
        self.access().admit(&record, &caller)?;
        // Read and written under the summary lock auto-pick records its
        // answer under: a pause either lands first and holds the pick, or
        // finds the ask answered and says it did not apply.
        let _writes = self
            .inner
            .records
            .summary_writes()
            .lock(&conversation)
            .await;
        let waiting = self
            .inner
            .summaries
            .decisions(&conversation)
            .await?
            .into_iter()
            .find(|decision| decision.ask.ask_id == ask_id && decision.outcome.is_none());
        let Some(waiting) = waiting else {
            return Ok(false);
        };
        if waiting.auto_pick_paused == paused {
            return Ok(false);
        }
        let at_ms = paused.then(|| self.inner.clock.unix_milliseconds());
        let applied = self
            .inner
            .summaries
            .pause_auto_pick(&conversation, ask_id, at_ms)
            .await?;
        if applied {
            self.inner.live_changes.publish(&conversation);
            self.inner.auto_pick_wakeup.notify_one();
            self.inner.decision_traces.event(
                &self.telemetry(),
                ask_id,
                if paused {
                    "auto_pick_paused"
                } else {
                    "auto_pick_resumed"
                },
                at_ms.unwrap_or_else(|| self.inner.clock.unix_milliseconds()),
            );
        }
        Ok(applied)
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
                REQUEST_ACCESS_TOOL => AccessAction::RequestAccess,
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
