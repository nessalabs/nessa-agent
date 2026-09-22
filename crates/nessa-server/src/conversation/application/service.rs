use super::{
    projection::Projection,
    view::{
        ConversationAttachmentEvidenceFailure, ConversationAttachmentEvidenceFailureCode,
        ConversationCapabilities, ConversationDisposition, ConversationLifecycle,
        ConversationLifecyclePhase, ConversationMessageStatus, ConversationPendingMode,
        ConversationReorderOutcome, ConversationRuntime, ConversationStartupFailure,
        ConversationStartupFailureCode, ConversationView, SubmissionReceipt,
    },
    AttachmentRelease, AttachmentReleaseCause, ConversationAttachments, ConversationCreationAudit,
    ConversationError, ConversationFileLinkAudit, ConversationFileLinkAuditRecord,
    ConversationFileLinkCause, ConversationFileLinkState, ConversationRepository, RuntimeReadiness,
    SubmittedMessage,
};
use crate::agents::domain::AgentId;
use crate::conversation::domain::{Conversation, ConversationId};
use futures_util::{future::join_all, FutureExt};
use nessa_auth::application::ports::Clock;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::application::agent_execution::{
    agents::{
        AdmissionEvidence, AdmissionEvidenceFailure, Agent, AgentError, AgentInitializationError,
        AttachmentFailure, AttachmentFailureCode, AttachmentPhase, AttachmentRequest,
        QueueAdmission, QueueRemoval, QueueReorder, SteeringDelivery, SteeringEvidence,
    },
    executions::{ExecutionAudit, ExecutionRequest},
    permissions::{
        ActionContext, ApprovalAttribution, ApprovalBasis, PermissionAnswer,
        PermissionCancellationRequest, PermissionSelectionState,
    },
    providers::{AgentProvider, OperationCapabilities},
    sessions::{SessionManager, SessionSnapshot, SessionStorage},
};
use nessa_sdk::domain::agent_execution::{
    executions::{ExecutionId, InvocationStage},
    permissions::{
        CustomPermissionCancellationReason, PermissionCancellationReason, PermissionId,
        PermissionOptionId,
    },
    prompts::{ImageReference, LinkedFile, PromptText, UserMessage},
    sessions::SessionId,
};
use nessa_sdk::domain::common::value_objects::{ImageMediaType, Sha256Digest};
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
use tokio::{
    sync::{watch, Mutex, Notify, OnceCell, RwLock, RwLockReadGuard},
    task::JoinHandle,
};
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
    /// The attribution every command on this service records, validated once.
    ///
    /// `ActionContext` bounds the length and refuses a blank, and permits
    /// control characters; `Conversation::check_creator_context` does not. Both
    /// rules are asked here rather than only where a conversation is
    /// constructed, because every one of these commands writes this surface and
    /// action into a durable record — `audit_mapping` carries the action as
    /// `requestId` — and what gets written down must not rewrite a terminal or
    /// split a log line whichever command wrote it. The conversation entity
    /// still asks the same question of its own fields, through the same
    /// function, so there is one rule and not two that have to agree.
    fn actor(&self) -> Result<ActionContext, ConversationError> {
        Conversation::check_creator_context(&self.surface_id, &self.action_id)
            .map_err(|_| ConversationError::InvalidInput)?;
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
    /// The same durable audit sink injected into the provider and prepared Agent.
    pub execution_audit: Arc<dyn ExecutionAudit>,
    /// The output budget every submission to this agent reserves.
    pub reserved_output_tokens: u32,
    /// Joins this agent's one-time runtime preparation before its provider is
    /// opened on a request path. Per agent rather than one for the server,
    /// because the first-execution scan being paid for belongs to the runtime
    /// this provider launches; a conversation on one agent gains nothing by
    /// waiting for another agent's binary to be scanned. `None` means nothing
    /// prepares this runtime, so the first conversation on it pays the scan
    /// itself — an explicit no-op rather than a wrapper that returns at once.
    pub readiness: Option<Arc<dyn RuntimeReadiness>>,
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
enum SubmissionDelivery {
    Queued(QueueAdmission),
    Injected {
        target: ExecutionId,
        evidence: SteeringEvidence,
    },
}
struct LiveConversation {
    agent: Agent,
    /// The configured output reservation of the agent this conversation runs
    /// on, read once when it was opened.
    reserved_output_tokens: u32,
    projection: Mutex<Projection>,
    watched: Mutex<HashSet<String>>,
    attachment_owner: Mutex<Option<JoinHandle<()>>>,
}
impl LiveConversation {
    async fn join_attachment_owner(&self) {
        if let Some(owner) = self.attachment_owner.lock().await.take() {
            if let Err(error) = owner.await {
                tracing::error!(%error, "conversation attachment owner panicked");
            }
        }
    }
}
/// Preparation failure before a live conversation and attachment owner exist.
///
/// `holds` says whether preparation may still own storage or audit cleanup, or
/// whether a panic left ownership unknowable. A failure that acquired nothing
/// releases its capacity slot; retained ownership keeps the slot for cleanup.
/// Provider attachment failures occur after publication and stay on the SDK's
/// typed attachment lifecycle instead of becoming preparation failures here.
struct OpeningFailure {
    cause: ConversationError,
    cleanup: Option<AgentInitializationError>,
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
    creation_audit: Arc<dyn ConversationCreationAudit>,
    file_link_audit: Arc<dyn ConversationFileLinkAudit>,
    attachments: Option<Arc<dyn ConversationAttachments>>,
    clock: Arc<dyn Clock>,
    limits: ConversationLimits,
    conversations: Mutex<HashMap<ConversationId, Arc<Slot>>>,
    creation: Mutex<()>,
    retirement: OnceLock<ActionContext>,
    admission: RwLock<()>,
    // Counts stop passes, bumped the moment one is decided and before it waits
    // for anything. An opening parked waiting for the runtime has to learn about
    // teardown from something other than the lock it is blocking, and a count
    // rather than a flag means a transient stop can pass through without
    // leaving the gateway permanently fenced.
    stops: watch::Sender<u64>,
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
/// What a reader of the gateway log is told about a failed provider opening.
/// Kept apart from the tracing call so the wording has its own regression.
#[derive(Debug, PartialEq, Eq)]
struct OpeningFailureReport {
    /// Constant summary line.
    message: &'static str,
    /// Startup step that ran out of budget, when the failure names one.
    phase: Option<&'static str>,
    /// Whether that step was restoring saved context or opening a new one.
    /// None when the failure does not say which.
    session: Option<&'static str>,
}
// The opening gate covers both a first `session/new` and a `session/resume` of
// saved context. Only a startup deadline carries which of the two it was, so
// every other failure is reported as an opening failure rather than claimed to
// be a restoration: that claim sent readers looking for a snapshot that need
// not exist. The step and the context are separate facts — a restoration can
// run out of budget before `session/resume` is even sent — so both come from
// the error rather than one being inferred from the other.
fn opening_failure_report(error: &AgentError) -> OpeningFailureReport {
    match error {
        AgentError::StartupDeadline(step) => OpeningFailureReport {
            message: "agent startup exceeded its budget",
            phase: Some(step.as_str()),
            session: Some(step.context().as_str()),
        },
        _ => OpeningFailureReport {
            message: "conversation agent opening failed",
            phase: None,
            session: None,
        },
    }
}
fn report_opening_failure(id: &ConversationId, error: &AgentError) {
    let report = opening_failure_report(error);
    tracing::error!(
        conversation_id = %id,
        phase = report.phase,
        session = report.session,
        %error,
        "{}",
        report.message
    );
}
pub(super) fn attachment_failure_message(code: AttachmentFailureCode) -> &'static str {
    match code {
        AttachmentFailureCode::Audit => "Required attachment audit was not acknowledged.",
        AttachmentFailureCode::Provider => "The agent provider could not attach.",
        AttachmentFailureCode::Storage => "Attachment state could not be saved.",
        AttachmentFailureCode::Cleanup => "Attachment cleanup could not be confirmed.",
    }
}
fn lifecycle_view(agent: &Agent) -> ConversationLifecycle {
    let status = agent.attachment_status();
    let phase = match status.phase() {
        AttachmentPhase::Absent => ConversationLifecyclePhase::Absent,
        AttachmentPhase::Waiting | AttachmentPhase::Starting => {
            ConversationLifecyclePhase::Starting
        }
        AttachmentPhase::Attached => ConversationLifecyclePhase::Attached,
        AttachmentPhase::Failed(_) => ConversationLifecyclePhase::Failed,
    };
    let failure_view = |failure: &AttachmentFailure| {
        let code = failure.code();
        ConversationStartupFailure {
            code: match code {
                AttachmentFailureCode::Audit => ConversationStartupFailureCode::Audit,
                AttachmentFailureCode::Provider => ConversationStartupFailureCode::Provider,
                AttachmentFailureCode::Storage => ConversationStartupFailureCode::Storage,
                AttachmentFailureCode::Cleanup => ConversationStartupFailureCode::Cleanup,
            },
            message: attachment_failure_message(code).into(),
        }
    };
    ConversationLifecycle {
        phase,
        failure: status.failure().map(failure_view),
        evidence_failure: status.evidence_failure().map(|_| {
            ConversationAttachmentEvidenceFailure {
                code: ConversationAttachmentEvidenceFailureCode::Audit,
                message: attachment_failure_message(AttachmentFailureCode::Audit).into(),
            }
        }),
    }
}
fn admission_evidence_error(failure: &AdmissionEvidenceFailure) -> ConversationError {
    ConversationError::AdmissionEvidence {
        audit: failure.audit().cloned(),
        storage: failure.storage().cloned(),
    }
}
/// Every port the service calls, constructed by composition and substituted in
/// tests. Grouped rather than passed one by one, so that adding one does not
/// add another positional argument at every call site.
pub struct ConversationDependencies {
    /// Every configured agent, and the one a caller that names none gets.
    pub agents: ConversationAgents,
    pub storage: Arc<dyn SessionStorage>,
    pub metadata: Arc<dyn ConversationRepository>,
    pub creation_audit: Arc<dyn ConversationCreationAudit>,
    /// Records that a message pointed the agent at files on this machine.
    /// Not optional: a message may name a path on any gateway, so there is no
    /// configuration under which that grant goes unrecorded.
    pub file_link_audit: Arc<dyn ConversationFileLinkAudit>,
    /// `None` when this gateway keeps no uploads: every image is then refused.
    pub attachments: Option<Arc<dyn ConversationAttachments>>,
    pub clock: Arc<dyn Clock>,
}
impl ConversationService {
    /// Own every configured agent, and the one a caller gets by default.
    pub fn new(
        dependencies: ConversationDependencies,
        limits: ConversationLimits,
        workspace: Option<String>,
    ) -> Result<Self, ConversationError> {
        let ConversationDependencies {
            agents,
            storage,
            metadata,
            creation_audit,
            file_link_audit,
            attachments,
            clock,
        } = dependencies;
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
                file_link_audit,
                attachments,
                clock,
                limits,
                conversations: Mutex::new(HashMap::new()),
                creation: Mutex::new(()),
                retirement: OnceLock::new(),
                admission: RwLock::new(()),
                stops: watch::channel(0).0,
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
            // Asked of every creation, not only the ones that build a
            // conversation, and before the record is loaded so neither branch
            // can record a caller nobody validated. A reopen writes this
            // caller's surface and action into its own audit record, so the
            // same context has to be fit to record on both branches.
            let actor = caller.actor()?;
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
                        service.start_slot(id.clone(), slot.clone(), actor.clone());
                        slot
                    }
                }
            };
            drop(creation_guard);
            service.wait_for_slot(&id, slot).await?;
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
        let actor = caller.actor()?;
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
                self.start_slot(id.clone(), slot.clone(), actor);
                slot
            }
        };
        self.wait_for_slot(id, slot).await
    }
    fn start_slot(&self, id: ConversationId, slot: Arc<Slot>, actor: ActionContext) {
        let service = self.clone();
        let owner = slot.clone();
        if !slot.started.swap(true, Ordering::SeqCst) {
            tokio::spawn(async move {
                let result = owner
                    .value
                    .get_or_init(|| async {
                        let preparation = async {
                            let record = service
                                .inner
                                .metadata
                                .load(&id)
                                .await
                                .map_err(|cause| OpeningFailure {
                                    cause,
                                    cleanup: None,
                                    holds: false,
                                })?
                                .ok_or(OpeningFailure {
                                    cause: ConversationError::NotFound,
                                    cleanup: None,
                                    holds: false,
                                })?;
                            service.reconcile_creation_audit(&record).await.map_err(|cause| {
                                OpeningFailure {
                                    cause,
                                    cleanup: None,
                                    holds: false,
                                }
                            })?;
                            let configured = service
                                .inner
                                .agents
                                .get(record.agent())
                                .cloned()
                                .ok_or(OpeningFailure {
                                    cause: ConversationError::AgentNotConfigured,
                                    cleanup: None,
                                    holds: false,
                                })?;
                            let session_id = SessionId::new(id.to_string()).expect("UUID session key");
                            let manager = SessionManager::open(
                                Some(session_id),
                                service.inner.storage.clone(),
                            )
                            .await
                            .map_err(|error| {
                                tracing::error!(conversation_id = %id, %error, "conversation storage opening failed");
                                OpeningFailure {
                                    cause: ConversationError::Storage(error),
                                    cleanup: None,
                                    holds: false,
                                }
                            })?;
                            let agent = Agent::prepare(
                                configured.provider.clone(),
                                manager,
                                configured.execution_audit.clone(),
                            )
                            .await
                            .map_err(|error| {
                                let holds = error.needs_cleanup();
                                OpeningFailure {
                                    cause: ConversationError::Agent(error.cause().clone()),
                                    cleanup: Some(error),
                                    holds,
                                }
                            })?;
                            let authorization = agent
                                .authorize_attachment(AttachmentRequest::CallerRequested(actor))
                                .map_err(|error| OpeningFailure {
                                    cause: ConversationError::Agent(error),
                                    cleanup: None,
                                    holds: true,
                                })?;
                            let mut events = agent.subscribe();
                            let snapshot = agent.session_manager().snapshot().await;
                            let operation_capabilities = agent.operation_capabilities();
                            let capabilities = ConversationCapabilities {
                                queue: true,
                                steer: true,
                                resume: operation_capabilities.session_resume(),
                                permissions: agent.capabilities().features().tool_use(),
                                image_input: service
                                    .takes_images(&agent, operation_capabilities)
                                    == Some(true),
                                agent_features: operation_capabilities.into(),
                            };
                            let mut projection =
                                Projection::new(id.to_string(), capabilities, snapshot.as_ref());
                            projection.lifecycle(lifecycle_view(&agent));
                            if let Some(workspace) = &service.inner.workspace {
                                let identity = configured.provider.identity();
                                projection.view.runtime = Some(ConversationRuntime {
                                    model: identity.model_id().into(),
                                    provider: identity.name().into(),
                                    workspace: workspace.clone(),
                                });
                            }
                            let live = Arc::new(LiveConversation {
                                agent,
                                reserved_output_tokens: configured.reserved_output_tokens,
                                projection: Mutex::new(projection),
                                watched: Mutex::new(HashSet::new()),
                                attachment_owner: Mutex::new(None),
                            });
                            let weak = Arc::downgrade(&live);
                            tokio::spawn(async move {
                                loop {
                                    let event = events.next().await;
                                    let Some(live) = weak.upgrade() else {
                                        break;
                                    };
                                    match event {
                                        Ok(Some(event)) => live.projection.lock().await.event(&event),
                                        Err(_) => live.projection.lock().await.lagged(),
                                        Ok(None) => break,
                                    }
                                }
                            });
                            let attachment = live.clone();
                            let attachment_id = id.clone();
                            let attachment_service = service.clone();
                            let readiness = configured.readiness.clone();
                            let mut stops = service.inner.stops.subscribe();
                            let owner = tokio::spawn(async move {
                                if let Some(readiness) = readiness {
                                    let cancellation = authorization.cancellation();
                                    tokio::select! {
                                        () = readiness.wait() => {}
                                        () = cancellation.wait() => return,
                                        _ = stops.changed() => return,
                                    }
                                }
                                if attachment_service.inner.retirement.get().is_some() {
                                    return;
                                }
                                match attachment.agent.start_attachment(authorization) {
                                    Ok(wait) => {
                                        if let Err(error) = wait.wait().await {
                                            report_opening_failure(&attachment_id, &error);
                                        }
                                    }
                                    Err(error) if matches!(error, AgentError::Closed) => {}
                                    Err(error) => report_opening_failure(&attachment_id, &error),
                                }
                            });
                            *live.attachment_owner.lock().await = Some(owner);
                            Ok(live)
                        };
                        match AssertUnwindSafe(preparation).catch_unwind().await {
                            Ok(result) => result,
                            Err(payload) => {
                                mem::forget(payload);
                                Err(OpeningFailure {
                                    cause: ConversationError::Unavailable,
                                    cleanup: None,
                                    holds: true,
                                })
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
        let operation_capabilities = live.agent.operation_capabilities();
        // The last known answer stands while the agent is being restored, so the
        // view never says no to what a send at the same moment would admit.
        let image_input = self
            .takes_images(&live.agent, operation_capabilities)
            .unwrap_or(projection.view.capabilities.image_input);
        let queue = projection.view.capabilities.queue;
        let steer = projection.view.capabilities.steer;
        let permissions = projection.view.capabilities.permissions;
        projection.capabilities(ConversationCapabilities {
            queue,
            steer,
            resume: operation_capabilities.session_resume(),
            permissions,
            image_input,
            agent_features: operation_capabilities.into(),
        });
        projection.lifecycle(lifecycle_view(&live.agent));
        Ok(projection.read())
    }
    /// Admit one SDK-owned queued/steering input. Its completion outlives this call and its socket.
    pub async fn submit(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        execution_id: String,
        message: SubmittedMessage,
        mode: SubmissionMode,
    ) -> Result<SubmissionReceipt, ConversationError> {
        let service = self.clone();
        supervised(async move {
            let SubmittedMessage {
                text,
                images,
                files,
            } = message;
            let _admission = service.admit().await?;
            let actor = caller.actor()?;
            if text.len() > service.inner.limits.max_input_bytes {
                return Err(ConversationError::InvalidInput);
            }
            let execution =
                ExecutionId::new(&execution_id).map_err(|_| ConversationError::InvalidInput)?;
            // Blank text is no text. The message's own rules then decide whether
            // what remains is a message: some text, some images, or both.
            let prompt = (!text.trim().is_empty())
                .then(|| PromptText::new(text))
                .transpose()
                .map_err(|_| ConversationError::InvalidInput)?;
            let images = images
                .into_iter()
                .map(|image| {
                    ImageReference::new(
                        Sha256Digest::parse(&image.digest)
                            .map_err(|_| ConversationError::InvalidInput)?,
                        ImageMediaType::parse(&image.media_type)
                            .map_err(|_| ConversationError::InvalidInput)?,
                        image.size,
                    )
                    .map_err(|_| ConversationError::InvalidInput)
                })
                .collect::<Result<Vec<_>, _>>()?;
            // Nothing is opened, resolved, or followed here. Whether the file
            // is there is only true or false when the agent opens it, which is
            // later than this and behind a permission the reader answers, so a
            // check now would prove nothing and would refuse a file the reader
            // is about to create. What the gateway does check is the whole of
            // what it can: that the path can be said faithfully, which is the
            // domain's rule and which every path travels through.
            let files = files
                .into_iter()
                .map(|file| LinkedFile::new(file.path).map_err(|_| ConversationError::InvalidInput))
                .collect::<Result<Vec<_>, _>>()?;
            let message = UserMessage::new(prompt, images, files)
                .map_err(|_| ConversationError::InvalidInput)?;
            let live = service.resolve(&id, &caller).await?;
            // A submission the agent already has is the agent's to answer: the
            // same message recovers its original delivery and any other is a
            // conflict, whatever this conversation holds today. Its images were
            // checked when it was first accepted. Asking again would turn a
            // retry of a delivered turn into "not found" once its upload was
            // let go, where the same retry of a text turn succeeds.
            let known = live
                .agent
                .session_manager()
                .snapshot()
                .await
                .is_some_and(|snapshot| {
                    snapshot
                        .invocations
                        .iter()
                        .any(|record| record.request.execution_id == execution)
                });
            if !message.images().is_empty() && !known {
                // Refuse before acceptance what the agent would refuse at dispatch,
                // and any digest this conversation did not upload itself.
                let operation_capabilities = live.agent.operation_capabilities();
                let attachments = service
                    .inner
                    .attachments
                    .as_ref()
                    .filter(|_| {
                        service.takes_images(&live.agent, operation_capabilities) != Some(false)
                    })
                    .ok_or(ConversationError::ImagesUnsupported)?;
                for image in message.images() {
                    if !attachments
                        .holds(&caller.organization_id, &id, image)
                        .await?
                    {
                        return Err(ConversationError::AttachmentNotFound);
                    }
                }
            }
            // A message naming paths is recorded before it is admitted, and
            // what is recorded is the naming — not a read, and not even a
            // successful admission, both of which happen later and may not
            // happen at all. Before rather than after because the failure to
            // avoid is an agent holding a path with no record of who pointed it
            // there; a record for a submission that is then refused is the
            // harmless direction, and is still true.
            //
            // Only for a submission the agent has not already seen. A retry is
            // the SDK's to settle: the same message recovers its original
            // delivery, and any other message under the same identity is a
            // conflict it refuses. Recording here would write evidence naming
            // paths that the conflicting retry never delivered — and, because
            // the first submission of that identity may have named no files at
            // all and so written nothing to contradict, that evidence would
            // stand unopposed. That is the forgery this condition closes.
            if !message.files().is_empty() && !known {
                service
                    .inner
                    .file_link_audit
                    .record(ConversationFileLinkAuditRecord {
                        conversation_id: id.clone(),
                        organization_id: caller.organization_id.clone(),
                        execution_id: execution_id.clone(),
                        paths: message
                            .files()
                            .iter()
                            .map(|file| file.path().to_owned())
                            .collect(),
                        before: ConversationFileLinkState::NotNamed,
                        after: ConversationFileLinkState::Named,
                        cause: ConversationFileLinkCause::CallerSubmitted,
                        initiator_principal_id: caller.principal_id.clone(),
                        initiator_surface_id: caller.surface_id.clone(),
                        observed_at_ms: service.inner.clock.unix_milliseconds(),
                    })
                    .await
                    .map_err(|_| ConversationError::Audit)?;
            }
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
                user_message: message.clone(),
                estimated_input_tokens: u64::from(limits.max_context_window() - reserved),
                reserved_output_tokens: reserved,
            };
            let delivery = match mode {
                SubmissionMode::Queue => live
                    .agent
                    .enqueue(request, actor)
                    .await
                    .map(SubmissionDelivery::Queued),
                SubmissionMode::Steer if !live.agent.operation_capabilities().native_steering() => {
                    live.agent
                        .enqueue_steering(request, actor)
                        .await
                        .map(SubmissionDelivery::Queued)
                }
                SubmissionMode::Steer => {
                    live.agent
                        .steer(request, actor)
                        .await
                        .map(|delivery| match delivery {
                            SteeringDelivery::Queued(receipt) => {
                                SubmissionDelivery::Queued(receipt)
                            }
                            SteeringDelivery::Injected { target, evidence } => {
                                SubmissionDelivery::Injected { target, evidence }
                            }
                        })
                }
            };
            let pending_mode = if matches!(mode, SubmissionMode::Queue) {
                ConversationPendingMode::Queued
            } else {
                ConversationPendingMode::Steering
            };
            let delivery = match delivery {
                Ok(delivery) => delivery,
                Err(AgentError::SubmissionUnresolved) => {
                    live.projection
                        .lock()
                        .await
                        .admitted(&execution_id, &message, pending_mode);
                    let snapshot = live.agent.session_manager().snapshot().await;
                    live.projection
                        .lock()
                        .await
                        .settled(&execution_id, snapshot.as_ref());
                    return Err(ConversationError::Agent(AgentError::SubmissionUnresolved));
                }
                Err(error) => return Err(ConversationError::Agent(error)),
            };
            live.projection
                .lock()
                .await
                .admitted(&execution_id, &message, pending_mode);
            match delivery {
                SubmissionDelivery::Queued(receipt) => {
                    let evidence_error = match receipt.evidence() {
                        AdmissionEvidence::Acknowledged => None,
                        AdmissionEvidence::Failed(failure) => {
                            Some(admission_evidence_error(failure))
                        }
                    };
                    let settled = Self::watch_receipt(live, receipt).await;
                    if let Some(error) = evidence_error {
                        return Err(error);
                    }
                    Ok(SubmissionReceipt {
                        execution_id,
                        disposition: if settled {
                            ConversationDisposition::Settled
                        } else {
                            ConversationDisposition::Queued
                        },
                    })
                }
                SubmissionDelivery::Injected { target, evidence } => {
                    let snapshot = live.agent.session_manager().snapshot().await;
                    let mut projection = live.projection.lock().await;
                    projection.settled(&execution_id, snapshot.as_ref());
                    projection.injected(&execution_id, target.as_str());
                    drop(projection);
                    if let SteeringEvidence::Failed(failure) = &evidence {
                        return Err(admission_evidence_error(failure));
                    }
                    Ok(SubmissionReceipt {
                        execution_id,
                        disposition: ConversationDisposition::Injected,
                    })
                }
            }
        })
        .await
    }
    async fn watch_receipt(live: Arc<LiveConversation>, receipt: QueueAdmission) -> bool {
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
            // Whose conversation this is comes from the ownership record, before
            // anything else. Letting go of files must not depend on being able
            // to start a provider, so it cannot depend on `resolve` for this.
            let record = service
                .inner
                .metadata
                .load(&id)
                .await?
                .ok_or(ConversationError::NotFound)?;
            if !record.allows(&caller.organization_id, &caller.principal_id) {
                return Err(ConversationError::NotFound);
            }
            let (closed, may_release) = match service.resolve(&id, &caller).await {
                Ok(live) => {
                    let result = live.agent.close(actor).await;
                    if result.is_ok() {
                        live.join_attachment_owner().await;
                        service.release_live_slot(&id, &live).await;
                    }
                    // Refresh only terminal records. A concurrently accepted new turn
                    // retains its live observation state; there is no second closed flag.
                    let snapshot = live.agent.session_manager().snapshot().await;
                    live.projection.lock().await.settled_all(snapshot.as_ref());
                    // A closed agent runs nothing more, so nothing can still
                    // need the files. An agent that did not close keeps its
                    // saved invocations, and one of those may be a turn that
                    // names images and has not settled.
                    let may_release = result.is_ok() || !awaits_images(snapshot.as_ref());
                    (
                        result.map(|_| ()).map_err(ConversationError::Agent),
                        may_release,
                    )
                }
                // No room for another live conversation, a provider that will
                // not start, storage that will not open: the agent was not
                // closed, and that is reported. Nothing was read about what it
                // has queued either, so its files stay where they are and the
                // next close, which can open it, lets them go.
                Err(error) => (Err(error), false),
            };
            let released = match (&service.inner.attachments, may_release) {
                (Some(attachments), true) => {
                    attachments
                        .release(AttachmentRelease {
                            organization_id: caller.organization_id.clone(),
                            conversation_id: id.clone(),
                            cause: AttachmentReleaseCause::ConversationClosed,
                            initiator_principal_id: caller.principal_id.clone(),
                            initiator_surface_id: caller.surface_id.clone(),
                            correlation_id: caller.action_id.clone(),
                        })
                        .await
                }
                (Some(_), false) => {
                    tracing::warn!(
                        conversation_id = %id,
                        "a conversation that did not close keeps its uploads"
                    );
                    Ok(())
                }
                (None, _) => Ok(()),
            };
            match (closed, released) {
                (Ok(()), Ok(())) => Ok(()),
                (Ok(()), Err(release)) => {
                    Err(ConversationError::AttachmentRelease(Box::new(release)))
                }
                (Err(agent), Ok(())) => Err(agent),
                (Err(agent), Err(release)) => Err(ConversationError::CloseIncomplete {
                    agent: Box::new(agent),
                    release: Box::new(release),
                }),
            }
        })
        .await
    }
    /// Whether a message to this agent may carry images, or `None` while that
    /// is not known. Three facts must agree: this gateway keeps uploads, the
    /// selected model is offered images, which it is only with recorded image
    /// limits, and the connected agent agreed to receive them. An agent that
    /// advertises images in front of a model that is offered none takes none.
    ///
    /// The first two never change. The agent's answer is unknown while it is
    /// opening or being restored, and unknown is not no: a message sent then is
    /// admitted, and the SDK refuses it at dispatch, with the same meaning, if
    /// the restored agent turns out to take no images.
    fn takes_images(
        &self,
        agent: &Agent,
        operation_capabilities: OperationCapabilities,
    ) -> Option<bool> {
        if self.inner.attachments.is_none() || !agent.capabilities().features().input().image() {
            return Some(false);
        }
        operation_capabilities
            .negotiated()
            .then_some(operation_capabilities.image_input())
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
        // Same signal as retirement, for the same reason: an opening parked on
        // the runtime would otherwise hold this pass for its full per-owner
        // budget and then launch a provider nothing is left to close. Admission
        // stays open, so the count is bumped and nothing is fenced — a
        // conversation stopped this way can be opened again.
        self.inner.stops.send_modify(|count| *count += 1);
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
        // Before waiting for exclusive admission, not after: a request parked
        // waiting for the runtime holds a shared admission guard, and this is
        // what tells it to stop so that guard can be released. The fence itself
        // is `retirement`, set above; this only wakes the waiters.
        self.inner.stops.send_modify(|count| *count += 1);
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
            let attempt = async {
                loop {
                    let ready = slot.ready.notified();
                    tokio::pin!(ready);
                    ready.as_mut().enable();
                    if let Some(value) = slot.value.get() {
                        return match value {
                            Ok(live) => match live.agent.close(actor.clone()).await {
                                Ok(_) => {
                                    live.join_attachment_owner().await;
                                    self.release_slot(&id, &slot).await;
                                    Ok(())
                                }
                                Err(error) => Err(error),
                            },
                            Err(failed) => match &failed.cleanup {
                                Some(error) => match error.retry_cleanup().await {
                                    Ok(_) => {
                                        self.release_slot(&id, &slot).await;
                                        Ok(())
                                    }
                                    Err(error) => Err(error),
                                },
                                None if failed.holds => Err(AgentError::CleanupUncertain),
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

    async fn release_live_slot(&self, id: &ConversationId, live: &Arc<LiveConversation>) {
        let slot = self.inner.conversations.lock().await.get(id).cloned();
        if let Some(slot) = slot {
            if slot
                .value
                .get()
                .and_then(|value| value.as_ref().ok())
                .is_some_and(|known| Arc::ptr_eq(known, live))
            {
                self.release_slot(id, &slot).await;
            }
        }
    }

    async fn release_slot(&self, id: &ConversationId, slot: &Arc<Slot>) {
        let mut owners = self.inner.conversations.lock().await;
        if owners.get(id).is_some_and(|known| Arc::ptr_eq(known, slot)) {
            owners.remove(id);
        }
    }
}

/// Whether the saved session still has a turn that names images and has not
/// settled. Such a turn is dispatched when the agent next opens, and its images
/// are read then, so its conversation's files must outlive a close that did not
/// reach the agent. Without a snapshot nothing is known, which is not the same
/// as knowing there is nothing.
fn awaits_images(snapshot: Option<&SessionSnapshot>) -> bool {
    let Some(snapshot) = snapshot else {
        return true;
    };
    snapshot.invocations.iter().any(|record| {
        !record.request.user_message.images().is_empty()
            && record.result.is_none()
            && !record.scheduling.last().is_some_and(|event| {
                matches!(
                    event.stage,
                    InvocationStage::Cancelled
                        | InvocationStage::Injected
                        | InvocationStage::Settled
                )
            })
    })
}

#[cfg(test)]
#[path = "../../../tests/conversation/close_release.rs"]
mod close_release_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/retirement.rs"]
mod retirement_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/opening_diagnostics.rs"]
mod opening_diagnostics_tests;
