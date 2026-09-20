use super::{
    projection::Projection,
    view::{
        ConversationCapabilities, ConversationDisposition, ConversationMessageStatus,
        ConversationPendingMode, ConversationReorderOutcome, ConversationRuntime, ConversationView,
        SubmissionReceipt,
    },
    ConversationError, ConversationRepository,
};
use crate::agents::domain::AgentId;
use crate::conversation::domain::{Conversation, ConversationId};
use futures_util::{future::join_all, FutureExt};
use nessa_auth::application::ports::Clock;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::application::agent_execution::{
    agents::{
        Agent, AgentError, AgentInitializationError, QueueRemoval, QueueReorder, QueuedInvocation,
        SteeringDelivery,
    },
    executions::ExecutionRequest,
    permissions::{
        ActionContext, ApprovalAttribution, ApprovalBasis, PermissionAnswer,
        PermissionCancellationRequest, PermissionSelectionState,
    },
    providers::AgentProvider,
    sessions::{SessionManager, SessionStorage},
};
use nessa_sdk::domain::agent_execution::{
    executions::ExecutionId,
    permissions::{
        CustomPermissionCancellationReason, PermissionCancellationReason, PermissionId,
        PermissionOptionId,
    },
    prompts::PromptText,
    sessions::SessionId,
};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    mem,
    panic::AssertUnwindSafe,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
};
use tokio::sync::{Mutex, Notify, OnceCell, RwLock, RwLockReadGuard};
use uuid::Uuid;

/// Authenticated identity and stable logical action supplied by the gateway boundary.
#[derive(Clone)]
pub struct ConversationCaller {
    pub organization_id: OrganizationId,
    pub principal_id: PrincipalId,
    pub surface_id: String,
    pub action_id: String,
}
impl ConversationCaller {
    fn actor(&self) -> Result<ActionContext, ConversationError> {
        ActionContext::new(
            self.principal_id.as_str(),
            &self.surface_id,
            &self.action_id,
        )
        .map_err(|_| ConversationError::InvalidInput)
    }
}
/// What a creation said about the agent it wants.
///
/// Three cases rather than two, because a name this build has no adapter for is
/// not the same as no name at all and must not be turned into one. It is kept
/// as a case instead of being refused where the request is parsed, because a
/// creation for a conversation that already exists reopens it on the agent its
/// own record names and never looks at this at all: the panel sends its
/// remembered choice on every send, close, reorder and permission answer, so
/// refusing the name outright made a name from another build — or from a
/// rolled-back one — fail every one of those in every existing conversation,
/// with no way back but editing the host's settings by hand.
///
/// A misspelling is still not an installation fact, so it keeps its own
/// refusal. It is simply given at the point the name would have been used.
#[derive(Clone, Copy)]
pub enum RequestedAgent {
    /// A name this build has an adapter for.
    Known(AgentId),
    /// A name it has none for.
    Unknown,
}

/// Server-selected admission limits. Clients cannot choose provider budgets or owner capacity.
///
/// What is here is the same for every agent. The output budget a submission
/// reserves is not: it is the ceiling that agent was configured with, so it
/// travels with the agent, in [`ConversationAgent`].
#[derive(Clone, Copy)]
pub struct ConversationLimits {
    pub max_conversations: usize,
    pub max_input_bytes: usize,
}
impl Default for ConversationLimits {
    fn default() -> Self {
        Self {
            max_conversations: 32,
            max_input_bytes: 8192,
        }
    }
}

/// One agent this server can run conversations on.
///
/// Composition builds one of these per configured agent and the service keeps
/// them all: a conversation is reopened on the agent it was created on, so the
/// agent nobody has selected is still the agent yesterday's conversations need.
#[derive(Clone)]
pub struct ConversationAgent {
    /// What opens an execution session for it.
    pub provider: Arc<dyn AgentProvider>,
    /// The output budget every submission to this agent reserves.
    pub reserved_output_tokens: u32,
}

