use super::{
    locks::ConversationLocks,
    projection::{clipped, Projection, MAX_TEXT},
    provider_sessions::{ProviderSessionErasers, ProviderSessionHandler},
    retries::{Claim, DeletionRetries, Waiting},
    view::{
        ConversationAttachmentEvidenceFailure, ConversationAttachmentEvidenceFailureCode,
        ConversationCapabilities, ConversationDisposition, ConversationLifecycle,
        ConversationLifecyclePhase, ConversationList, ConversationListEntry,
        ConversationMessageStatus, ConversationPendingMode, ConversationReorderOutcome,
        ConversationRuntime, ConversationStartupFailure, ConversationStartupFailureCode,
        ConversationView, SubmissionReceipt,
    },
    AttachmentRelease, AttachmentReleaseCause, ConversationAttachments, ConversationCreationAudit,
    ConversationDeletionAudit, ConversationDeletionAuditRecord, ConversationDeletionCause,
    ConversationError, ConversationFileLinkAudit, ConversationFileLinkAuditRecord,
    ConversationFileLinkCause, ConversationFileLinkState, ConversationOwnershipState,
    ConversationRepository, ConversationSummaries, DeletionFailures, RuntimeReadiness, StopFailure,
    SubmittedMessage,
};
use crate::agents::domain::AgentId;
use crate::conversation::domain::{
    Conversation, ConversationDeletion, ConversationId, ConversationSummary,
    ProviderSessionErasure, ProviderSessionLink,
};
use futures_util::{future::join_all, FutureExt};
use nessa_auth::application::ports::Clock;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::application::agent_execution::{
    agents::{
        AdmissionEvidence, AdmissionEvidenceFailure, Agent, AgentError, AttachmentFailure,
        AttachmentFailureCode, AttachmentPhase, AttachmentRequest, QueueAdmission, QueueRemoval,
        QueueReorder, SteeringDelivery, SteeringEvidence,
    },
    executions::{ExecutionAudit, ExecutionRequest, ExecutionUpdate},
    permissions::{
        ActionContext, ApprovalAttribution, ApprovalBasis, PermissionAnswer,
        PermissionCancellationRequest, PermissionSelectionState,
    },
    providers::{AgentProvider, OperationCapabilities},
    sessions::{
        SessionManager, SessionSnapshot, SessionStorage, SessionStorageLease, StorageError,
    },
};
use nessa_sdk::domain::agent_execution::{
    executions::{ExecutionId, ExecutionOutcome, InvocationStage, MessageKind},
    permissions::{
        CustomPermissionCancellationReason, PermissionCancellationReason, PermissionId,
        PermissionOptionId,
    },
    prompts::{ImageReference, LinkedFile, PromptText, UserMessage},
    sessions::{ExecutionSessionId, SessionId},
};
use nessa_sdk::domain::common::value_objects::{ImageMediaType, Sha256Digest};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    mem,
    panic::AssertUnwindSafe,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock, Weak,
    },
    time::Duration,
};
use tokio::{
    sync::{
        watch, Mutex, Notify, OnceCell, OwnedMutexGuard, OwnedSemaphorePermit, RwLock,
        RwLockReadGuard, Semaphore,
    },
    task::JoinHandle,
    time::Instant,
};
use uuid::Uuid;

/// Most conversations one list returns, applied after the caller's ownership
/// has been checked so that nobody else's conversations take a place in it.
pub const MAX_LISTED_CONVERSATIONS: usize = 500;

/// What a delete spends before it asks the agent about its own record, set by
/// composition from the one budgets table the client derives its wait from
/// (`protocol/defaults/agent-startup-budgets.json`, `deletion`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConversationDeletionBudgets {
    /// How long stopping a conversation's live agent may take before the stop
    /// is reported unconfirmed. Every stop of one owner takes it — a delete's,
    /// and each owner's in retirement — so there is one bound for stopping.
    /// It bounds how long a delete waits, not how long an agent may take to
    /// stop: a delete's stop still unconfirmed after it is carried on
    /// in-process until it is (`waiting_for`).
    pub stop: Duration,
    /// How long a delete asks again for the lease on a conversation's saved
    /// history before it reports the history still held. A stopped agent's
    /// last handles let go of their lease as the tasks holding them finish,
    /// which is soon after the stop but not at it.
    pub history_lease: Duration,
}

/// How many times, when the gateway starts, it tries to finish a deletion
/// that did not finish, and how long it waits before each try after the
/// first, times the number of tries so far. Bounded, so a deletion that
/// cannot finish — its agent will not answer, its history is damaged — is
/// reported and left for the next start rather than retried forever.
const DELETION_ATTEMPTS: u32 = 3;
const DELETION_RETRY_DELAY: Duration = Duration::from_secs(1);

/// Most agents asked at once, across every delete on this gateway, about their
/// own record of a session. Each ask may launch an agent process; a delete
/// that finds them all taken is left unfinished with
/// [`DeletionFailures::no_agent_slot`] and finished by a later try, rather than
/// waiting beyond its own bound
/// (`agents_asked_at_once_never_exceed_the_bound_and_the_rest_are_left_unfinished`).
const MAX_AGENTS_ASKED_AT_ONCE: usize = 2;

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

/// One asynchronous current-generation agent lookup.
pub type ConversationAgentFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<ConversationAgent>, ConversationError>> + Send + 'a>>;

/// Resolves the current provider generation for one cold conversation slot.
///
/// Implementations return an owned configuration so no registry guard crosses
/// storage or provider effects. Existing live slots never call this port.
pub trait ConversationAgentSource: Send + Sync {
    fn resolve(&self, agent: AgentId) -> ConversationAgentFuture<'_>;
}

#[derive(Clone)]
struct FixedConversationAgentSource {
    agents: HashMap<AgentId, ConversationAgent>,
}

impl ConversationAgentSource for FixedConversationAgentSource {
    fn resolve(&self, agent: AgentId) -> ConversationAgentFuture<'_> {
        let configured = self.agents.get(&agent).cloned();
        Box::pin(async move { Ok(configured) })
    }
}

