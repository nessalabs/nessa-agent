use super::{
    projection::Projection,
    view::{
        ConversationCapabilities, ConversationDisposition, ConversationMessageStatus,
        ConversationPendingMode, ConversationReorderOutcome, ConversationRuntime, ConversationView,
        SubmissionReceipt,
    },
    ConversationError, ConversationRepository,
};
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
    sessions::{SessionManager, SessionStorage, StorageError},
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
/// Server-selected admission limits. Clients cannot choose provider budgets or owner capacity.
#[derive(Clone, Copy)]
pub struct ConversationLimits {
    pub max_conversations: usize,
    pub max_input_bytes: usize,
    pub reserved_output_tokens: u32,
}
impl Default for ConversationLimits {
    fn default() -> Self {
        Self {
            max_conversations: 32,
            max_input_bytes: 8192,
            reserved_output_tokens: 4096,
        }
    }
}
#[derive(Clone, Copy)]
pub enum SubmissionMode {
    Queue,
    Steer,
}
struct LiveConversation {
    agent: Agent,
    projection: Mutex<Projection>,
    watched: Mutex<HashSet<String>>,
}
struct OpeningFailure {
    cause: ConversationError,
    _cleanup: Option<AgentInitializationError>,
    retryable: bool,
}
struct Slot {
    value: OnceCell<Result<Arc<LiveConversation>, OpeningFailure>>,
    ready: Notify,
    started: AtomicBool,
}
struct Inner {
    workspace: Option<String>,
    provider: Arc<dyn AgentProvider>,
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
fn retryable_agent_open(error: &AgentError) -> bool {
    !matches!(
        error,
        AgentError::Configuration(_)
            | AgentError::Unsupported(_)
            | AgentError::InvalidInput(_)
            | AgentError::Storage(StorageError::Corrupt(_) | StorageError::IdentityMismatch)
    )
}
impl ConversationService {
    pub fn new(
        provider: Arc<dyn AgentProvider>,
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
            || limits.reserved_output_tokens == 0
        {
            return Err(ConversationError::InvalidInput);
        }
        Ok(Self {
            inner: Arc::new(Inner {
                workspace,
                provider,
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
    pub async fn create(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
    ) -> Result<(), ConversationError> {
        let service = self.clone();
        supervised(async move {
            let _admission = service.admit().await?;
            caller.actor()?;
            if service.inner.retirement.get().is_some() {
                return Err(ConversationError::Unavailable);
            }
            let requested_at_ms = service.inner.clock.unix_milliseconds();
            let proposed = Conversation::new(
                id.clone(),
                caller.organization_id.clone(),
                caller.principal_id.clone(),
                caller.surface_id.clone(),
                caller.action_id.clone(),
                requested_at_ms,
            )
            .map_err(|_| ConversationError::InvalidInput)?;
            // Serialize create/reopen decisions without holding the live-owner map
            // across repository or audit I/O. Existing ownership is checked before
            // this request can reserve capacity or open a provider.
            let creation_guard = service.inner.creation.lock().await;
            if let Some(record) = service.inner.metadata.load(&id).await? {
                if !record.allows(&caller.organization_id, &caller.principal_id) {
                    return Err(ConversationError::NotFound);
                }
                service
                    .inner
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
                        observed_at_ms: service.inner.clock.unix_milliseconds(),
                    })
                    .await
                    .map_err(|_| ConversationError::Audit)?;
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
                drop(creation_guard);
                service.resolve(&id, &caller).await?;
                return Ok(());
            }
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
            service
                .inner
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
                    observed_at_ms: service.inner.clock.unix_milliseconds(),
                })
                .await
                .map_err(|_| ConversationError::Audit)?;
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
    async fn resolve(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<Arc<LiveConversation>, ConversationError> {
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
                            let session_id =
                                SessionId::new(id.to_string()).expect("UUID session key");
                            let manager = SessionManager::open(
                                Some(session_id),
                                service.inner.storage.clone(),
                            )
                            .await
                            .map_err(|error| {
                                tracing::error!(conversation_id = %id, %error, "conversation storage opening failed");
                                let retryable = matches!(error, StorageError::Busy | StorageError::Io(_));
                                OpeningFailure { cause: ConversationError::Storage(error), _cleanup: None, retryable }
                            })?;
                            let agent = Agent::new(service.inner.provider.clone(), manager)
                                .await
                                .map_err(|error| {
                                tracing::error!(conversation_id = %id, %error, "conversation restoration failed");
                                let retryable = !error.needs_cleanup() && retryable_agent_open(error.cause());
                                OpeningFailure {
                                    cause: ConversationError::Agent(error.cause().clone()),
                                    _cleanup: Some(error),
                                    retryable,
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
                                let identity = service.inner.provider.identity();
                                projection.view.runtime = Some(ConversationRuntime {
                                    model: identity.model_id().into(), provider: identity.name().into(), workspace: workspace.clone(),
                                });
                            }
                            let live = Arc::new(LiveConversation {
                                agent,
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
                                Err(OpeningFailure { cause: ConversationError::Unavailable, _cleanup: None, retryable: false })
                            }
                        }
                    })
                    .await;
                owner.ready.notify_waiters();
                if result.as_ref().is_err_and(|failure| failure.retryable) {
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
                if result.as_ref().is_err_and(|failure| failure.retryable) {
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
            let live = service.resolve(&id, &caller).await?;
            // Reserve the full effective context budget consistently across retries.
            // ACP owns hidden context/tokenization; this is a conservative admission
            // reservation, not a claim about actual token usage or provider billing.
            let limits = live.agent.capabilities().limits();
            if service.inner.limits.reserved_output_tokens > limits.max_output()
                || service.inner.limits.reserved_output_tokens >= limits.max_context_window()
            {
                return Err(ConversationError::InvalidInput);
            }
            let request = ExecutionRequest {
                execution_id: execution,
                user_message: PromptText::new(text.clone())
                    .map_err(|_| ConversationError::InvalidInput)?,
                estimated_input_tokens: u64::from(
                    limits.max_context_window() - service.inner.limits.reserved_output_tokens,
                ),
                reserved_output_tokens: service.inner.limits.reserved_output_tokens,
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