/// Every agent this server can start, and the one a caller who names none gets.
///
/// One value rather than two parameters, because the two are only valid
/// together: a default that is not among the configured agents would accept a
/// creation this server can never open. Checked here, so that no code holding a
/// [`ConversationAgents`] has to consider the pairing invalid.
#[derive(Clone)]
pub struct ConversationAgents {
    agents: HashMap<AgentId, ConversationAgent>,
    default_agent: AgentId,
}
impl ConversationAgents {
    /// # Errors
    /// Returns [`ConversationError::InvalidInput`] when `default_agent` is not
    /// among `agents`, or when any agent reserves no output at all.
    pub fn new(
        agents: HashMap<AgentId, ConversationAgent>,
        default_agent: AgentId,
    ) -> Result<Self, ConversationError> {
        if !agents.contains_key(&default_agent)
            || agents
                .values()
                .any(|agent| agent.reserved_output_tokens == 0)
        {
            return Err(ConversationError::InvalidInput);
        }
        Ok(Self {
            agents,
            default_agent,
        })
    }
    /// What runs this agent, or nothing where this server cannot start it.
    fn get(&self, agent: AgentId) -> Option<&ConversationAgent> {
        self.agents.get(&agent)
    }
    /// The agent a creation that names none is made on.
    fn default_agent(&self) -> AgentId {
        self.default_agent
    }
}
#[derive(Clone, Copy)]
pub enum SubmissionMode {
    Queue,
    Steer,
}
struct LiveConversation {
    agent: Agent,
    /// The configured output reservation of the agent this conversation runs
    /// on, read once when it was opened.
    reserved_output_tokens: u32,
    projection: Mutex<Projection>,
    watched: Mutex<HashSet<String>>,
}
/// A first opening that did not produce a live conversation.
///
/// `holds` is the one thing this has to say about such a failure: does the
/// attempt still own something — a provider part-way through initialization, or
/// a state nothing can describe after a panic — so that the
/// `max_conversations` slot it was given has to stay taken. A failure that
/// acquired nothing gives its slot back.
///
/// This was a `retryable` flag until now, which is a different question, and
/// one only the caller asks; [`ConversationError`] already answers it at the
/// wire. The two agreed for as long as every permanent failure had genuinely
/// retained something, and parted company at the first one that acquired
/// nothing at all: an agent dropped from the configuration refuses each of its
/// conversations instantly and permanently, and thirty-two such refusals used
/// to leave the server unable to create any conversation at all.
struct OpeningFailure {
    cause: ConversationError,
    _cleanup: Option<AgentInitializationError>,
    holds: bool,
}
struct Slot {
    value: OnceCell<Result<Arc<LiveConversation>, OpeningFailure>>,
    ready: Notify,
    started: AtomicBool,
}
struct Inner {
    workspace: Option<String>,
    /// Every agent this server can start, and which of them a creation that
    /// names none is made on.
    agents: ConversationAgents,
    storage: Arc<dyn SessionStorage>,
    metadata: Arc<dyn ConversationRepository>,
    creation_audit: Arc<dyn super::ConversationCreationAudit>,
    clock: Arc<dyn Clock>,
    limits: ConversationLimits,
    conversations: Mutex<HashMap<ConversationId, Arc<Slot>>>,
    creation: Mutex<()>,
    retirement: OnceLock<ActionContext>,
    admission: RwLock<()>,
}
/// Owns Agents independently of authenticated socket lifetimes. Clones share all owners.
#[derive(Clone)]
pub struct ConversationService {
    inner: Arc<Inner>,
}
async fn supervised<T: Send + 'static>(
    operation: impl Future<Output = Result<T, ConversationError>> + Send + 'static,
) -> Result<T, ConversationError> {
    tokio::spawn(operation)
        .await
        .map_err(|_| ConversationError::Unavailable)?
}
impl ConversationService {
    /// Own every configured agent, and the one a caller gets by default.
    pub fn new(
        agents: ConversationAgents,
        storage: Arc<dyn SessionStorage>,
        metadata: Arc<dyn ConversationRepository>,
        creation_audit: Arc<dyn super::ConversationCreationAudit>,
        clock: Arc<dyn Clock>,
        limits: ConversationLimits,
        workspace: Option<String>,
    ) -> Result<Self, ConversationError> {
        if workspace.as_ref().is_some_and(|value| value.len() > 4096)
            || limits.max_conversations == 0
            || limits.max_input_bytes == 0
            || limits.max_input_bytes > 8192
        {
            return Err(ConversationError::InvalidInput);
        }
        Ok(Self {
            inner: Arc::new(Inner {
                workspace,
                agents,
                storage,
                metadata,
                creation_audit,
                clock,
                limits,
                conversations: Mutex::new(HashMap::new()),
                creation: Mutex::new(()),
                retirement: OnceLock::new(),
                admission: RwLock::new(()),
            }),
        })
    }
    /// Persist ownership before opening a provider. Repeating the same UUID never changes its owner.
    ///
    /// `agent` is the agent the conversation runs on for the rest of its life;
    /// `None` takes this server's configured default. It is only consulted for
    /// a conversation that does not exist yet: repeating a UUID reopens the
    /// conversation on record, on the agent it was created with, whatever this
    /// caller asked for.
    pub async fn create(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        agent: Option<RequestedAgent>,
    ) -> Result<(), ConversationError> {
        let service = self.clone();
        supervised(async move {
            let _admission = service.admit().await?;
            caller.actor()?;
            if service.inner.retirement.get().is_some() {
                return Err(ConversationError::Unavailable);
            }
            let requested_at_ms = service.inner.clock.unix_milliseconds();
            // Serialize create/reopen decisions without holding the live-owner map
            // across repository or audit I/O. Existing ownership is checked before
            // this request can reserve capacity or open a provider.
            let creation_guard = service.inner.creation.lock().await;
            if let Some(record) = service.inner.metadata.load(&id).await? {
                if !record.allows(&caller.organization_id, &caller.principal_id) {
                    return Err(ConversationError::NotFound);
                }
                drop(creation_guard);
                // Acknowledge the original creation from its stored creator
                // evidence before attributing this reopen to its caller, and
                // before any provider opening. An unavailable audit sink
                // therefore refuses the reopen instead of leaving an opened
                // conversation with no record of who reopened it.
                service.reconcile_creation_audit(&record).await?;
                if caller.action_id != record.creation_action() {
                    service
                        .inner
                        .creation_audit
                        .record(super::ConversationCreationAuditRecord {
                            conversation_id: record.id().clone(),
                            organization_id: record.organization().clone(),
                            owner_id: record.owner().clone(),
                            before: super::ConversationOwnershipState::Owned,
                            after: super::ConversationOwnershipState::Owned,
                            cause: super::ConversationCreationCause::IdempotentReopen,
                            initiator_principal_id: caller.principal_id.clone(),
                            initiator_surface_id: caller.surface_id.clone(),
                            correlation_id: caller.action_id.clone(),
                            requested_at_ms,
                            observed_at_ms: service.inner.clock.unix_milliseconds(),
                        })
                        .await
                        .map_err(|_| ConversationError::Audit)?;
                }
                service.resolve(&id, &caller).await?;
                return Ok(());
            }
            // Only now does the agent this caller asked for matter. Checking it
            // before the record above would have refused to reopen somebody's
            // existing Claude conversation because the panel's remembered choice
            // names an agent this server is no longer configured for — a
            // conversation that does not need that agent at all. The same is
            // true of a name no adapter exists for, which is why that one is
            // carried this far instead of being refused where it was parsed.
            let agent = match agent {
                Some(RequestedAgent::Known(agent)) => agent,
                Some(RequestedAgent::Unknown) => return Err(ConversationError::InvalidInput),
                None => service.inner.agents.default_agent(),
            };
            if service.inner.agents.get(agent).is_none() {
                return Err(ConversationError::AgentNotConfigured);
            }
            let proposed = Conversation::new(
                id.clone(),
                caller.organization_id.clone(),
                caller.principal_id.clone(),
                caller.surface_id.clone(),
                caller.action_id.clone(),
                requested_at_ms,
                agent,
            )
            .map_err(|_| ConversationError::InvalidInput)?;
            {
                let owners = service.inner.conversations.lock().await;
                if service.inner.retirement.get().is_some() {
                    return Err(ConversationError::Unavailable);
                }
                if !owners.contains_key(&id)
                    && owners.len() >= service.inner.limits.max_conversations
                {
                    return Err(ConversationError::Capacity);
                }
            }
            let outcome = service.inner.metadata.create(proposed).await?;
            let record = &outcome.conversation;
            if !record.allows(&caller.organization_id, &caller.principal_id) {
                return Err(ConversationError::NotFound);
            }
            service.reconcile_creation_audit(record).await?;
            if outcome.disposition == super::ConversationCreationDisposition::Existing
                && caller.action_id != record.creation_action()
            {
                service
                    .inner
                    .creation_audit
                    .record(super::ConversationCreationAuditRecord {
                        conversation_id: record.id().clone(),
                        organization_id: record.organization().clone(),
                        owner_id: record.owner().clone(),
                        before: super::ConversationOwnershipState::Owned,
                        after: super::ConversationOwnershipState::Owned,
                        cause: super::ConversationCreationCause::IdempotentReopen,
                        initiator_principal_id: caller.principal_id.clone(),
                        initiator_surface_id: caller.surface_id.clone(),
                        correlation_id: caller.action_id.clone(),
                        requested_at_ms,
                        observed_at_ms: service.inner.clock.unix_milliseconds(),
                    })
                    .await
                    .map_err(|_| ConversationError::Audit)?;
            }
            let slot = {
                let mut owners = service.inner.conversations.lock().await;
                if service.inner.retirement.get().is_some() {
                    return Err(ConversationError::Unavailable);
                }
                if !owners.contains_key(&id)
                    && owners.len() >= service.inner.limits.max_conversations
                {
                    return Err(ConversationError::Capacity);
                }
                match owners.entry(id.clone()) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.get().clone(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let slot = Arc::new(Slot {
                            value: OnceCell::new(),
                            ready: Notify::new(),
                            started: AtomicBool::new(false),
                        });
                        entry.insert(slot.clone());
                        service.start_slot(id.clone(), slot.clone());
                        slot
                    }
                }
            };
            drop(creation_guard);
            service.open_slot(&id, slot).await?;
            Ok(())
        })
        .await
    }
    /// Acknowledge the original creation from stored creator evidence.
    ///
    /// Caller-requested creation records are idempotent by conversation
    /// identity, so every entry point can reconcile the same evidence before it
    /// opens a provider without inventing a second creation.
    async fn reconcile_creation_audit(
        &self,
        record: &Conversation,
    ) -> Result<(), ConversationError> {
        self.inner
            .creation_audit
            .record(super::ConversationCreationAuditRecord {
                conversation_id: record.id().clone(),
                organization_id: record.organization().clone(),
                owner_id: record.owner().clone(),
                before: super::ConversationOwnershipState::Absent,
                after: super::ConversationOwnershipState::Owned,
                cause: super::ConversationCreationCause::CallerRequested,
                initiator_principal_id: record.owner().clone(),
                initiator_surface_id: record.creator_surface().to_owned(),
                correlation_id: record.creation_action().to_owned(),
                requested_at_ms: record.creation_requested_at_ms(),
                observed_at_ms: self.inner.clock.unix_milliseconds(),
            })
            .await
            .map_err(|_| ConversationError::Audit)
    }
    async fn resolve(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<Arc<LiveConversation>, ConversationError> {
        caller.actor()?;
        let record = self
            .inner
            .metadata
            .load(id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        if !record.allows(&caller.organization_id, &caller.principal_id) {
            return Err(ConversationError::NotFound);
        }
        let slot = {
            let mut owners = self.inner.conversations.lock().await;
            if self.inner.retirement.get().is_some() {
                return Err(ConversationError::Unavailable);
            }
            if let Some(slot) = owners.get(id) {
                slot.clone()
            } else {
                if owners.len() >= self.inner.limits.max_conversations {
                    return Err(ConversationError::Capacity);
                }
                let slot = Arc::new(Slot {
                    value: OnceCell::new(),
                    ready: Notify::new(),
                    started: AtomicBool::new(false),
                });
                owners.insert(id.clone(), slot.clone());
                self.start_slot(id.clone(), slot.clone());
                slot
            }
        };
        self.open_slot(id, slot).await
    }
    async fn open_slot(
        &self,
        id: &ConversationId,
        slot: Arc<Slot>,
    ) -> Result<Arc<LiveConversation>, ConversationError> {
        if slot.value.get().is_none() {
            self.start_slot(id.clone(), slot.clone());
        }
        self.wait_for_slot(id, slot).await
    }
    fn start_slot(&self, id: ConversationId, slot: Arc<Slot>) {
        let service = self.clone();
        let owner = slot.clone();
        if !slot.started.swap(true, Ordering::SeqCst) {
            tokio::spawn(async move {
                let result = owner
                    .value
                    .get_or_init(|| async {
                        let opening = async {
                            // One gate for every first provider opening: read the
                            // authoritative creator evidence and reconcile its
                            // mandatory creation audit before any provider work.
                            // Repeating it is idempotent by conversation identity.
                            let record = service
                                .inner
                                .metadata
                                .load(&id)
                                .await
                                .map_err(|cause| OpeningFailure { cause, _cleanup: None, holds: false })?
                                .ok_or(OpeningFailure {
                                    cause: ConversationError::NotFound,
                                    _cleanup: None,
                                    holds: false,
                                })?;
                            service
                                .reconcile_creation_audit(&record)
                                .await
                                .map_err(|cause| OpeningFailure {
                                    cause,
                                    _cleanup: None,
                                    holds: false,
                                })?;
                            // The agent the record names, never this server's
                            // current default: a conversation restores a
                            // provider session that belongs to one agent, and
                            // reopening it on another would hand that session
                            // to a harness that never wrote it.
                            //
                            // Settled before the storage lease is taken. A
                            // conversation this build cannot open is refused the
                            // same way on every attempt, and there is no reason
                            // for each of those attempts to acquire the
                            // exclusive lease and drop it again.
                            let configured = service
                                .inner
                                .agents
                                .get(record.agent())
                                .cloned()
                                .ok_or(OpeningFailure {
                                    cause: ConversationError::AgentNotConfigured,
                                    _cleanup: None,
                                    holds: false,
                                })?;
                            let session_id =
                                SessionId::new(id.to_string()).expect("UUID session key");
                            let manager = SessionManager::open(
                                Some(session_id),
                                service.inner.storage.clone(),
                            )
                            .await
                            .map_err(|error| {
                                tracing::error!(conversation_id = %id, %error, "conversation storage opening failed");
                                OpeningFailure { cause: ConversationError::Storage(error), _cleanup: None, holds: false }
                            })?;
                            let agent = Agent::new(configured.provider.clone(), manager)
                                .await
                                .map_err(|error| {
                                tracing::error!(conversation_id = %id, %error, "conversation restoration failed");
                                // The one question that decides ownership: a
                                // provider left part-way through initialization
                                // is this slot's to finish closing.
                                let holds = error.needs_cleanup();
                                OpeningFailure {
                                    cause: ConversationError::Agent(error.cause().clone()),
                                    _cleanup: Some(error),
                                    holds,
                                }
                            })?;
                            let mut events = agent.subscribe();
                            let snapshot = agent.session_manager().snapshot().await;
                            let capabilities = ConversationCapabilities {
                                queue: true,
                                steer: true,
                                resume: agent.operation_capabilities().session_resume,
                                permissions: agent.capabilities().features().tool_use(),
                            };
                            let mut projection =
                                Projection::new(id.to_string(), capabilities, snapshot.as_ref());
                            if let Some(workspace) = &service.inner.workspace {
                                let identity = configured.provider.identity();
                                projection.view.runtime = Some(ConversationRuntime {
                                    model: identity.model_id().into(), provider: identity.name().into(), workspace: workspace.clone(),
                                });
                            }
                            let live = Arc::new(LiveConversation {
                                agent,
                                reserved_output_tokens: configured.reserved_output_tokens,
                                projection: Mutex::new(projection),
                                watched: Mutex::new(HashSet::new()),
                            });
                            let weak = Arc::downgrade(&live);
                            tokio::spawn(async move {
                                loop {
                                    let event = events.next().await;
                                    let Some(live) = weak.upgrade() else {
                                        break;
                                    };
                                    match event {
                                        Ok(Some(event)) => {
                                            live.projection.lock().await.event(&event)
                                        }
                                        Err(_) => live.projection.lock().await.lagged(),
                                        Ok(None) => break,
                                    }
                                }
                            });
                            Ok(live)
                        };
                        match AssertUnwindSafe(opening).catch_unwind().await {
                            Ok(result) => result,
                            Err(payload) => {
                                // An adapter panic payload may itself panic on drop.
                                // Publish failure before allowing any waiter to hang.
                                mem::forget(payload);
                                // Nothing can say what the panic left behind,
                                // so the slot stays taken rather than being
                                // handed to an opening that assumes it is free.
                                Err(OpeningFailure { cause: ConversationError::Unavailable, _cleanup: None, holds: true })
                            }
                        }
                    })
                    .await;
                owner.ready.notify_waiters();
                if result.as_ref().is_err_and(|failure| !failure.holds) {
                    let mut owners = service.inner.conversations.lock().await;
                    if owners
                        .get(&id)
                        .is_some_and(|known| Arc::ptr_eq(known, &owner))
                    {
                        owners.remove(&id);
                    }
                }
            });
        }
    }
    async fn wait_for_slot(
        &self,
        id: &ConversationId,
        slot: Arc<Slot>,
    ) -> Result<Arc<LiveConversation>, ConversationError> {
        loop {
            let ready = slot.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            if let Some(result) = slot.value.get() {
                let response = result
                    .as_ref()
                    .cloned()
                    .map_err(|failure| failure.cause.clone());
                if result.as_ref().is_err_and(|failure| !failure.holds) {
                    let mut owners = self.inner.conversations.lock().await;
                    if owners
                        .get(id)
                        .is_some_and(|known| Arc::ptr_eq(known, &slot))
                    {
                        owners.remove(id);
                    }
                }
                return response;
            }
            ready.await;
        }
    }
    /// Read a bounded current replacement projection; never replay provider input to reconstruct it.
    pub async fn read(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
    ) -> Result<ConversationView, ConversationError> {
        let _admission = self.admit().await?;
        let live = self.resolve(&id, &caller).await?;
        let order = live.agent.queued_ids().await;
        let snapshot = live.agent.session_manager().snapshot().await;
        let mut projection = live.projection.lock().await;
        projection.recover_permissions(snapshot.as_ref());
        projection.queue_order(&order);
        projection.view.capabilities.resume = live.agent.operation_capabilities().session_resume;
        Ok(projection.read())
    }
    /// Admit one SDK-owned queued/steering input. Its completion outlives this call and its socket.
    pub async fn submit(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        execution_id: String,
        text: String,
        mode: SubmissionMode,
    ) -> Result<SubmissionReceipt, ConversationError> {
        let service = self.clone();
        supervised(async move {
            let _admission = service.admit().await?;
            let actor = caller.actor()?;
            if text.trim().is_empty() || text.len() > service.inner.limits.max_input_bytes {
                return Err(ConversationError::InvalidInput);
            }
            let execution =
                ExecutionId::new(&execution_id).map_err(|_| ConversationError::InvalidInput)?;
            let prompt =
                PromptText::new(text.clone()).map_err(|_| ConversationError::InvalidInput)?;
            let live = service.resolve(&id, &caller).await?;
            // Reserve the full effective context budget consistently across retries.
            // ACP owns hidden context/tokenization; this is a conservative admission
            // reservation, not a claim about actual token usage or provider billing.
            let limits = live.agent.capabilities().limits();
            let reserved = live.reserved_output_tokens;
            if reserved > limits.max_output() || reserved >= limits.max_context_window() {
                return Err(ConversationError::InvalidInput);
            }
            let request = ExecutionRequest {
                execution_id: execution,
                user_message: prompt,
                estimated_input_tokens: u64::from(limits.max_context_window() - reserved),
                reserved_output_tokens: reserved,
            };
            let receipt = match mode {
                SubmissionMode::Queue => Some(live.agent.enqueue(request, actor).await?),
                SubmissionMode::Steer if !live.agent.operation_capabilities().native_steering => {
                    Some(live.agent.enqueue_steering(request, actor).await?)
                }
                SubmissionMode::Steer => match live.agent.steer(request, actor).await? {
                    SteeringDelivery::Queued(receipt) => Some(receipt),
                    SteeringDelivery::Injected { .. } => None,
                },
            };
            live.projection.lock().await.admitted(
                &execution_id,
                &text,
                if matches!(mode, SubmissionMode::Queue) {
                    ConversationPendingMode::Queued
                } else {
                    ConversationPendingMode::Steering
                },
            );
            if let Some(receipt) = receipt {
                let settled = Self::watch_receipt(live, receipt).await;
                Ok(SubmissionReceipt {
                    execution_id,
                    disposition: if settled {
                        ConversationDisposition::Settled
                    } else {
                        ConversationDisposition::Queued
                    },
                })
            } else {
                let snapshot = live.agent.session_manager().snapshot().await;
                live.projection
                    .lock()
                    .await
                    .settled(&execution_id, snapshot.as_ref());
                Ok(SubmissionReceipt {
                    execution_id,
                    disposition: ConversationDisposition::Injected,
                })
            }
        })
        .await
    }
    async fn watch_receipt(live: Arc<LiveConversation>, receipt: QueuedInvocation) -> bool {
        let id = receipt.id().as_str().to_owned();
        let mut completion = Box::pin(receipt.wait());
        // Inspect the actual SDK receipt, not a second admission ledger. A retry
        // can already be settled; dropping this wait never cancels SDK work.
        if let Some(result) = completion.as_mut().now_or_never() {
            let snapshot = live.agent.session_manager().snapshot().await;
            let mut projection = live.projection.lock().await;
            projection.settled(&id, snapshot.as_ref());
            if result.is_err()
                && !(matches!(result, Err(AgentError::Closed))
                    && projection.view.messages.iter().any(|message| {
                        message.execution_id == id
                            && message.status == ConversationMessageStatus::Cancelled
                    }))
            {
                projection.receipt_failed(&id);
            }
            return true;
        }
        if !live.watched.lock().await.insert(id.clone()) {
            return false;
        }
        tokio::spawn(async move {
            let result = completion.await;
            let snapshot = live.agent.session_manager().snapshot().await;
            let mut projection = live.projection.lock().await;
            projection.settled(&id, snapshot.as_ref());
            if result.is_err()
                && !(matches!(result, Err(AgentError::Closed))
                    && projection.view.messages.iter().any(|message| {
                        message.execution_id == id
                            && message.status == ConversationMessageStatus::Cancelled
                    }))
            {
                projection.receipt_failed(&id);
            }
            drop(projection);
            live.watched.lock().await.remove(&id);
        });
        false
    }
    /// Reorder the complete pending queue without changing message identities or priority.
    /// Dispatch/removal races return QueueChanged; failed writes may have applied the move.
    pub async fn reorder(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        executions: Vec<String>,
    ) -> Result<ConversationReorderOutcome, ConversationError> {
        let service = self.clone();
        supervised(async move {
            let _admission = service.admit().await?;
            if executions.len() > 64 {
                return Err(ConversationError::InvalidInput);
            }
            let order = executions
                .into_iter()
                .map(|value| ExecutionId::new(value).map_err(|_| ConversationError::InvalidInput))
                .collect::<Result<Vec<_>, _>>()?;
            let actor = caller.actor()?;
            let live = service.resolve(&id, &caller).await?;
            let result = live.agent.reorder_queued(order, actor).await?;
            Ok(match result {
                QueueReorder::Applied => ConversationReorderOutcome::Applied,
                QueueReorder::Unchanged => ConversationReorderOutcome::Unchanged,
                QueueReorder::QueueChanged => ConversationReorderOutcome::QueueChanged,
                QueueReorder::PriorityConflict => ConversationReorderOutcome::PriorityConflict,
            })
        })
        .await
    }
    pub async fn remove(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        execution: String,
    ) -> Result<bool, ConversationError> {
        let service = self.clone();
        supervised(async move {
            let _admission = service.admit().await?;
            let actor = caller.actor()?;
            let execution =
                ExecutionId::new(&execution).map_err(|_| ConversationError::InvalidInput)?;
            let live = service.resolve(&id, &caller).await?;
            let result = live.agent.remove_queued(execution.clone(), actor).await?;
            let snapshot = live.agent.session_manager().snapshot().await;
            live.projection
                .lock()
                .await
                .settled(execution.as_str(), snapshot.as_ref());
            Ok(result == QueueRemoval::Removed)
        })
        .await
    }
    pub async fn answer(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        execution: String,
        permission: String,
        option: String,
    ) -> Result<(), ConversationError> {
        let service = self.clone();
        supervised(async move {
            let _admission = service.admit().await?;
            let actor = caller.actor()?;
            let execution_id =
                ExecutionId::new(&execution).map_err(|_| ConversationError::InvalidInput)?;
            let permission_id =
                PermissionId::new(&permission).map_err(|_| ConversationError::InvalidInput)?;
            let option_id =
                PermissionOptionId::new(option).map_err(|_| ConversationError::InvalidInput)?;
            let live = service.resolve(&id, &caller).await?;
            let answer = live
                .agent
                .answer_permission(PermissionAnswer {
                    attribution: ApprovalAttribution::new(actor, ApprovalBasis::Explicit),
                    execution_id,
                    id: permission_id,
                    option_id,
                })
                .await;
            match answer {
                Ok(_) => {
                    live.projection
                        .lock()
                        .await
                        .resolved_permission(&execution, &permission);
                    Ok(())
                }
                Err(failure) => {
                    let (error, selection) = failure.into_parts();
                    let mut projection = live.projection.lock().await;
                    match selection {
                        PermissionSelectionState::Pending => {}
                        PermissionSelectionState::Consumed => {
                            projection.resolved_permission(&execution, &permission)
                        }
                        PermissionSelectionState::Unknown => {
                            projection.uncertain_permission(&execution, &permission)
                        }
                    }
                    Err(ConversationError::PermissionAnswer { error, selection })
                }
            }
        })
        .await
    }
    pub async fn cancel_permission(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        execution: String,
        permission: String,
        reason: String,
    ) -> Result<(), ConversationError> {
        let service = self.clone();
        supervised(async move {
            let _admission = service.admit().await?;
            if reason.len() > 1024 {
                return Err(ConversationError::InvalidInput);
            }
            let actor = caller.actor()?;
            let reason = CustomPermissionCancellationReason::new("gateway_request", reason)
                .map_err(|_| ConversationError::InvalidInput)?;
            let execution_id =
                ExecutionId::new(&execution).map_err(|_| ConversationError::InvalidInput)?;
            let permission_id =
                PermissionId::new(&permission).map_err(|_| ConversationError::InvalidInput)?;
            let live = service.resolve(&id, &caller).await?;
            let _cancellation = live
                .agent
                .cancel_permission(PermissionCancellationRequest {
                    execution_id,
                    id: permission_id,
                    reason: PermissionCancellationReason::custom(reason),
                    actor,
                })
                .await?;
            live.projection
                .lock()
                .await
                .resolved_permission(&execution, &permission);
            Ok(())
        })
        .await
    }
    pub async fn close(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
    ) -> Result<(), ConversationError> {
        let service = self.clone();
        supervised(async move {
            let _admission = service.admit().await?;
            let actor = caller.actor()?;
            let live = service.resolve(&id, &caller).await?;
            let result = live.agent.close(actor).await;
            // Refresh only terminal records. A concurrently accepted new turn
            // retains its live observation state; there is no second closed flag.
            let snapshot = live.agent.session_manager().snapshot().await;
            live.projection.lock().await.settled_all(snapshot.as_ref());
            result.map(|_| ()).map_err(ConversationError::Agent)
        })
        .await
    }
    async fn admit(&self) -> Result<RwLockReadGuard<'_, ()>, ConversationError> {
        let permit = self.inner.admission.read().await;
        if self.inner.retirement.get().is_some() {
            return Err(ConversationError::Unavailable);
        }
        Ok(permit)
    }

    /// Stop admission and join every initialized or initializing owner before process exit.
    pub async fn shutdown(&self) -> Result<(), ConversationError> {
        self.retire("server_shutdown", &Uuid::new_v4().to_string())
            .await
    }

    /// Local desktop policy: stop work without stopping admission or the gateway.
    pub async fn stop_active_agents(&self) -> Result<(), ConversationError> {
        let actor = self.inner.retirement.get().cloned().unwrap_or_else(|| {
            ActionContext::new("gateway", "desktop_quit", Uuid::new_v4().to_string())
                .expect("gateway attribution")
        });
        self.close_agents(&actor).await
    }

    /// Permanently fence new operations, join admitted commands, then stop owned agents.
    /// The caller supplies the automatic lifecycle cause and stable transaction correlation.
    pub(crate) async fn retire(
        &self,
        reason: &str,
        request_id: &str,
    ) -> Result<(), ConversationError> {
        let proposed = ActionContext::new("gateway", reason, request_id)
            .map_err(|_| ConversationError::InvalidInput)?;
        self.retire_with_context(proposed).await
    }

    /// Restore an already validated lifecycle fence with its original attribution.
    pub(crate) async fn retire_with_context(
        &self,
        proposed: ActionContext,
    ) -> Result<(), ConversationError> {
        let actor = self.inner.retirement.get_or_init(|| proposed);
        // Close can interrupt an admitted provider control. If admission cannot
        // drain, still attempt every owner, but retain that uncertainty and never
        // acknowledge retirement. A later request joins the retained ownership.
        let admission = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.inner.admission.write(),
        )
        .await;
        let Ok(_exclusive) = admission else {
            let cleanup_error = self.close_agents(actor).await.err().map(Box::new);
            return Err(ConversationError::RetirementAdmission { cleanup_error });
        };
        self.close_agents(actor).await
    }

    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn retirement_cause(&self) -> Option<ActionContext> {
        self.inner.retirement.get().cloned()
    }

    async fn close_agents(&self, actor: &ActionContext) -> Result<(), ConversationError> {
        let slots: Vec<_> = self
            .inner
            .conversations
            .lock()
            .await
            .iter()
            .map(|(id, slot)| (id.clone(), slot.clone()))
            .collect();
        // Give every owner its own bounded attempt. A stalled opening or provider
        // cannot consume another owner's cleanup opportunity. Timeout means only
        // unconfirmed cleanup; the slot and supervised SDK work remain owned.
        let attempts = slots.into_iter().map(|(id, slot)| async move {
            self.start_slot(id.clone(), slot.clone());
            let attempt = async {
                loop {
                    let ready = slot.ready.notified();
                    tokio::pin!(ready);
                    ready.as_mut().enable();
                    if let Some(value) = slot.value.get() {
                        return match value {
                            Ok(live) => live.agent.close(actor.clone()).await.map(|_| ()),
                            Err(failed) => match &failed._cleanup {
                                Some(error) => error.retry_cleanup().await.map(|_| ()),
                                None => Ok(()),
                            },
                        };
                    }
                    ready.await;
                }
            };
            match tokio::time::timeout(std::time::Duration::from_secs(10), attempt).await {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some((id.to_string(), error)),
                Err(_) => Some((id.to_string(), AgentError::Deadline)),
            }
        });
        let failures: Vec<_> = join_all(attempts).await.into_iter().flatten().collect();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(ConversationError::Retirement(failures))
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/retirement.rs"]
mod retirement_tests;