/// Every agent this server can start, and the one a caller who names none gets.
///
/// One value rather than two parameters, because the two are only valid
/// together: a default that is not among the configured agents would accept a
/// creation this server can never open. Checked here, so that no code holding a
/// [`ConversationAgents`] has to consider the pairing invalid.
#[derive(Clone)]
pub struct ConversationAgents {
    configured: HashSet<AgentId>,
    default_agent: AgentId,
    source: Arc<dyn ConversationAgentSource>,
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
        let configured = agents.keys().copied().collect();
        Ok(Self {
            configured,
            default_agent,
            source: Arc::new(FixedConversationAgentSource { agents }),
        })
    }

    /// Construct a current-generation source with its pure configured set and default.
    pub fn from_source(
        configured: HashSet<AgentId>,
        default_agent: AgentId,
        source: Arc<dyn ConversationAgentSource>,
    ) -> Result<Self, ConversationError> {
        if !configured.contains(&default_agent) || configured.is_empty() {
            return Err(ConversationError::InvalidInput);
        }
        Ok(Self {
            configured,
            default_agent,
            source,
        })
    }

    fn select(&self, requested: Option<AgentId>) -> Result<AgentId, ConversationError> {
        let agent = requested.unwrap_or(self.default_agent);
        self.configured
            .contains(&agent)
            .then_some(agent)
            .ok_or(ConversationError::AgentNotConfigured)
    }

    async fn resolve(&self, agent: AgentId) -> Result<ConversationAgent, ConversationError> {
        let configured = self
            .source
            .resolve(agent)
            .await?
            .ok_or(ConversationError::AgentNotConfigured)?;
        if configured.reserved_output_tokens == 0 {
            return Err(ConversationError::InvalidInput);
        }
        Ok(configured)
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
    /// Whether what it launched may still be held. Stopping it cannot confirm
    /// otherwise, so a deletion stays unfinished while it does
    /// (`a_failed_opening_holding_what_it_launched_leaves_the_delete_unfinished`).
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
    deletion_audit: Arc<dyn ConversationDeletionAudit>,
    attachments: Option<Arc<dyn ConversationAttachments>>,
    summaries: Arc<dyn ConversationSummaries>,
    // One summary change at a time per conversation, so a turn's reply and
    // the next message cannot each read the same summary and have the later
    // write lose the other's change. Per conversation, so one conversation's
    // summary never waits behind another's
    // (`summary_writes_of_one_conversation_do_not_wait_for_another`). An
    // archive and a delete's erasure of the summary take it too, and each
    // writer asks whether the conversation was deleted while holding it, so a
    // summary is not written back after its erasure
    // (`a_late_reply_cannot_write_back_a_deleted_summary`).
    summary_writes: ConversationLocks,
    // One delete of a conversation at a time, from its fence to its last
    // erasure, so a second delete waits for the first and answers from the
    // tombstone it left (`concurrent_deletes_of_one_conversation_run_one_after_the_other`).
    deletions: ConversationLocks,
    /// Who is asked to delete an agent's own record of a provider session.
    provider_sessions: ProviderSessionErasers,
    deletion_budgets: ConversationDeletionBudgets,
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
    // Set once, when the service is retired, and never reset. What a delete
    // waits on — a stop, the history's lease, an agent being asked — ends on
    // this and not on `stops`: a desktop quit's pass that stops agents without
    // stopping admission is no reason to give up a person's delete
    // (`a_desktop_stop_leaves_a_delete_asking_its_agent_alone`,
    // `a_shutdown_ends_a_delete_waiting_to_stop_or_to_lease_and_it_is_left_unfinished`).
    retired: watch::Sender<bool>,
    /// Deletions this run left unfinished for a reason that can change while
    /// it runs, carried on by it.
    retries: Arc<DeletionRetries>,
    /// Permits for asking agents about their own record of a session.
    agents_asked: Arc<Semaphore>,
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
    /// Records who deleted a conversation, before any of it is erased.
    pub deletion_audit: Arc<dyn ConversationDeletionAudit>,
    /// `None` when this gateway keeps no uploads: every image is then refused.
    pub attachments: Option<Arc<dyn ConversationAttachments>>,
    /// What a list of conversations shows about each one.
    pub summaries: Arc<dyn ConversationSummaries>,
    /// Each agent's way of deleting its own record of a provider session,
    /// asked when a conversation on that agent is deleted.
    pub provider_sessions: ProviderSessionErasers,
    /// How long a delete waits to stop the agent and to lease the history.
    pub deletion_budgets: ConversationDeletionBudgets,
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
            deletion_audit,
            attachments,
            summaries,
            provider_sessions,
            deletion_budgets,
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
                deletion_audit,
                attachments,
                summaries,
                summary_writes: ConversationLocks::default(),
                deletions: ConversationLocks::default(),
                provider_sessions,
                deletion_budgets,
                clock,
                limits,
                conversations: Mutex::new(HashMap::new()),
                creation: Mutex::new(()),
                retirement: OnceLock::new(),
                admission: RwLock::new(()),
                stops: watch::channel(0).0,
                retired: watch::channel(false).0,
                agents_asked: Arc::new(Semaphore::new(MAX_AGENTS_ASKED_AT_ONCE)),
                retries: Arc::new(DeletionRetries::default()),
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
                // A deleted conversation is not created again under its
                // identity, whoever asks; only its owner is told why
                // (`a_deleted_conversation_refuses_every_command_on_it`).
                record.check_access(&caller.organization_id, &caller.principal_id)?;
                drop(creation_guard);
                // Acknowledge the original creation from its stored creator
                // evidence before attributing this reopen to its caller, and
                // before any provider opening. An unavailable audit sink
                // therefore refuses the reopen instead of leaving an opened
                // conversation with no record of who reopened it.
                service.reconcile_creation_audit(&record).await?;
                if caller.action_id != record.creation_action() {
                    // Asked again, and the reopen written, under the creation
                    // lock a delete writes its tombstone under: a delete that
                    // finished since the check above leaves no reopen
                    // recorded after its deletion
                    // (`a_reopen_racing_a_delete_is_not_recorded_after_the_deletion`).
                    let _creation = service.inner.creation.lock().await;
                    service
                        .inner
                        .metadata
                        .load(&id)
                        .await?
                        .ok_or(ConversationError::NotFound)?
                        .check_access(&caller.organization_id, &caller.principal_id)?;
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
            let requested = match agent {
                Some(RequestedAgent::Known(agent)) => Some(agent),
                Some(RequestedAgent::Unknown) => return Err(ConversationError::InvalidInput),
                None => None,
            };
            let agent = service.inner.agents.select(requested)?;
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
            record.check_access(&caller.organization_id, &caller.principal_id)?;
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
        record.check_access(&caller.organization_id, &caller.principal_id)?;
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
                                    holds: false,
                                })?
                                .ok_or(OpeningFailure {
                                    cause: ConversationError::NotFound,
                                    holds: false,
                                })?;
                            // Asked again here, after this slot was published:
                            // a delete writes its tombstone before it looks for
                            // slots to stop, so a slot it did not see reads the
                            // tombstone now and opens nothing
                            // (`a_create_racing_a_delete_cannot_republish_it`).
                            if record.deletion().is_some() {
                                return Err(OpeningFailure {
                                    cause: ConversationError::Deleted,
                                    holds: false,
                                });
                            }
                            service.reconcile_creation_audit(&record).await.map_err(|cause| {
                                OpeningFailure {
                                    cause,
                                    holds: false,
                                }
                            })?;
                            // A record naming an agent this build has no
                            // adapter for is read, listed, and deletable,
                            // and never opened on some other agent.
                            let agent = record.agent().ok_or(OpeningFailure {
                                cause: ConversationError::AgentUnsupported,
                                holds: false,
                            })?;
                            let mut stops = service.inner.stops.subscribe();
                            let configured = tokio::select! {
                                resolved = service.inner.agents.resolve(agent) => {
                                    resolved.map_err(|cause| OpeningFailure {
                                        cause,
                                        holds: false,
                                    })?
                                }
                                _ = stops.changed() => {
                                    return Err(OpeningFailure {
                                        cause: ConversationError::Unavailable,
                                        holds: false,
                                    });
                                }
                            };
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
                                OpeningFailure {
                                    cause: ConversationError::Agent(error.cause().clone()),
                                    holds: error.needs_cleanup(),
                                }
                            })?;
                            let authorization = agent
                                .authorize_attachment(AttachmentRequest::CallerRequested(actor))
                                .map_err(|error| OpeningFailure {
                                    cause: ConversationError::Agent(error),
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
                                    Err(AgentError::Closed) => {}
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
        let mut view = projection.read();
        drop(projection);
        view.title = self.title(&id).await;
        Ok(view)
    }
    /// The title a list shows for the conversation, which a read shows too:
    /// one rule, the summary's, whichever of the two is drawing it
    /// (`a_read_carries_the_title_the_list_shows`). A summary that cannot be
    /// read is logged and the view goes without a title.
    async fn title(&self, id: &ConversationId) -> Option<String> {
        match self.inner.summaries.load(id).await {
            Ok(summary) => summary
                .as_ref()
                .and_then(ConversationSummary::title)
                .map(|title| title.as_str().to_owned()),
            Err(error) => {
                tracing::warn!(
                    conversation_id = %id,
                    %error,
                    "conversation summary could not be read; the view has no title"
                );
                None
            }
        }
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
            // The agent has the message now, so the list says so — before the
            // turn's watcher can record its reply, which must land after it. A
            // retry of a message the agent already had says nothing new.
            if !known {
                service.summarize_message(&id, &message).await;
            }
            match delivery {
                SubmissionDelivery::Queued(receipt) => {
                    let evidence_error = match receipt.evidence() {
                        AdmissionEvidence::Acknowledged => None,
                        AdmissionEvidence::Failed(failure) => {
                            Some(admission_evidence_error(failure))
                        }
                    };
                    let settled = service.watch_receipt(&id, live, receipt, !known).await;
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
    /// `new_submission` is false for a retry of a submission the agent already
    /// had. One found already settled then says nothing new, so its reply is
    /// not recorded as if it had just been said.
    async fn watch_receipt(
        &self,
        conversation: &ConversationId,
        live: Arc<LiveConversation>,
        receipt: QueueAdmission,
        new_submission: bool,
    ) -> bool {
        let id = receipt.id().as_str().to_owned();
        let mut completion = Box::pin(receipt.wait());
        // Inspect the actual SDK receipt, not a second admission ledger. A retry
        // can already be settled; dropping this wait never cancels SDK work.
        if let Some(result) = completion.as_mut().now_or_never() {
            let snapshot = live.agent.session_manager().snapshot().await;
            let reply = completed_reply(snapshot.as_ref(), &id).filter(|_| new_submission);
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
            // Let go of the agent before waiting for the summary lock: a
            // summary is not a reason to keep a stopped agent's history
            // leased (`a_reply_waiting_to_be_summarized_does_not_keep_a_deleted_history_leased`).
            drop(live);
            if let Some(reply) = reply {
                self.summarize_reply(conversation, &reply).await;
            }
            return true;
        }
        if !live.watched.lock().await.insert(id.clone()) {
            return false;
        }
        let service = self.clone();
        let conversation = conversation.clone();
        tokio::spawn(async move {
            let result = completion.await;
            let snapshot = live.agent.session_manager().snapshot().await;
            let reply = completed_reply(snapshot.as_ref(), &id);
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
            // As above: the agent is let go of before the summary lock is
            // waited for, so a watcher cannot hold a stopped agent's history
            // leased past a delete's wait for it.
            drop(live);
            if let Some(reply) = reply {
                service.summarize_reply(&conversation, &reply).await;
            }
        });
        false
    }
    /// Every conversation this caller owns and has not deleted, most recently
    /// updated first, at most [`MAX_LISTED_CONVERSATIONS`] of them: those not
    /// archived, or, when `archived`, only those that are.
    ///
    /// Read from ownership records, stored summaries, and what is live on this
    /// gateway. Nothing is resolved: no provider is opened and no session
    /// storage is opened to draw a list
    /// (`listing_shows_only_the_callers_conversations_and_opens_nothing`).
    ///
    /// A conversation with no summary is left out: nothing was ever said in
    /// it — one opened only to hold an upload, say — and a list of what was
    /// said has nothing to show for it
    /// (`a_conversation_nothing_was_said_in_is_not_listed`). Only something
    /// said makes a summary — archiving makes none
    /// (`archiving_a_conversation_nothing_was_said_in_changes_nothing`) — so
    /// "has a summary" and "something was said" are the one rule. That includes
    /// conversations from before summaries were kept, which are not given a
    /// summary after the fact. One whose summary cannot be read was written,
    /// so something was said, and it is listed rather than failing the list:
    /// bare — no title or preview, its creation time — and in the default list
    /// only, reported as not archived. Whether it was archived cannot be read;
    /// showing it where a person looks first, over hiding it in the archived
    /// list or in neither, is the choice that never loses a conversation from
    /// view (`newest_summary_first_then_identity_and_an_unreadable_summary_lists_bare`). A conversation naming an
    /// agent this build cannot open is listed like any other: it is its
    /// owner's to see and to delete. The bound is applied after ownership and
    /// the archive filter (`the_archive_filter_applies_before_the_bound`), and
    /// the list says whether it left any out
    /// ([`ConversationList::complete`]): a list exactly at the bound with
    /// nothing behind it is complete
    /// (`a_list_says_whether_the_bound_left_any_out`), and no list — any
    /// caller's — is complete while any conversation record on the gateway
    /// cannot be read, until an operator repairs it
    /// (`an_unreadable_record_makes_every_list_incomplete`; the remedy is in
    /// [`ConversationList::complete`]).
    pub async fn list(
        &self,
        caller: ConversationCaller,
        archived: bool,
    ) -> Result<ConversationList, ConversationError> {
        let _admission = self.admit().await?;
        caller.actor()?;
        let records = self.inner.metadata.list().await?;
        // A record that cannot be read may be this caller's, so a list that
        // left one out cannot say it names them all
        // (`an_unreadable_record_makes_every_list_incomplete`).
        let unreadable = records.unreadable;
        let owned: Vec<Conversation> = records
            .conversations
            .into_iter()
            .filter(|record| {
                record
                    .check_access(&caller.organization_id, &caller.principal_id)
                    .is_ok()
            })
            .collect();
        // Only a conversation that finished opening has anything to say about
        // running; one still opening, or whose opening failed, is not waited on.
        // Answered first, and the live conversations let go of before any
        // summary is read: a list must not keep a stopped agent, or its
        // history's lease, while it waits on storage
        // (`a_list_waiting_on_summaries_does_not_keep_a_deleted_history_leased`).
        let live: HashMap<ConversationId, Arc<LiveConversation>> = {
            let owners = self.inner.conversations.lock().await;
            owned
                .iter()
                .filter_map(|record| {
                    let slot = owners.get(record.id())?;
                    let live = slot.value.get()?.as_ref().ok()?;
                    Some((record.id().clone(), live.clone()))
                })
                .collect()
        };
        let mut running = HashSet::new();
        for (id, live) in live {
            if turn_in_progress(live.agent.session_manager().snapshot().await) {
                running.insert(id);
            }
        }
        let mut entries = Vec::with_capacity(owned.len());
        for record in owned {
            let summary = match self.inner.summaries.load(record.id()).await {
                Ok(Some(summary)) => Some(summary),
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(
                        conversation_id = %record.id(),
                        %error,
                        "conversation summary could not be read; listed without it"
                    );
                    None
                }
            };
            let is_archived = summary.as_ref().is_some_and(ConversationSummary::archived);
            if is_archived != archived {
                continue;
            }
            let running = running.contains(record.id());
            entries.push(ConversationListEntry {
                conversation_id: record.id().to_string(),
                title: summary
                    .as_ref()
                    .and_then(ConversationSummary::title)
                    .map(|title| title.as_str().to_owned()),
                preview: summary
                    .as_ref()
                    .and_then(ConversationSummary::preview)
                    .map(|preview| preview.as_str().to_owned()),
                created_at_ms: record.creation_requested_at_ms(),
                updated_at_ms: summary.as_ref().map_or(
                    record.creation_requested_at_ms(),
                    ConversationSummary::updated_at_ms,
                ),
                running,
                archived: is_archived,
            });
        }
        entries.sort_by(|left, right| {
            right
                .updated_at_ms
                .cmp(&left.updated_at_ms)
                .then_with(|| left.conversation_id.cmp(&right.conversation_id))
        });
        let complete = unreadable == 0 && entries.len() <= MAX_LISTED_CONVERSATIONS;
        entries.truncate(MAX_LISTED_CONVERSATIONS);
        Ok(ConversationList {
            conversations: entries,
            complete,
        })
    }
    /// Record in the conversation's summary that `message` was accepted.
    async fn summarize_message(&self, id: &ConversationId, message: &UserMessage) {
        let at_ms = self.inner.clock.unix_milliseconds();
        let first_file = message.files().first().map(LinkedFile::name);
        self.change_summary(id, |previous| {
            Some(ConversationSummary::after_message(
                previous,
                message.text_str(),
                first_file,
                at_ms,
            ))
        })
        .await;
    }
    /// Record in the conversation's summary that the agent replied `reply`.
    async fn summarize_reply(&self, id: &ConversationId, reply: &str) {
        let at_ms = self.inner.clock.unix_milliseconds();
        self.change_summary(id, |previous| {
            ConversationSummary::after_reply(previous, reply, at_ms)
        })
        .await;
    }
    /// Replace a conversation's summary with what `change` makes of it.
    ///
    /// A summary is what a list shows, not what the conversation is, so
    /// failing to read or write one is logged and the command goes on: the
    /// list may be stale, the conversation is not
    /// (`a_summary_that_cannot_be_written_does_not_fail_the_message`). One that
    /// cannot be read is left alone rather than overwritten with a summary that
    /// would lose its title.
    ///
    /// A deleted conversation's summary is never written. The question is
    /// asked under the summary lock a delete erases it under, so a reply
    /// settling after the delete cannot put the file back
    /// (`a_late_reply_cannot_write_back_a_deleted_summary`).
    async fn change_summary(
        &self,
        id: &ConversationId,
        change: impl FnOnce(Option<&ConversationSummary>) -> Option<ConversationSummary>,
    ) {
        let _writes = self.inner.summary_writes.lock(id).await;
        match self.inner.metadata.load(id).await {
            Ok(Some(record)) if record.deletion().is_none() => {}
            Ok(_) => return,
            Err(error) => {
                tracing::warn!(
                    conversation_id = %id,
                    %error,
                    "conversation record could not be read; its summary was left as it was"
                );
                return;
            }
        }
        let previous = match self.inner.summaries.load(id).await {
            Ok(previous) => previous,
            Err(error) => {
                tracing::warn!(
                    conversation_id = %id,
                    %error,
                    "conversation summary could not be read; left as it was"
                );
                return;
            }
        };
        let Some(next) = change(previous.as_ref()) else {
            return;
        };
        if let Err(error) = self.inner.summaries.record(id, next).await {
            tracing::warn!(
                conversation_id = %id,
                %error,
                "conversation summary could not be written; the list shows the last one"
            );
        }
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
            record.check_access(&caller.organization_id, &caller.principal_id)?;
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
    /// Archive the conversation, or unarchive it: whether `list` shows it by
    /// default. Nothing is stopped or removed. Returns whether this changed
    /// it; `false` when it already was in that state, including a repeat of
    /// the same request, and for a conversation nothing was said in, which is
    /// in neither list and is left so.
    ///
    /// Unlike the rest of the summary this is a person's decision and not a
    /// projection, so failing to read or write it fails the command with the
    /// store's typed error instead of being logged and passed over
    /// (`an_archive_that_cannot_be_written_fails_visibly`).
    pub async fn archive(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        archived: bool,
    ) -> Result<bool, ConversationError> {
        let service = self.clone();
        supervised(async move {
            let _admission = service.admit().await?;
            caller.actor()?;
            // Access is decided under the summary lock, which a delete erases
            // the summary under: an archive either lands before that erasure
            // or finds the tombstone.
            let _writes = service.inner.summary_writes.lock(&id).await;
            let record = service
                .inner
                .metadata
                .load(&id)
                .await?
                .ok_or(ConversationError::NotFound)?;
            record.check_access(&caller.organization_id, &caller.principal_id)?;
            // Nothing said, nothing listed, nothing to archive: no summary is
            // made for it, so it stays out of both lists.
            let Some(previous) = service.inner.summaries.load(&id).await? else {
                return Ok(false);
            };
            if previous.archived() == archived {
                return Ok(false);
            }
            service
                .inner
                .summaries
                .record(&id, previous.after_archiving(archived))
                .await?;
            Ok(true)
        })
        .await
    }

    /// Delete the conversation for good. Returns whether this request is the
    /// one that deleted it: `true` for the deciding request and a repeat of
    /// it, `false` for any other — the domain's rule,
    /// [`ConversationDeletion::is_same_decision`].
    ///
    /// Authorize from the ownership record, as `close` does: a deleted
    /// conversation is still its owner's to finish deleting. Then, one delete
    /// of a conversation at a time, fence it: under the creation lock the
    /// repository writes a tombstone, and from then on each command on the
    /// identity, a creation of it included, is refused with
    /// [`ConversationError::Deleted`], and it is left out of both lists
    /// (`a_deleted_conversation_refuses_every_command_on_it`,
    /// `a_create_racing_a_delete_cannot_republish_it`). The rest is
    /// `finish_deletion`, which a repeat — or the gateway, when it
    /// starts ([`Self::finish_deletions`]) — carries on from wherever the
    /// tombstone says it got to, and which a finished deletion skips.
    pub async fn delete(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
    ) -> Result<bool, ConversationError> {
        let service = self.clone();
        supervised(async move {
            let _admission = service.admit().await?;
            let (record, applied, _deleting) = match service.fence(&id, &caller).await? {
                Fence::Written {
                    record,
                    applied,
                    deleting,
                } => (*record, applied, deleting),
                // Whoever erased it has already told the worker it is done.
                Fence::FinishedByAnother { applied } => return Ok(applied),
                Fence::LeftByAnother => {
                    return Err(ConversationError::DeletionIncomplete(Box::new(
                        DeletionFailures {
                            another_attempt: true,
                            ..DeletionFailures::default()
                        },
                    )))
                }
            };
            match service.finish_deletion(record).await {
                Ok(()) => {
                    // Nothing is left for the worker to carry
                    // (`a_person_s_delete_that_finishes_leaves_nothing_waiting`).
                    service.inner.retries.finished(&id);
                    Ok(applied)
                }
                Err(error) => {
                    service.carry_on_if_it_can_finish(&id, &error);
                    Err(error)
                }
            }
        })
        .await
    }

    /// Everything a delete does up to and including its own tombstone write:
    /// authorize from the ownership record, take the conversation's delete
    /// lock — waiting for another attempt and answering from the tombstone it
    /// leaves, if one holds it — and write the tombstone.
    ///
    /// Its failures are [`FenceFailure`], so none of them can be answered as one
    /// of the two codes that promise a deletion, whatever a substituted
    /// repository returns (`a_repository_s_error_before_the_fence_is_never_answered_as_a_deletion`).
    async fn fence(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<Fence, FenceFailure> {
        caller.actor().map_err(|_| FenceFailure::InvalidInput)?;
        let requested_at_ms = self.inner.clock.unix_milliseconds();
        let record = self
            .inner
            .metadata
            .load(id)
            .await
            .map_err(RepositoryFailure::from_load)?
            .ok_or(FenceFailure::NotFound)?;
        if !record.allows(&caller.organization_id, &caller.principal_id) {
            return Err(FenceFailure::NotFound);
        }
        // Never dated before the conversation it deletes: a clock stepped
        // back since its creation would otherwise propose a tombstone its
        // own record refuses, and every delete of it would be refused as
        // storage trouble. The stored tombstone is still held to that rule
        // when read back (`a_clock_stepped_back_since_creation_still_deletes`).
        let requested_at_ms = requested_at_ms.max(record.creation_requested_at_ms());
        let proposed = ConversationDeletion::new(
            caller.organization_id.clone(),
            caller.principal_id.clone(),
            caller.surface_id.clone(),
            caller.action_id.clone(),
            requested_at_ms,
        )
        .map_err(|_| FenceFailure::InvalidInput)?;
        // A delete that finds another attempt carrying this deletion waits
        // for it and answers from the tombstone it leaves, rather than
        // attempting again: one delete spends at most one attempt
        // (`a_delete_queued_behind_another_attempt_answers_from_its_tombstone`).
        // If that attempt never fenced it, this one does
        // (`a_delete_whose_predecessor_never_fenced_fences_it_itself`).
        let (deleting, waited) = match self.inner.deletions.try_lock(id) {
            Some(guard) => (guard, false),
            None => (self.inner.deletions.lock(id).await, true),
        };
        if waited {
            let current = self
                .inner
                .metadata
                .load(id)
                .await
                .map_err(RepositoryFailure::from_load)?
                .ok_or(FenceFailure::NotFound)?;
            if let Some(decided) = current.deletion() {
                let applied = decided.is_same_decision(&proposed);
                return Ok(if decided.erased() {
                    Fence::FinishedByAnother { applied }
                } else {
                    Fence::LeftByAnother
                });
            }
        }
        let record = {
            let _creation = self.inner.creation.lock().await;
            self.inner
                .metadata
                .record_deletion(id, proposed.clone())
                .await
                .map_err(RepositoryFailure::from_write)?
        };
        let decided = record.deletion().ok_or(RepositoryFailure::Metadata)?;
        let applied = decided.is_same_decision(&proposed);
        Ok(Fence::Written {
            record: Box::new(record),
            applied,
            deleting,
        })
    }

    /// Finish every deletion a tombstone says did not finish: one that
    /// stopped short of its end — an agent not confirmed stopped, a history
    /// still leased, an agent that could not be asked about its own record, a
    /// deletion record the sink did not take — or that the gateway stopped in
    /// the middle of. Composition starts this once the gateway is listening,
    /// in the background, so it never holds up startup
    /// (`an_unfinished_deletion_is_finished_and_recorded_when_the_gateway_starts`).
    ///
    /// Each gets at most `DELETION_ATTEMPTS` tries, `DELETION_RETRY_DELAY`
    /// apart and growing. What is still unfinished after its tries is
    /// returned with its last typed failure, and logged, and is tried again
    /// at the next start or by a repeated delete. Stops early, returning what
    /// it has, once the service is retired. Records that could not be read
    /// are counted too: a damaged tombstone is a deletion nothing here can
    /// see, let alone finish
    /// (`an_unreadable_record_makes_every_list_incomplete`).
    ///
    /// # Errors
    /// The repository's error when the records cannot be enumerated at all.
    pub async fn finish_deletions(&self) -> Result<DeletionsLeft, ConversationError> {
        let records = self.inner.metadata.list().await?;
        let (unreadable, orphaned_tombstones) = (records.unreadable, records.orphaned_tombstones);
        let unfinished: Vec<ConversationId> = records
            .conversations
            .into_iter()
            .filter(|record| record.deletion().is_some_and(|deletion| !deletion.erased()))
            .map(|record| record.id().clone())
            .collect();
        let mut left = Vec::new();
        for id in unfinished {
            let mut failure = None;
            for attempt in 0..DELETION_ATTEMPTS {
                if attempt > 0 {
                    tokio::time::sleep(DELETION_RETRY_DELAY * attempt).await;
                }
                match self.finish_deletion_in_background(&id).await {
                    // Finished, or being carried by whoever holds it now.
                    Ok(BackgroundTry::Finished | BackgroundTry::HeldElsewhere) => {
                        failure = None;
                        break;
                    }
                    // What this run can see change is carried on by the one
                    // rule for that, not retried here on a clock of its own
                    // (`a_startup_finish_that_finds_every_slot_taken_is_finished_once_one_frees`).
                    Err(error) if self.carry_on_if_it_can_finish(&id, &error) => {
                        failure = None;
                        break;
                    }
                    Err(error) => failure = Some(error),
                }
                if self.inner.retirement.get().is_some() {
                    break;
                }
            }
            if let Some(error) = failure {
                tracing::warn!(
                    conversation_id = %id,
                    %error,
                    "a deleted conversation's erasure is still unfinished; the next start tries again"
                );
                left.push((id, error));
            }
            if self.inner.retirement.get().is_some() {
                break;
            }
        }
        Ok(DeletionsLeft {
            unfinished: left,
            unreadable,
            orphaned_tombstones,
        })
    }

    /// Whether `error` left a deletion unfinished for a reason this run can
    /// see change — every agent slot taken, the conversation's agent still
    /// stopping, the history's lease still held ([`waiting_for`]) — and so one
    /// it carries on itself, rather than leaving it for the next
    /// start; `true` when it does. A cause that cannot change while it runs
    /// (an agent not built, a damaged history, an agent that refused) is left
    /// for the next start, and so is everything once retirement has begun
    /// (`a_delete_turned_away_for_want_of_an_agent_slot_is_finished_once_one_frees`,
    /// `a_deletion_left_for_a_held_lease_is_finished_once_it_is_let_go`,
    /// `nothing_is_carried_on_once_retirement_has_begun`).
    ///
    /// The one rule for what is carried on in-process, for every finisher:
    /// a person's delete, and the finish at start.
    fn carry_on_if_it_can_finish(&self, id: &ConversationId, error: &ConversationError) -> bool {
        let ConversationError::DeletionIncomplete(failures) = error else {
            return false;
        };
        let Some(waiting) = waiting_for(failures) else {
            return false;
        };
        if self.inner.retirement.get().is_some() {
            return false;
        }
        if self.inner.retries.start() {
            let worker = Arc::downgrade(&self.inner);
            let retries = self.inner.retries.clone();
            let retired = self.inner.retired.subscribe();
            drop(tokio::spawn(supervise_carrying_on(
                worker, retries, retired,
            )));
        }
        let generation = self.inner.retries.wait(id, waiting);
        if waiting == Waiting::ForRelease {
            due_again_after(&self.inner.retries, id, generation, 1);
        }
        true
    }

    /// One background try at finishing `id`'s deletion from its tombstone as
    /// it now stands, admitted like a delete.
    ///
    /// Of the repository's error reading the record, only what
    /// [`RepositoryFailure`] keeps is answered, so the one
    /// [`ConversationError::DeletionIncomplete`] a try can answer is
    /// `finish_deletion`'s own
    /// (`a_repository_error_in_a_background_try_is_never_a_reason_to_wait`).
    ///
    /// A background finisher never queues on the conversation's delete lock:
    /// whoever holds it is carrying this deletion, and a finisher waiting its
    /// turn would put a whole attempt between that one and a person's delete
    /// queued behind it (`a_person_s_delete_is_never_queued_behind_a_background_try`).
    async fn finish_deletion_in_background(
        &self,
        id: &ConversationId,
    ) -> Result<BackgroundTry, ConversationError> {
        let service = self.clone();
        let id = id.clone();
        supervised(async move {
            let _admission = service.admit().await?;
            let Some(_deleting) = service.inner.deletions.try_lock(&id) else {
                return Ok(BackgroundTry::HeldElsewhere);
            };
            let record = service
                .inner
                .metadata
                .load(&id)
                .await
                .map_err(RepositoryFailure::from_load)?
                .ok_or(ConversationError::NotFound)?;
            service.finish_deletion(record).await?;
            Ok(BackgroundTry::Finished)
        })
        .await
    }

    /// Carry a fenced deletion to its end, from wherever its tombstone says it
    /// got to, each step in the deciding request's name and each step's
    /// failure kept apart in [`ConversationError::DeletionIncomplete`]. The
    /// caller holds the conversation's deletion lock.
    ///
    /// 1. Stop the live agent as retirement stops one, waiting for a slot that
    ///    is still opening. Unconfirmed, nothing else is done.
    /// 2. Read the provider session from the saved history, under a lease
    ///    this deletion holds and validated as a restoration validates it,
    ///    and keep it in the tombstone.
    /// 3. Ask the conversation's agent, through [`ProviderSessionErasers`], to
    ///    delete its own record of that session, and keep what that settled
    ///    in the tombstone. After the stop, so the agent is not running it;
    ///    before the history is erased
    ///    (`the_agents_own_record_is_asked_to_go_after_the_stop_and_before_the_history`).
    ///    A failure leaves the deletion unfinished, to be asked again.
    /// 4. Record [`ConversationDeletionAuditRecord`] from the tombstone, once
    ///    steps 2 and 3 have settled what it says.
    /// 5. Erase: uploads are let go whatever became of the record; the
    ///    history (under the same lease) and the summary (under the summary
    ///    lock) only once the record is acknowledged.
    /// 6. Mark the tombstone erased, so nothing of it is attempted again.
    ///
    /// Audit stores are not touched
    /// (`deleting_on_the_local_stores_erases_what_it_owns_and_leaves_every_audit_record`).
    ///
    /// Every failure past the fence is a [`ConversationError::DeletionIncomplete`],
    /// since the conversation is deleted whatever else happened: one of a
    /// step's own, a panic in any port it calls
    /// ([`DeletionFailures::interrupted`]), or a tombstone that cannot be read
    /// as a deletion ([`DeletionFailures::tombstone`])
    /// (`a_port_that_panics_after_the_fence_leaves_the_deletion_unfinished`).
    async fn finish_deletion(&self, record: Conversation) -> Result<(), ConversationError> {
        let id = record.id().clone();
        match AssertUnwindSafe(self.finish_deletion_steps(record))
            .catch_unwind()
            .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(ConversationError::DeletionIncomplete(failures))) => {
                Err(ConversationError::DeletionIncomplete(failures))
            }
            Ok(Err(error)) => Err(ConversationError::DeletionIncomplete(Box::new(
                DeletionFailures {
                    tombstone: Some(error),
                    ..DeletionFailures::default()
                },
            ))),
            Err(payload) => {
                mem::forget(payload);
                tracing::error!(
                    conversation_id = %id,
                    "a step of a deleted conversation's erasure panicked; it is left unfinished"
                );
                Err(ConversationError::DeletionIncomplete(Box::new(
                    DeletionFailures {
                        interrupted: true,
                        ..DeletionFailures::default()
                    },
                )))
            }
        }
    }
    async fn finish_deletion_steps(&self, record: Conversation) -> Result<(), ConversationError> {
        let id = record.id().clone();
        let deletion = record
            .deletion()
            .cloned()
            .ok_or(ConversationError::Metadata)?;
        if deletion.erased() {
            return Ok(());
        }
        let actor = ActionContext::new(
            deletion.initiator().as_str(),
            deletion.surface(),
            deletion.request(),
        )
        .map_err(|_| ConversationError::Metadata)?;
        let slot = self.inner.conversations.lock().await.get(&id).cloned();
        if let Some(slot) = slot {
            // Retirement stops this agent itself, and must not wait here for
            // this delete to do it first. Answered as every step after the
            // tombstone is: the delete happened and did not finish
            // (`a_shutdown_ends_a_delete_waiting_to_stop_or_to_lease_and_it_is_left_unfinished`).
            let stopped = tokio::select! {
                stopped = self.stop_slot(&id, slot, &actor) => stopped,
                () = self.retired() => Err(StopFailure::Failed(AgentError::Closed)),
            };
            if let Err(stop) = stopped {
                return Err(ConversationError::DeletionIncomplete(Box::new(
                    DeletionFailures {
                        stop: Some(stop),
                        ..DeletionFailures::default()
                    },
                )));
            }
        }
        let mut failures = DeletionFailures::default();
        // `None` when it could not be taken; `Some(None)` when the history was
        // never opened, so there is nothing of it to read or erase.
        let leased = tokio::select! {
            leased = self.erasure_lease(&id) => leased.map_err(ConversationError::Storage),
            () = self.retired() => Err(ConversationError::Unavailable),
        };
        let lease = match leased {
            Ok(lease) => Some(lease),
            // Still held after the wait for it: the one place a deletion
            // learns its history is held elsewhere. `Busy` from reading or
            // erasing under its own lease is only storage failing
            // (`busy_under_the_deletion_s_own_lease_is_left_not_carried_on`).
            Err(ConversationError::Storage(StorageError::Busy)) => {
                failures.history_held = true;
                None
            }
            Err(error) => {
                failures.history = Some(error);
                None
            }
        };
        let mut record = record;
        if let (ProviderSessionLink::Unread, Some(lease)) = (deletion.provider_session(), &lease) {
            // What could not be read is the history's; what was read and
            // could not be kept is the tombstone's
            // (`a_history_read_that_cannot_be_kept_is_the_tombstone_s_failure`).
            match self.read_provider_session(&record, lease.as_deref()).await {
                Ok(read) => match self.inner.metadata.record_deletion(record.id(), read).await {
                    Ok(kept) => record = kept,
                    Err(error) => failures.tombstone = Some(error),
                },
                Err(error) => failures.history = Some(error),
            }
        }
        // The agent is asked only while this deletion holds the history's
        // lease: a lease held elsewhere may be a writer still running the
        // session, and deleting a running session is exactly what the agent
        // must not be asked to do
        // (`the_agent_is_not_asked_while_the_history_is_leased_elsewhere`).
        let record = match &lease {
            Some(_) => self.settle_provider_session(record, &mut failures).await,
            None => record,
        };
        let recorded = match Self::deletion_record(&record) {
            // Unsettled only when the history could not be read or the agent
            // could not be asked, which `failures` already says.
            None => false,
            Some(evidence) => match self.inner.deletion_audit.record(evidence).await {
                Ok(()) => true,
                Err(error) => {
                    failures.audit = Some(error);
                    false
                }
            },
        };
        // No agent is left to read an upload of a stopped, tombstoned
        // conversation, so its uploads go whatever became of the record.
        if let Some(attachments) = &self.inner.attachments {
            if let Err(error) = attachments
                .release(AttachmentRelease {
                    organization_id: record.organization().clone(),
                    conversation_id: id.clone(),
                    cause: AttachmentReleaseCause::ConversationDeleted,
                    initiator_principal_id: deletion.initiator().clone(),
                    initiator_surface_id: deletion.surface().to_owned(),
                    correlation_id: deletion.request().to_owned(),
                })
                .await
            {
                failures.attachments = Some(error);
            }
        }
        if recorded {
            if let Some(Some(lease)) = &lease {
                if let Err(error) = lease.erase().await {
                    failures.history = Some(ConversationError::Storage(error));
                }
            }
            let _writes = self.inner.summary_writes.lock(&id).await;
            if let Err(error) = self.inner.summaries.erase(&id).await {
                failures.summary = Some(error);
            }
        }
        drop(lease);
        if failures.is_empty() {
            // Finished only if the tombstone can say so: a deletion whose
            // provider session is unsettled has no finished form, so it can
            // never be reported done here, whatever `failures` says.
            match record
                .deletion()
                .and_then(ConversationDeletion::after_erasure)
            {
                Some(finished) => match self.inner.metadata.record_deletion(&id, finished).await {
                    Ok(_) => return Ok(()),
                    Err(error) => failures.tombstone = Some(error),
                },
                None => failures.tombstone = Some(ConversationError::Metadata),
            }
        }
        tracing::error!(
            conversation_id = %id,
            ?failures,
            "a deleted conversation's erasure did not finish"
        );
        Err(ConversationError::DeletionIncomplete(Box::new(failures)))
    }

    /// The delete's own lease on the conversation's saved history, asked for
    /// again while the stopped agent's last handles let go of theirs, for at
    /// most [`ConversationDeletionBudgets::history_lease`]. Still held after that is
    /// [`StorageError::Busy`]: somebody else is its writer, so it is not
    /// erased. `None` for a conversation whose history was never opened —
    /// there is nothing to erase, and asking creates nothing
    /// (`deleting_a_conversation_that_never_opened_creates_no_history_lock`).
    async fn erasure_lease(
        &self,
        id: &ConversationId,
    ) -> Result<Option<Box<dyn SessionStorageLease>>, StorageError> {
        let session = SessionId::new(id.to_string()).expect("UUID session key");
        let deadline = Instant::now() + self.inner.deletion_budgets.history_lease;
        loop {
            match self.inner.storage.open_existing(session.clone()).await {
                Err(StorageError::Busy) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                opened => return opened,
            }
        }
    }

    /// Read which provider session the saved history names, as the
    /// tombstone that records it; the caller keeps it, before anything erases
    /// that history.
    ///
    /// Read through [`SessionSnapshot::load_saved`], so a snapshot a custom
    /// adapter returns for some other session, or one whose relationships do
    /// not hold, is refused and nothing is kept from it
    /// (`a_history_that_names_another_session_is_refused_and_nothing_is_erased`).
    /// `lease` is `None` when the history was never opened: it named nothing.
    async fn read_provider_session(
        &self,
        record: &Conversation,
        lease: Option<&dyn SessionStorageLease>,
    ) -> Result<ConversationDeletion, ConversationError> {
        let decided = record.deletion().ok_or(ConversationError::Metadata)?;
        let read = match lease {
            Some(lease) => {
                let key = SessionId::new(record.id().to_string()).expect("UUID session key");
                match SessionSnapshot::load_saved(lease, &key)
                    .await
                    .map_err(ConversationError::Storage)?
                {
                    Some(snapshot) => {
                        decided.after_reading(snapshot.provider_context.recorded().cloned())
                    }
                    // A lease with no history behind it: opened, and then its
                    // journal moved aside or never written. Not "named none".
                    None => decided.after_losing_history(),
                }
            }
            None => decided.after_reading(None),
        };
        Ok(read)
    }

    /// Settle what becomes of the agent's own record of the provider session
    /// the tombstone read, if that is not settled yet, and keep it in the
    /// tombstone. A failure is kept in `failures` and leaves it unsettled.
    async fn settle_provider_session(
        &self,
        record: Conversation,
        failures: &mut DeletionFailures,
    ) -> Conversation {
        let Some(deletion) = record.deletion() else {
            return record;
        };
        let ProviderSessionLink::Recorded(session) = deletion.provider_session() else {
            return record;
        };
        if deletion.provider_erasure().is_some() {
            return record;
        }
        let erasure = match self.erase_provider_session(&record, session.clone()).await {
            Ok(erasure) => erasure,
            Err(AskFailure::NoFreeSlot) => {
                failures.no_agent_slot = true;
                return record;
            }
            Err(AskFailure::Failed(error)) => {
                failures.provider = Some(error);
                return record;
            }
        };
        let settled = deletion.after_provider_erasure(erasure);
        match self
            .inner
            .metadata
            .record_deletion(record.id(), settled)
            .await
        {
            Ok(settled) => settled,
            Err(error) => {
                failures.tombstone = Some(error);
                record
            }
        }
    }

    /// Ask the conversation's agent to delete its own record of `session`.
    /// Which agent is asked, and what an answer means, is
    /// [`ProviderSessionErasers`]'.
    ///
    /// Retiring this gateway ends the exchange: the ask is abandoned — the
    /// SDK still stops the connection's process and releases what it made —
    /// and answered [`ConversationError::Unavailable`], so the deletion stays
    /// unfinished for the next start, and a retirement is never held waiting
    /// for an agent (`a_shutdown_ends_an_agent_that_is_being_asked_and_a_later_start_finishes`).
    /// A desktop quit's stop pass does not end it.
    ///
    /// A slot is taken only for an agent that will be asked, and nothing is
    /// asked once retirement has begun. With every slot taken the answer is
    /// [`AskFailure::NoFreeSlot`] at once — its own outcome, which no eraser's
    /// error can be taken for
    /// (`an_eraser_answering_capacity_is_a_failure_not_a_slot_wait`); the
    /// deletion is then carried on by this gateway when a slot frees, which
    /// releasing a slot here signals.
    ///
    /// Not held behind the runtime's first-launch preparation: the exchange's
    /// own launch budget is the published one for that scan, and waiting for
    /// preparation first would add a wait with no bound of its own to the
    /// delete.
    async fn erase_provider_session(
        &self,
        record: &Conversation,
        session: ExecutionSessionId,
    ) -> Result<ProviderSessionErasure, AskFailure> {
        let eraser = match self
            .inner
            .provider_sessions
            .handler(record.agent())
            .map_err(AskFailure::Failed)?
        {
            ProviderSessionHandler::NoHandler => return Ok(ProviderSessionErasure::NoHandler),
            ProviderSessionHandler::Ask(eraser) => eraser,
        };
        // Nothing is launched once retirement has begun, and a slot is taken
        // only for an agent that will be asked
        // (`nothing_is_asked_once_retirement_has_begun`,
        // `a_conversation_nobody_can_ask_about_takes_no_agent_slot`).
        if self.inner.retirement.get().is_some() {
            return Err(AskFailure::Failed(ConversationError::Unavailable));
        }
        let Ok(permit) = self.inner.agents_asked.clone().try_acquire_owned() else {
            return Err(AskFailure::NoFreeSlot);
        };
        // Held for the ask; letting it go wakes deletions waiting for a slot,
        // however the ask ends — answered, abandoned, or panicking.
        let _slot = AgentSlot {
            permit: Some(permit),
            retries: self.inner.retries.clone(),
        };
        // An ask that panics is a failure of this attempt's ask, like any
        // other: the tombstone is written, so the delete happened and is
        // answered unfinished (`conversation_erasure_incomplete`), not as
        // the command failing (`a_slot_freed_by_an_ask_that_panicked_still_wakes_those_waiting`).
        let asked = async {
            match AssertUnwindSafe(eraser.erase(session)).catch_unwind().await {
                Ok(erased) => erased,
                Err(payload) => {
                    mem::forget(payload);
                    tracing::error!(
                        conversation_id = %record.id(),
                        "the agent's handler panicked while asked to delete its own session"
                    );
                    Err(ConversationError::Unavailable)
                }
            }
        };
        tokio::select! {
            biased;
            () = self.retired() => Err(AskFailure::Failed(ConversationError::Unavailable)),
            erased = asked => erased.map_err(AskFailure::Failed),
        }
    }

    /// The deletion record a tombstone stands for, or `None` while what it
    /// says about the provider session is not settled.
    fn deletion_record(record: &Conversation) -> Option<ConversationDeletionAuditRecord> {
        let deletion = record.deletion()?;
        let provider_erasure = deletion.provider_erasure()?;
        let provider_session_id = match deletion.provider_session() {
            ProviderSessionLink::Recorded(session) => Some(session.clone()),
            ProviderSessionLink::Unread
            | ProviderSessionLink::Absent
            | ProviderSessionLink::Unknown => None,
        };
        Some(ConversationDeletionAuditRecord {
            conversation_id: record.id().clone(),
            organization_id: record.organization().clone(),
            owner_id: record.owner().clone(),
            before: ConversationOwnershipState::Owned,
            after: ConversationOwnershipState::Deleted,
            cause: ConversationDeletionCause::CallerRequested,
            initiator_principal_id: deletion.initiator().clone(),
            initiator_surface_id: deletion.surface().to_owned(),
            correlation_id: deletion.request().to_owned(),
            provider_session_id,
            provider_erasure,
            requested_at_ms: deletion.requested_at_ms(),
        })
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
    /// Resolves once this service is retired, at once if it already is.
    async fn retired(&self) {
        let mut retired = self.inner.retired.subscribe();
        // The sender lives in `inner`, which `self` holds, so this cannot close.
        let _ = retired.wait_for(|retired| *retired).await;
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
        self.inner.retired.send_replace(true);
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
        let retired = match admission {
            Ok(_exclusive) => self.close_agents(actor).await,
            Err(_) => {
                let cleanup_error = self.close_agents(actor).await.err().map(Box::new);
                Err(ConversationError::RetirementAdmission { cleanup_error })
            }
        };
        // Deletions whose agents were being asked when retirement ended them
        // are still stopping those agents on tasks of their own; the runtime
        // that ends after this would cut them short and leave what they made
        // behind. Each binding bounds its own wait
        // (`retirement_waits_for_abandoned_agent_deletions_to_settle`).
        self.inner.provider_sessions.settled().await;
        retired
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
        // cannot consume another owner's cleanup opportunity.
        let attempts = slots.into_iter().map(|(id, slot)| async move {
            // Retirement reports every stop as the agent's error: over its
            // budget is a deadline there.
            self.stop_slot(&id, slot, actor).await.err().map(|stop| {
                let error = match stop {
                    StopFailure::OverBudget => AgentError::Deadline,
                    StopFailure::Failed(error) => error,
                };
                (id.to_string(), error)
            })
        });
        let failures: Vec<_> = join_all(attempts).await.into_iter().flatten().collect();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(ConversationError::Retirement(failures))
        }
    }

    /// Stop one owner, waiting for it to finish opening if it has not, and
    /// release its slot once it is confirmed closed. Bounded by
    /// [`ConversationDeletionBudgets::stop`]: running out of time is
    /// [`StopFailure::OverBudget`], its own outcome and never the stop's own
    /// error, which means only that cleanup is unconfirmed; the slot and
    /// supervised SDK work remain owned
    /// (`a_close_that_fails_with_a_deadline_of_its_own_is_not_the_stop_budget`).
    async fn stop_slot(
        &self,
        id: &ConversationId,
        slot: Arc<Slot>,
        actor: &ActionContext,
    ) -> Result<(), StopFailure> {
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
                                self.release_slot(id, &slot).await;
                                Ok(())
                            }
                            Err(error) => Err(error),
                        },
                        // One rule for a failed opening: while it may still hold
                        // what it launched, its stop cannot be confirmed.
                        Err(failed) if failed.holds => Err(AgentError::CleanupUncertain),
                        Err(_) => Ok(()),
                    };
                }
                ready.await;
            }
        };
        match tokio::time::timeout(self.inner.deletion_budgets.stop, attempt).await {
            Ok(result) => result.map_err(StopFailure::Failed),
            Err(_) => Err(StopFailure::OverBudget),
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

/// What a deletion left unfinished with `failures` is waiting for, when it is
/// something this run can see change: an agent slot; or a release — the
/// conversation's agent confirming a stop that ran past the stop budget, or
/// the history's lease let go. `None` for anything else, which only the next
/// start can finish. The one classification, for a person's delete, the
/// finish at start, and the worker alike.
///
/// A stop past its budget is carried on, not left for the next start: the
/// agent may take longer to stop than a delete waits — an ACP agent's close is
/// bounded by its own shutdown budgets, not by `stopMs` — and a delete the
/// person has already seen succeed must not wait for a restart to finish
/// (`a_delete_whose_agent_stops_after_the_stop_budget_is_finished_once_it_has`).
fn waiting_for(failures: &DeletionFailures) -> Option<Waiting> {
    if failures.no_agent_slot {
        return Some(Waiting::ForSlot);
    }
    let still_stopping = matches!(failures.stop, Some(StopFailure::OverBudget));
    (still_stopping || failures.history_held).then_some(Waiting::ForRelease)
}

/// An agent slot taken for one ask. Released on drop, however the ask ends,
/// and its release wakes deletions waiting for a slot
/// (`a_slot_freed_by_an_ask_that_panicked_still_wakes_those_waiting`).
struct AgentSlot {
    permit: Option<OwnedSemaphorePermit>,
    retries: Arc<DeletionRetries>,
}
impl Drop for AgentSlot {
    fn drop(&mut self) {
        // The slot is free before anybody is told it is.
        drop(self.permit.take());
        self.retries.slot_freed();
    }
}

/// What [`ConversationService::finish_deletions`] could not finish.
#[derive(Debug)]
pub struct DeletionsLeft {
    /// Each deletion still unfinished after its tries, with its last typed
    /// failure.
    pub unfinished: Vec<(ConversationId, ConversationError)>,
    /// How many conversation records could not be read at all, so whether
    /// any is an unfinished deletion cannot be known.
    pub unreadable: usize,
    /// How many tombstones have no conversation record beside them: deletions
    /// that can be neither read nor finished.
    pub orphaned_tombstones: usize,
}

/// Why the agent was not asked, or its ask failed.
enum AskFailure {
    /// Every agent slot on this gateway was taken.
    NoFreeSlot,
    /// Anything else: the agent's handler, or the ask itself, failed.
    Failed(ConversationError),
}

/// What a deletion keeps of a repository's failure where that failure is
/// its answer — the delete's fence, and a background try's read of the
/// record: only what [`ConversationRepository`]'s contract lets that call
/// answer. Every other error — one a substituted repository returns
/// included — is [`Self::Metadata`], so none there can be answered as a
/// deletion or taken for a reason to wait. Past the fence, a repository's
/// error is kept whole inside [`DeletionFailures`], and only its own
/// fields decide a wait ([`waiting_for`])
/// (`a_repository_s_error_before_the_fence_is_never_answered_as_a_deletion`,
/// `a_repository_error_in_a_background_try_is_never_a_reason_to_wait`).
#[derive(Debug)]
enum RepositoryFailure {
    /// No record to delete.
    NotFound,
    /// A record from before records named their agent.
    AgentUnsupported,
    /// The record could not be read or written:
    /// [`ConversationError::Metadata`].
    Metadata,
}
impl RepositoryFailure {
    /// What [`ConversationRepository::load`]'s failure can say: a record
    /// from before agents were named, or storage failing. A missing record is
    /// no error there, so a `NotFound` from it is storage failing too.
    fn from_load(error: ConversationError) -> Self {
        match error {
            ConversationError::AgentUnsupported => Self::AgentUnsupported,
            _ => Self::Metadata,
        }
    }
    /// What [`ConversationRepository::record_deletion`]'s failure can say:
    /// no record to delete, a record from before agents were named, or
    /// storage failing.
    fn from_write(error: ConversationError) -> Self {
        match error {
            ConversationError::NotFound => Self::NotFound,
            ConversationError::AgentUnsupported => Self::AgentUnsupported,
            _ => Self::Metadata,
        }
    }
}
impl From<RepositoryFailure> for ConversationError {
    fn from(failure: RepositoryFailure) -> Self {
        match failure {
            RepositoryFailure::NotFound => Self::NotFound,
            RepositoryFailure::AgentUnsupported => Self::AgentUnsupported,
            RepositoryFailure::Metadata => Self::Metadata,
        }
    }
}

/// Why a delete failed before its own tombstone write succeeded. Whether the
/// conversation is fenced is not known — a write that failed may still have
/// landed — so nothing here becomes `conversation_erasure_incomplete` or
/// `audit_unavailable`, the two answers that promise a deletion: of a
/// repository's error, only what [`RepositoryFailure`] keeps.
#[derive(Debug)]
enum FenceFailure {
    /// The request's own attribution or deletion could not be made.
    InvalidInput,
    /// No conversation of the caller's by that identity.
    NotFound,
    /// The repository failed.
    Repository(RepositoryFailure),
}
impl From<RepositoryFailure> for FenceFailure {
    fn from(failure: RepositoryFailure) -> Self {
        Self::Repository(failure)
    }
}
impl From<FenceFailure> for ConversationError {
    fn from(failure: FenceFailure) -> Self {
        match failure {
            FenceFailure::InvalidInput => Self::InvalidInput,
            FenceFailure::NotFound => Self::NotFound,
            FenceFailure::Repository(failure) => failure.into(),
        }
    }
}

/// How a delete stood once it was past [`ConversationService::fence`].
enum Fence {
    /// This delete wrote its tombstone, or carried the one there: the
    /// conversation as it now stands, whether this request is the deciding
    /// one, and the delete lock it holds to finish it.
    Written {
        record: Box<Conversation>,
        applied: bool,
        deleting: OwnedMutexGuard<()>,
    },
    /// Another attempt, which this delete waited for, finished it.
    FinishedByAnother { applied: bool },
    /// Another attempt, which this delete waited for, fenced it and did not
    /// finish.
    LeftByAnother,
}

/// What one background try came to, when it did not fail.
enum BackgroundTry {
    /// The deletion is finished.
    Finished,
    /// Another attempt holds it now, and is carrying it.
    HeldElsewhere,
}

/// Mark `id` due a try again after `tries` × [`DELETION_RETRY_DELAY`]: a
/// lease wait's own schedule, or one delay for a claim another attempt held.
fn due_again_after(
    retries: &Arc<DeletionRetries>,
    id: &ConversationId,
    generation: u64,
    tries: u32,
) {
    let retries = retries.clone();
    let id = id.clone();
    drop(tokio::spawn(async move {
        tokio::time::sleep(DELETION_RETRY_DELAY * tries).await;
        retries.due_again(&id, generation);
    }));
}

/// Run the worker, and if it ever ends by panicking, say so loudly and run
/// another after [`DELETION_RETRY_DELAY`] — with every waiting deletion due,
/// since a try it had claimed died with it — rather than leave deletions
/// piling up with nobody to carry them. Ends when the worker ends as told:
/// the service retired or gone.
async fn supervise_carrying_on(
    service: Weak<Inner>,
    retries: Arc<DeletionRetries>,
    retired: watch::Receiver<bool>,
) {
    let alive = service.clone();
    let worker_retries = retries.clone();
    let worker_retired = retired.clone();
    supervise(
        retries,
        retired,
        move || alive.strong_count() > 0,
        move || {
            carry_on(
                service.clone(),
                worker_retries.clone(),
                worker_retired.clone(),
            )
        },
    )
    .await;
}

/// [`supervise_carrying_on`]'s rule, apart from the worker it runs, so a
/// worker that panics can be tested
/// (`a_worker_that_panics_is_replaced_with_every_waiting_deletion_due`).
async fn supervise<Worker: Future<Output = ()>>(
    retries: Arc<DeletionRetries>,
    retired: watch::Receiver<bool>,
    alive: impl Fn() -> bool,
    mut worker: impl FnMut() -> Worker,
) {
    loop {
        let ended = AssertUnwindSafe(worker()).catch_unwind().await;
        let Err(payload) = ended else {
            break;
        };
        mem::forget(payload);
        tracing::error!(
            "the worker carrying on unfinished deletions panicked; another starts shortly"
        );
        tokio::time::sleep(DELETION_RETRY_DELAY).await;
        if *retired.borrow() || !alive() {
            break;
        }
        retries.all_due();
    }
    retries.stopped();
}

/// The one worker that carries on deletions this run left unfinished for a
/// reason it can see change, by the same path everything finishes a deletion
/// by. It wakes when something may be due and tries each deletion that is:
/// one waiting for a slot when a slot freed, spending no try; one waiting for
/// a release — a stop or a lease — only when its own timer fires — [`DELETION_ATTEMPTS`] tries,
/// [`DELETION_RETRY_DELAY`] apart and growing — never on another deletion's
/// wake (`a_lease_wait_is_not_spent_by_slots_freeing`). A deletion some other
/// attempt holds is left to it. Anything else is left for the next start.
/// Holds nothing of the service while it waits, and ends when the service is
/// retired or gone (`the_worker_carrying_deletions_on_ends_with_retirement`).
async fn carry_on(
    service: Weak<Inner>,
    retries: Arc<DeletionRetries>,
    mut retired: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            () = retries.woken() => {}
            _ = retired.wait_for(|retired| *retired) => return,
        }
        for claim in retries.claim_due() {
            if *retired.borrow() {
                return;
            }
            let Some(inner) = service.upgrade() else {
                return;
            };
            let answer = ConversationService { inner }
                .finish_deletion_in_background(&claim.id)
                .await;
            carried(&retries, &claim, answer);
        }
    }
}

/// Record what `claim`'s try came to.
fn carried(
    retries: &Arc<DeletionRetries>,
    claim: &Claim,
    answer: Result<BackgroundTry, ConversationError>,
) {
    let failures = match answer {
        Ok(BackgroundTry::Finished) => return retries.done(claim),
        // Somebody else holds it — an attempt carrying it, or only a delete
        // reading the tombstone to answer — so this one is due again once
        // the holder has had time to let go, without spending a try: a lease
        // wait on its own schedule, a slot wait after one delay. Nothing else
        // may wake a slot wait, since no ask need be running
        // (`a_claim_found_held_is_tried_again_once_its_holder_lets_go`).
        Ok(BackgroundTry::HeldElsewhere) => {
            let tries = match claim.waiting {
                Waiting::ForRelease => claim.release_tries + 1,
                Waiting::ForSlot => 1,
            };
            return due_again_after(retries, &claim.id, claim.generation, tries);
        }
        Err(ConversationError::DeletionIncomplete(failures)) => failures,
        Err(error) => {
            tracing::warn!(conversation_id = %claim.id, %error, "a deletion is left for the next start");
            return retries.done(claim);
        }
    };
    // Only a try its release timer ran spends one of the release's tries: a
    // slot wait that turns to waiting for a release starts that wait where it
    // stood (`a_slot_wait_that_turns_to_a_release_wait_spends_no_release_try`).
    let tries = match claim.waiting {
        Waiting::ForRelease => claim.release_tries + 1,
        Waiting::ForSlot => claim.release_tries,
    };
    match waiting_for(&failures) {
        Some(Waiting::ForSlot) => {
            return retries.requeue(claim, Waiting::ForSlot, claim.release_tries);
        }
        Some(Waiting::ForRelease) if tries < DELETION_ATTEMPTS => {
            retries.requeue(claim, Waiting::ForRelease, tries);
            return due_again_after(retries, &claim.id, claim.generation, tries + 1);
        }
        Some(Waiting::ForRelease) | None => {}
    }
    tracing::warn!(conversation_id = %claim.id, ?failures, "a deletion is left for the next start");
    retries.done(claim);
}

/// Whether the agent's own record has a turn it selected to run and that has
/// not settled. The session's committed snapshot, held in memory by the live
/// agent: nothing is read from storage to answer this.
fn turn_in_progress(snapshot: Option<SessionSnapshot>) -> bool {
    snapshot.is_some_and(|snapshot| {
        snapshot.invocations.iter().any(|record| {
            record.result.is_none()
                && record
                    .scheduling
                    .last()
                    .is_some_and(|event| event.stage == InvocationStage::Running)
        })
    })
}

/// What a turn that completed said last, as the saved session records it.
///
/// A reply that calls tools is several runs of text with the tool calls
/// between them, and a list previews the last run: what the agent said when it
/// finished, not what it said it was about to do. Only text is read, never a
/// thought, and no more of it than a preview could use.
fn completed_reply(snapshot: Option<&SessionSnapshot>, execution: &str) -> Option<String> {
    let record = snapshot?
        .invocations
        .iter()
        .find(|record| record.request.execution_id.as_str() == execution)?;
    if !matches!(record.result, Some(Ok(ExecutionOutcome::Completed))) {
        return None;
    }
    let mut reply = String::new();
    let mut after_tool = false;
    for event in &record.events {
        match event.update() {
            ExecutionUpdate::Message(chunk) if chunk.kind() == MessageKind::Text => {
                if after_tool {
                    reply.clear();
                    after_tool = false;
                }
                let room = MAX_TEXT.saturating_sub(reply.len());
                reply.push_str(&clipped(chunk.as_str(), room));
            }
            ExecutionUpdate::Tool(_) => after_tool = true,
            _ => {}
        }
    }
    (!reply.is_empty()).then_some(reply)
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

#[cfg(test)]
#[path = "../../../tests/conversation/deletion.rs"]
mod deletion_tests;
