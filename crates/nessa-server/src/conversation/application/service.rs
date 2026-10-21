mod agent_tools;
mod creation;
mod model_changes;
mod mutation;
mod opening_resources;
mod owned_threads;
use opening_resources::OpeningResources;
mod parent_stop;
mod thread_report_admission;
mod thread_reports;
mod workspace_windows;
use super::metadata_target::{load_conversation, validate_target};
use super::session_key::conversation_session;
use super::source_permits::SourcePermits;
use super::{
    app_reviews::{AppReviews, ReviewAnswer, ReviewAnswerer, Taken},
    drive_tools::{checked_source, AdmittedPolicies, DriveToolHook, DriveTools},
    environment::{
        confirmed_despite, issue, open_lease_on, AgentEnd, LeaseIssueError, LeaseRequest, LiveLease,
    },
    live_changes::LiveChanges,
    locks::ConversationLocks,
    mcp_apps::{McpAppError, McpAppInitiator, McpAppPorts, McpAppRef},
    placement::Environments,
    provider_sessions::{ProviderSessionErasers, ProviderSessionHandler},
    retries::{Claim, DeletionRetries, Waiting},
    AgentToolAudit, ArtifactBudget, ArtifactCollector, ArtifactOffer, AttachmentRelease,
    AttachmentReleaseCause, ConversationAccessPolicyEvaluator, ConversationAttachments,
    ConversationCreationAudit, ConversationDeletionAudit, ConversationDeletionAuditRecord,
    ConversationDeletionCause, ConversationError, ConversationFileLinkAudit,
    ConversationFileLinkAuditRecord, ConversationFileLinkCause, ConversationFileLinkState,
    ConversationListing, ConversationModeApplication, ConversationModeAudit,
    ConversationModeAuditPhase, ConversationModeRequest, ConversationModeRequestState,
    ConversationOwnershipState, ConversationRepository, ConversationSummaries, DeletionFailures,
    FenceCapture, FetchRequests, ListedConversation, ObservationCursor, ObservedConversations,
    OwnerAccess, PublishRequests, RevisionHold, RuntimeReadiness, ShareGrants, StopFailure,
    SubmittedMessage, UnfinishedDeletions,
};
use crate::conversation::domain::{
    Conversation, ConversationDeletion, ProviderSessionErasure, ProviderSessionLink, ToolSource,
};
use futures_util::{future::join_all, FutureExt};
use nessa_auth::application::ports::Clock;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{
    ConversationApprovalMode, ConversationId, ConversationModelId, ConversationSummary,
    ConversationTitle,
};
use nessa_protocol::conversation::{
    projection::{
        agent_question, agent_review, bound_view, clipped, question_entry, review_entry,
        CommittedCursor, Projection, MAX_TEXT,
    },
    tool_uis::{McpToolUis, NoMcpToolUis},
    view::{
        ConversationApprovalModeChangeStatus, ConversationApprovalModeChangeView,
        ConversationAttachmentEvidenceFailure, ConversationAttachmentEvidenceFailureCode,
        ConversationCapabilities, ConversationDisposition, ConversationLifecycle,
        ConversationLifecyclePhase, ConversationList, ConversationListEntry,
        ConversationObservation, ConversationObservationCursor, ConversationPermission,
        ConversationQuestion, ConversationQuestionEntry, ConversationReorderOutcome,
        ConversationReviewEntry, ConversationReviewKind, ConversationRuntime,
        ConversationSelectionView, ConversationStartupFailure, ConversationStartupFailureCode,
        ConversationView, SubmissionReceipt,
    },
};
use nessa_protocol::product_contract::generated::MAX_MCP_MESSAGE_BYTES;
use nessa_sdk::application::agent_execution::{
    agents::{
        AdmissionEvidence, AdmissionEvidenceFailure, Agent, AgentError, AttachmentFailure,
        AttachmentFailureCode, AttachmentPhase, AttachmentRequest, OwnedLifetime, QueueAdmission,
        QueueRemoval, QueueReorder, SteeringDelivery, SteeringEvidence,
    },
    commands::{CreationStorage, CreationTaskFault},
    executions::{
        AfterSaveCheck, AfterSaveRefusal, ExecutionAudit, ExecutionRequest, ExecutionUpdate,
        Submitter,
    },
    permissions::{
        is_remember_option, ActionContext, ApprovalAttribution, ApprovalBasis, PermissionAnswer,
        PermissionCaller, PermissionCancellationRequest, PermissionRules, QuestionAnswer,
    },
    providers::{
        AgentProvider, ApprovalMode as ProviderApprovalMode, CloseOutcome, OperationCapabilities,
        ProviderIdentity, ProviderOperationStage, SessionConfigurationKind,
    },
    sessions::{
        CommittedSession, LeaseRecordError, MessageCommitClock, SessionManager, SessionSnapshot,
        SessionStorage, SessionStorageLease, StorageError,
    },
};
use nessa_sdk::domain::agent_execution::{
    executions::{
        ExecutionId, ExecutionOutcome, GrantInitiator, Initiation, Initiator, InvocationStage,
        MessageKind,
    },
    leases::{AgentWork, LeaseEndCause, SandboxProfile, SandboxProfiles},
    permissions::{
        CustomPermissionCancellationReason, PermissionCancellationReason, PermissionId,
        PermissionOptionId,
    },
    prompts::{ImageReference, LinkedFile, McpAppSource, MessageSender, PromptText, UserMessage},
    questions::{QuestionChoice, QuestionId},
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
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, OnceLock, Weak,
    },
    time::Duration,
};
#[cfg(test)]
pub(crate) use thread_reports::report_text_for_test;
pub use thread_reports::{
    AwaitedTurn, PendingReport, ReportRegistration, ThreadReport, ThreadReportStore, ThreadTurn,
};
pub use workspace_windows::{PaneConversation, WindowReading};

use tokio::{
    sync::{
        oneshot, watch, Mutex, Notify, OnceCell, OwnedMutexGuard, OwnedRwLockWriteGuard,
        OwnedSemaphorePermit, RwLock, RwLockReadGuard, Semaphore,
    },
    task::JoinHandle,
    time::Instant,
};
use uuid::Uuid;

fn provider_approval_mode(mode: ConversationApprovalMode) -> ProviderApprovalMode {
    match mode {
        ConversationApprovalMode::Ask => ProviderApprovalMode::Ask,
        ConversationApprovalMode::Auto => ProviderApprovalMode::Auto,
        ConversationApprovalMode::Full => ProviderApprovalMode::Full,
    }
}

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
    /// How long a delete, or an opening, asks again for the lease on a
    /// conversation's saved history before it reports the history still held.
    /// A stopped agent's last handles let go of their lease as the tasks
    /// holding them finish, which is soon after the stop but not at it
    /// (`a_read_right_after_a_persons_close_is_not_busy`,
    /// `an_opening_stops_waiting_for_a_history_lease_at_its_bound`).
    pub history_lease: Duration,
}

/// How many times, when the gateway starts, it tries to finish a deletion
/// that did not finish, and how long it waits before each try after the
/// first, times the number of tries so far. Bounded, so a deletion that
/// cannot finish — its agent will not answer, its history is damaged — is
/// reported and left for the next start rather than retried forever.
const DELETION_ATTEMPTS: u32 = 3;
/// Most owners retirement starts stopping at once. Each stop's own writes take
/// a few milliseconds of the record store's one writer, so this many keep it
/// busy without leaving a stop waiting behind thousands of others' writes for
/// longer than its budget (`retirement_stops_a_bounded_number_of_owners_at_once`).
const RETIRING_AT_ONCE: usize = 64;
/// How long a stop keeps its place among [`RETIRING_AT_ONCE`]. One still
/// running after this is waiting on its agent, not the store, so it gives its
/// place to the next owner and carries on within its own budget: a crowd of
/// agents that will not stop cannot hold every other owner's stop back
/// (`agents_that_will_not_stop_do_not_hold_back_the_other_stops`).
const RETIRING_PLACE_HELD: Duration = Duration::from_secs(1);
const DELETION_RETRY_DELAY: Duration = Duration::from_secs(1);

/// One creation's place among the live conversations, held from its capacity
/// check until its slot is in the map (`Inner::creating`).
struct CreationPlace<'a>(&'a AtomicUsize);

impl<'a> CreationPlace<'a> {
    /// Take a place, or refuse when the live conversations and the other
    /// creations' places fill `max`. Called under the `conversations` lock.
    fn take(creating: &'a AtomicUsize, live: usize, max: usize) -> Result<Self, ConversationError> {
        if live + creating.load(Ordering::SeqCst) >= max {
            return Err(ConversationError::Capacity);
        }
        creating.fetch_add(1, Ordering::SeqCst);
        Ok(Self(creating))
    }
}

impl Drop for CreationPlace<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Most agents asked at once, across every delete on this gateway, about their
/// own record of a session. Each ask may launch an agent process; a delete
/// that finds them all taken is left unfinished with
/// [`DeletionFailures::no_agent_slot`] and finished by a later try, rather than
/// waiting beyond its own bound
/// (`agents_asked_at_once_never_exceed_the_bound_and_the_rest_are_left_unfinished`).
const MAX_AGENTS_ASKED_AT_ONCE: usize = 2;
/// How long a stopping gateway waits for every MCP App call to record its
/// last step.
const APP_CALLS_SETTLE: Duration = Duration::from_secs(10);

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
    pub(crate) fn actor(&self) -> Result<ActionContext, ConversationError> {
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

/// Optional choices for a new conversation. A repeated conversation identity
/// always reopens its stored selection and ignores these hints.
#[derive(Default)]
pub struct RequestedConversation {
    pub agent: Option<RequestedAgent>,
    pub model: Option<String>,
    pub approval_mode: Option<ConversationApprovalMode>,
    /// The SSH host it runs on, by the destination the configuration names
    /// it by; `None` runs it here. Kept as asked: a name the configuration
    /// does not have is refused where it would be used.
    pub environment: Option<String>,
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
    /// How long an idle agent nobody watches keeps running before it is
    /// parked (`service/parking.rs`); `None` never parks one.
    pub park_after: Option<Duration>,
}
impl ConversationLimits {
    /// Whether `text` is past what one message may say: the one bound a
    /// person's message (in `submit_as`) and an app's (refused early, on
    /// record, in `app_message`) are both held to.
    pub(super) fn past_input_bound(&self, text: &str) -> bool {
        text.len() > self.max_input_bytes
    }
}
/// A small service's limits. The gateway does not use these: it passes
/// `limits.liveConversations` from its configuration.
impl Default for ConversationLimits {
    fn default() -> Self {
        Self {
            max_conversations: 32,
            max_input_bytes: 8192,
            park_after: None,
        }
    }
}

/// Whether a message's text says nothing: blank text is no text. The one
/// reading `submit_as` takes of a person's message, and `app_message` of an
/// app's, which is text only and so refused early when it says nothing.
pub(super) fn blank_text(text: &str) -> bool {
    text.trim().is_empty()
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
    /// The sandbox profiles this agent's binding can set up. A lease asks for
    /// one of them; the environment admits it only if it can enforce it too.
    pub sandbox: SandboxProfiles,
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
    fn resolve_for<'a>(
        &'a self,
        agent: AgentId,
        model: &'a str,
        mode: ConversationApprovalMode,
    ) -> ConversationAgentFuture<'a>;
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
    fn resolve_for<'a>(
        &'a self,
        agent: AgentId,
        model: &'a str,
        mode: ConversationApprovalMode,
    ) -> ConversationAgentFuture<'a> {
        let configured = self.agents.get(&agent).cloned();
        Box::pin(async move {
            let Some(configured) = configured else {
                return Ok(None);
            };
            if configured.provider.identity().model_id() != model {
                return Err(ConversationError::ModelUnavailable);
            }
            if mode != ConversationApprovalMode::Ask {
                return Err(ConversationError::ApprovalModeUnavailable);
            }
            Ok(Some(configured))
        })
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
    /// Construct a valid target factory for a live model control. Ask is a
    /// provisional catalogue preset, not a native permission-setting command;
    /// the model acknowledgement owns the selected preset.
    async fn resolve_model(
        &self,
        agent: AgentId,
        model: &str,
    ) -> Result<ConversationAgent, ConversationError> {
        self.resolve_for(agent, model, ConversationApprovalMode::Ask)
            .await
    }
    async fn resolve_for(
        &self,
        agent: AgentId,
        model: &str,
        mode: ConversationApprovalMode,
    ) -> Result<ConversationAgent, ConversationError> {
        if !self.configured.contains(&agent) {
            return Err(ConversationError::AgentNotConfigured);
        }
        let configured = self
            .source
            .resolve_for(agent, model, mode)
            .await?
            .ok_or(ConversationError::AgentNotConfigured)?;
        if configured.reserved_output_tokens == 0 {
            return Err(ConversationError::InvalidInput);
        }
        Ok(configured)
    }
}
/// Whether a read opens a conversation that has no agent live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadOpening {
    /// Open it, as `conversation.read` does: the view then says what the
    /// agent can do.
    Open,
    /// Read only what is live: a conversation nothing has open is folded
    /// from its committed records, read-only, its lifecycle `absent`. A view
    /// subscription reads this way, so following a conversation never
    /// starts its agent again after a close or a stop (row S26).
    LiveOnly,
}
/// Why a command did not get going: the gateway retiring, which refuses
/// every command, or anything else, by its error. Answered
/// [`ConversationError::Unavailable`] when retiring; told apart for a
/// submission, whose app's message is then refused as M10
/// (`docs/design/mcp-app-calls.md`).
#[derive(Debug)]
enum Halt {
    Retired,
    Failed(ConversationError),
    /// Refused by an agent between a failed turn and its recovery, with
    /// nothing taken: offered again, the message finds the agent recovered,
    /// or stuck and the conversation opened again.
    Again(ConversationError),
}
impl From<ConversationError> for Halt {
    fn from(error: ConversationError) -> Self {
        Self::Failed(error)
    }
}
impl From<Halt> for ConversationError {
    fn from(halt: Halt) -> Self {
        match halt {
            Halt::Retired => Self::Unavailable,
            Halt::Failed(error) | Halt::Again(error) => error,
        }
    }
}
/// How a submission failed, by how far it got.
#[derive(Debug)]
pub(super) enum SubmitFailure {
    /// The gateway was retiring before the agent was asked to take it:
    /// nothing reached it.
    Retired,
    /// Refused before the agent was asked to take it: nothing reached it.
    NotAsked(ConversationError),
    /// The agent was asked to take it, and this is what came of it: refused,
    /// not settled, or taken without its evidence.
    Asked(ConversationError),
    /// Its own task failed; `asked`, whether the agent had been asked to
    /// take it by then.
    TaskFailed { asked: bool },
}
/// Whether the agent has a submission that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reach {
    /// It does not: refused, or failed, before it was asked, or refused by
    /// the agent.
    NotTaken,
    /// Nobody knows: the agent could not settle it, or the submission's own
    /// task failed once the agent was asked.
    Unknown,
    /// It does, and what failed is the evidence of its taking it.
    Taken,
}
impl Reach {
    /// How far a submission got that failed with `error` once the agent was
    /// asked to take it: read by [`SubmitFailure::reach`], and by the
    /// submission's own task as the agent answers, for what it took.
    fn of_asked(error: &ConversationError) -> Self {
        match error {
            ConversationError::Agent(AgentError::SubmissionUnresolved) => Self::Unknown,
            ConversationError::AdmissionEvidence { .. } => Self::Taken,
            // Saved with the turn, then refused or left unchecked before
            // injection. The contexts went with that turn and are lost with
            // it (C13), not dropped unsent.
            ConversationError::RevokedAfterSave | ConversationError::UnavailableAfterSave => {
                Self::Taken
            }
            _ => Self::NotTaken,
        }
    }
}
impl SubmitFailure {
    /// The one reading of how far a failed submission got, for the contexts
    /// it took and for an app's record of its message.
    pub(super) fn reach(&self) -> Reach {
        match self {
            Self::Retired | Self::NotAsked(_) | Self::TaskFailed { asked: false } => {
                Reach::NotTaken
            }
            Self::TaskFailed { asked: true } => Reach::Unknown,
            Self::Asked(error) => Reach::of_asked(error),
        }
    }
    /// The answer, whatever it got to: a task that failed, or the gateway
    /// retiring, is [`ConversationError::Unavailable`].
    fn into_error(self) -> ConversationError {
        match self {
            Self::NotAsked(error) | Self::Asked(error) => error,
            Self::Retired | Self::TaskFailed { .. } => ConversationError::Unavailable,
        }
    }
}
#[derive(Clone, Copy)]
pub enum SubmissionMode {
    Queue,
    Steer,
}
/// Who writes a submission: the person, or an app of theirs, admitted in an
/// opening of the conversation for one mount.
#[derive(Clone)]
pub(super) enum Writer {
    /// The person, with who their turn is saved as started by: the owner, or
    /// the share grant that admitted it. The flag is whether this membership
    /// may use a remember choice. A grant turn is not offered one.
    Person(Initiation, bool),
    /// Retain report admission through its supervised queue write.
    ThreadReport {
        _admission: Arc<OwnedMutexGuard<()>>,
    },
    App {
        sender: McpAppSource,
        apps: Arc<AppReviews>,
        epoch: u64,
        mount: McpAppRef,
    },
}
impl Writer {
    fn is_app(&self) -> bool {
        matches!(self, Self::App { .. })
    }
    /// Who the turn is saved as started by. An app's call asks for
    /// `conversation.write`, which no paired session is admitted for
    /// (`read_access::admit_request`), so an app's message is always from the
    /// owner's own surface.
    fn initiation(&self) -> Initiation {
        match self {
            Self::Person(initiation, _) => initiation.clone(),
            Self::App { .. } | Self::ThreadReport { .. } => Initiation::owner(),
        }
    }
    /// Whether this message may be offered a remember choice. An app writes
    /// on the owner's surface, so it may. A person's turn may only when the
    /// membership that sent it can use one, and only an owner turn is armed.
    fn offers_remember(&self) -> bool {
        match self {
            Self::Person(_, offered) => *offered,
            Self::App { .. } => true,
            Self::ThreadReport { .. } => false,
        }
    }
    fn sender(&self) -> MessageSender {
        match self {
            Self::Person(_, _) | Self::ThreadReport { .. } => MessageSender::Person,
            Self::App { sender, .. } => MessageSender::App(sender.clone()),
        }
    }
    /// Whether an app's message may still go into `live`: its mount not
    /// released, the opening it was admitted in not ended, and that opening
    /// the live one. The person's always may.
    fn still_admitted(&self, live: &LiveConversation) -> Result<(), ConversationError> {
        let Self::App { apps, epoch, .. } = self else {
            return Ok(());
        };
        let reopened = !Arc::ptr_eq(&live.app_reviews, apps) || live.app_epoch != *epoch;
        if reopened || self.opening_ended() {
            return Err(ConversationError::McpApp(McpAppError::Cancelled));
        }
        Ok(())
    }
    /// Whether an app's own apps say, now, that the opening its message was
    /// admitted in ended or its mount was released. Never the person's.
    fn opening_ended(&self) -> bool {
        match self {
            Self::Person(_, _) | Self::ThreadReport { .. } => false,
            Self::App {
                apps, epoch, mount, ..
            } => apps.admit(*epoch, mount).is_err(),
        }
    }
}
enum SubmissionDelivery {
    Queued(QueueAdmission),
    Injected { evidence: SteeringEvidence },
}
/// What one message's offers took from its apps, and the agent they were
/// offered to: kept from a refused offer for the next (`submit_admitted_as`).
/// The agent is named by a weak reference: holding it would keep an agent
/// that closed alive, its history leased, and the opening that replaces it
/// would wait on that lease.
type Carried = Arc<std::sync::Mutex<Option<(std::sync::Weak<LiveConversation>, Taken)>>>;
struct LiveConversation {
    agent: Agent,
    /// The lease its agent runs under. Every close of the agent goes through
    /// it, so the lease's end is recorded with the close.
    lease: Arc<LiveLease>,
    /// One acknowledged model binding and its configured admission reservation.
    model_budget: Mutex<ModelBudget>,
    projection: Mutex<Projection>,
    watched: Mutex<HashSet<String>>,
    attachment_owner: Mutex<AttachmentOwner>,
    /// Its MCP Apps' state — the conversation's, kept by the service — and
    /// the epoch of this opening of it.
    app_reviews: Arc<AppReviews>,
    app_epoch: u64,
    /// The publishes its agent asked for under its lease and not yet
    /// offered (`artifacts_publish`), shared with its lease's collector.
    publishes: Arc<PublishRequests>,
    /// The fetches its agent asked for under its lease (`artifacts_fetch`).
    fetches: Arc<FetchRequests>,
    /// Whose organization the conversation is, as its record says.
    organization_id: OrganizationId,
    /// Who names its agent's tools, when every call that agent makes outside
    /// its Ask-mode floor is asked about; `None` for OpenCode, which admits
    /// no Drive turn.
    tool_source: Option<ToolSource>,
    /// The tool policies of the Drive turns admitted here.
    admitted_policies: Arc<AdmittedPolicies>,
}
struct ModelBudget {
    identity: ProviderIdentity,
    reserved_output_tokens: u32,
}
enum AttachmentOwner {
    Running(JoinHandle<()>),
    Finished(Result<(), CreationTaskFault>),
}
impl LiveConversation {
    // Holding this same owner through await makes a dropped join waiter safe:
    // its original handle stays Running, and every completed waiter sees the
    // retained result. Creation readiness consumes it; ordinary cleanup logs
    // an unexpected host fault and still confirms release through SDK close.
    async fn join_attachment_owner(&self) -> Result<(), CreationTaskFault> {
        let mut owner = self.attachment_owner.lock().await;
        let result = match &mut *owner {
            AttachmentOwner::Running(handle) => {
                handle.await.map_err(CreationTaskFault::from_join_error)
            }
            AttachmentOwner::Finished(result) => return *result,
        };
        *owner = AttachmentOwner::Finished(result);
        if let Err(fault) = result {
            tracing::error!(?fault, "conversation attachment owner failed");
        }
        result
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
    /// Completion and actual environment cleanup owner. Successful preparation
    /// transfers the lease; failed preparation retains it until release answers.
    acquisition: Arc<OpeningResources>,
    value: OnceCell<Result<Arc<LiveConversation>, OpeningFailure>>,
    ready: Notify,
    started: AtomicBool,
    /// Set when a stop of this owner begins — on the desktop path once it
    /// has the submission lock, before it awaits anything else — and never
    /// cleared: a slot is not reused. From then on a submission hands its
    /// agent no new message (`an_owner_marked_as_stopping_is_handed_no_message`).
    /// A person's close needs none: it holds the submission lock until its
    /// slot is let go.
    stopping: AtomicBool,
    /// When the conversation last showed it was in use: opened or asked for
    /// by a read or a message, or seen busy, followed or with an app call
    /// waiting by the parking sweep. An agent is parked only once this is
    /// `ConversationLimits::park_after` old, or, past the bound on live
    /// agents, the one with the oldest is parked first
    /// (`service/parking.rs`). Unix milliseconds from the service's clock.
    touched: AtomicU64,
}
impl Slot {
    fn new(now_ms: u64) -> Self {
        Self {
            acquisition: Arc::new(OpeningResources::default()),
            value: OnceCell::new(),
            ready: Notify::new(),
            started: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            touched: AtomicU64::new(now_ms),
        }
    }
    /// The conversation is in use at `now_ms`.
    fn touch(&self, now_ms: u64) {
        self.touched.fetch_max(now_ms, Ordering::SeqCst);
    }
    /// How long before `now_ms` the conversation was last in use. A clock
    /// that went back reads as just now.
    fn untouched_for(&self, now_ms: u64) -> Duration {
        Duration::from_millis(now_ms.saturating_sub(self.touched.load(Ordering::SeqCst)))
    }
}
struct Inner {
    workspace: Option<String>,
    /// Every agent this server can start, and which of them a creation that
    /// names none is made on.
    agents: ConversationAgents,
    storage: Arc<dyn SessionStorage>,
    message_commit_clock: Arc<dyn MessageCommitClock>,
    metadata: Arc<dyn ConversationRepository>,
    creation_audit: Arc<dyn ConversationCreationAudit>,
    mode_audit: Arc<dyn ConversationModeAudit>,
    file_link_audit: Arc<dyn ConversationFileLinkAudit>,
    deletion_audit: Arc<dyn ConversationDeletionAudit>,
    attachments: Option<Arc<dyn ConversationAttachments>>,
    summaries: Arc<dyn ConversationSummaries>,
    listing: Arc<dyn ConversationListing>,
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
    mode_changes: ConversationLocks,
    // One create command of a conversation at a time, from its reopen check
    // to its saved readiness. Each request keeps its own command history, so
    // without it two requests for one new conversation could both find it
    // missing and the second open it as a conflict
    // (`two_requests_creating_one_conversation_both_open_it`).
    creations: ConversationLocks,
    /// Who is asked to delete an agent's own record of a provider session.
    provider_sessions: ProviderSessionErasers,
    deletion_budgets: ConversationDeletionBudgets,
    /// Where agents run; every opening is leased from it.
    environment: Environments,
    clock: Arc<dyn Clock>,
    /// Acknowledges a source turn's delegation decision before target work.
    agent_tool_audit: OnceLock<Arc<dyn AgentToolAudit>>,
    /// Physical source admission owners retain their slot after response loss.
    agent_tool_admissions: Arc<SourcePermits>,
    /// What each person's open desktop windows show, for their agents.
    workspace_windows: Arc<super::WorkspaceWindows>,
    /// Agent sends waiting to report their thread's finished turn.
    thread_reports: thread_reports::ThreadReports,
    ownership: OnceLock<owned_threads::Ownership>,
    thread_admission: Arc<Mutex<()>>,
    ownership_preparations: Mutex<HashMap<SessionId, owned_threads::PendingPreparation>>,

    /// Decides every command's admission to a conversation.
    access_policy: Arc<dyn ConversationAccessPolicyEvaluator>,
    limits: ConversationLimits,
    conversations: Mutex<HashMap<ConversationId, Arc<Slot>>>,
    /// Places held by creations between their capacity check and their slot:
    /// a creation writes its metadata and audit in between, so two creations
    /// of different conversations at the last place must not both pass the
    /// check and one then be refused after it was saved. Taken only under the
    /// `conversations` lock, and counted by every capacity check there.
    creating: AtomicUsize,
    /// One conversation's create, reopen and deletion decisions, in turn.
    /// Per conversation, so creating one never waits behind another's
    /// metadata and audit writes.
    creation: ConversationLocks,
    retirement: OnceLock<ActionContext>,
    admission: Arc<RwLock<()>>,
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
    /// Where a view finds the UI an MCP call's tool declared.
    tool_uis: Arc<dyn McpToolUis>,
    /// What an MCP App's calls go through; `None` with no MCP servers.
    mcp_apps: Option<McpAppPorts>,
    /// Each conversation's MCP Apps, kept for the conversation rather than
    /// its live agent: a release is kept across openings and before one.
    /// Let go of when the conversation is deleted.
    apps: std::sync::Mutex<HashMap<ConversationId, Arc<AppReviews>>>,
    /// Wakes view subscriptions when a live fact of a view changes; committed
    /// records wake them through the SDK's own watch.
    live_changes: Arc<LiveChanges>,
    /// MCP App calls running, across every caller: each holds one of these
    /// on its own task until that task ends, so a caller that goes and comes
    /// back cannot leave calls running past the bound.
    app_calls: Arc<Semaphore>,
    /// Held-file ranges `artifacts_fetch` reads at once, across every
    /// conversation: apart from the product socket's own reads, so fetches
    /// never lock a surface out of reading.
    fetch_reads: Arc<Semaphore>,
    /// Principal command receipts. Composition binds the shared record runtime.
    /// Absent storage refuses command admission rather than skipping the receipt.
    commands: Arc<OnceLock<Arc<dyn CreationStorage>>>,
    /// The share grants a waiting note's author is checked against before a
    /// turn takes it. Composition binds the gateway's table; without one, a
    /// grant's note is never checked and waits, and an owner's is taken.
    note_authors: Arc<OnceLock<Arc<dyn super::ShareGrants>>>,
    /// Where a conversation that may have unfinished turns is marked
    /// (`resumption`). Unbound, nothing is marked or resumed.
    unfinished_turns: OnceLock<Arc<dyn super::UnfinishedTurns>>,
    /// The marks this run has written or read, so a busy conversation's
    /// turns write one mark, not one each. Set under the conversation's
    /// submission lock, and cleared under it too, except where nothing can
    /// still run there: an agent closed for good, a conversation deleted.
    unfinished_marked: std::sync::Mutex<HashMap<ConversationId, resumption::Retry>>,
    /// How long a failure's retry waits (`resumption`), bound with the marks.
    failure_backoff: OnceLock<resumption::FailureBackoff>,
    /// Woken when a failure retry is scheduled, so the retry loop waits for
    /// the new earliest one.
    retry_scheduled: tokio::sync::Notify,
    /// What checks a Drive grant's tools. Composition binds it; until then
    /// no Drive turn is admitted (`drive_policy_unenforceable`).
    drive: Arc<OnceLock<DriveTools>>,
    /// Remembered permission rules. Unbound, no turn is offered a remember
    /// choice and no review is answered by a rule.
    permission_rules: Arc<OnceLock<Arc<dyn PermissionRules>>>,
    /// The deletion inventory, asked after a tombstone commits. Unbound, a
    /// delete still fences and finishes; the inventory is unchanged.
    fence_capture: Arc<OnceLock<Arc<dyn FenceCapture>>>,
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
/// Terminal settlement contains both polling faults and external panic payloads.
/// Destructors are external code too: retain the caught payload rather than
/// letting its destruction escape settlement
/// (`terminal_panic_payload_ownership_preserves_settlement`).
async fn contain_mode_terminal<T>(effect: impl Future<Output = T>) -> Result<T, ()> {
    match AssertUnwindSafe(effect).catch_unwind().await {
        Ok(value) => Ok(value),
        Err(payload) => {
            mem::forget(payload);
            Err(())
        }
    }
}
/// Correlate a retained pending envelope with the loaded conversation, before
/// it can authorize cleanup or be joined into a publication.
/// `terminal_pending_query_is_correlated_before_cleanup_and_publication`.
fn pending_for(
    id: &ConversationId,
    record: &Conversation,
    request: ConversationModeRequest,
) -> Result<ConversationModeRequest, ConversationError> {
    if request.conversation_id != *id
        || request.organization_id != *record.organization()
        || request.initiator_principal_id != *record.owner()
        || request.prior != record.approval_mode()
        || request.state != ConversationModeRequestState::Pending
    {
        return Err(ConversationError::ApprovalModeUncertain);
    }
    Ok(request)
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
/// Whether `live`'s agent runs at its conversation's approval mode. Attached,
/// the mode is the one applied to that attachment. With none yet, or one
/// still opening, it is the mode the agent opens its attachment at
/// (`Agent::approval_mode`): a message sent just after the conversation
/// opened again is checked against that, not refused for being early.
async fn mode_holds(live: &LiveConversation) -> bool {
    let _ = live.join_attachment_owner().await;
    if !matches!(
        live.agent.attachment_status().phase(),
        AttachmentPhase::Attached
            | AttachmentPhase::Absent
            | AttachmentPhase::Waiting
            | AttachmentPhase::Starting
    ) {
        return false;
    }
    let committed = live
        .projection
        .lock()
        .await
        .view()
        .selection
        .as_ref()
        .map(|selection| selection.approval_mode);
    committed.is_some_and(|mode| live.agent.approval_mode() == Some(provider_approval_mode(mode)))
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
    pub message_commit_clock: Arc<dyn MessageCommitClock>,
    pub metadata: Arc<dyn ConversationRepository>,
    pub creation_audit: Arc<dyn ConversationCreationAudit>,
    pub mode_audit: Arc<dyn ConversationModeAudit>,
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
    /// Which of one owner's conversations a list shows, read from ownership,
    /// tombstones and summaries without reading anybody else's.
    pub listing: Arc<dyn ConversationListing>,
    /// Each agent's way of deleting its own record of a provider session,
    /// asked when a conversation on that agent is deleted.
    pub provider_sessions: ProviderSessionErasers,
    /// How long a delete waits to stop the agent and to lease the history.
    pub deletion_budgets: ConversationDeletionBudgets,
    /// Where conversations' agents run, under a lease each: here, and the
    /// SSH hosts the configuration names.
    pub(crate) environment: Environments,
    pub clock: Arc<dyn Clock>,
    /// Decides who may locate, act on and read a conversation: every
    /// command's admission asks it ([`OwnerAccess`]).
    pub access_policy: Arc<dyn ConversationAccessPolicyEvaluator>,
}
/// What a host chose for one of the agent's questions.
///
/// Named rather than a tuple because each part means something different to
/// the agent: which question this answers, what was chosen from what it
/// offered, and any words the answerer typed instead.
#[derive(Clone, Debug)]
pub struct QuestionChoiceInput {
    /// The question this answers.
    pub key: String,
    /// Option values chosen, which that question must have offered.
    pub values: Vec<String>,
    /// Words of the answerer's own, only where the question invited them.
    pub own_words: Option<String>,
}

/// The reviews `agent`'s live invocation waits on, oldest first, as a view
/// lists them. The live owner says which wait and what each shows; a review
/// is listed once its request is on record in `snapshot` exactly as it waits
/// (the SDK admits an answer only to a recorded request), and not if the
/// owner cannot show it as what it is ([`agent_review`]). An unreadable
/// owner lists none.
fn agent_review_entries(
    agent: &Agent,
    snapshot: Option<&SessionSnapshot>,
) -> Vec<ConversationReviewEntry> {
    let reviews = agent.pending_reviews().unwrap_or_default();
    if reviews.is_empty() {
        return Vec::new();
    }
    let mut recorded = HashMap::new();
    for record in snapshot.map_or(&[][..], |snapshot| &snapshot.invocations[..]) {
        for event in &record.events {
            if let ExecutionUpdate::PermissionRequested {
                id,
                tool_id,
                input,
                options,
                ..
            } = event.update()
            {
                recorded.insert(
                    (&record.request.execution_id, id),
                    (tool_id, input, options),
                );
            }
        }
    }
    reviews
        .iter()
        .filter(|review| {
            recorded
                .get(&(&review.execution_id, &review.id))
                .is_some_and(|(tool, input, options)| {
                    **tool == review.tool_id
                        && **options == review.options
                        && input.name == review.content.tool_name()
                        && input.arguments_json == review.content.arguments_json()
                })
        })
        .filter(|review| agent_review(review).is_some())
        .map(|review| {
            review_entry(
                &review.content.key(),
                ConversationReviewKind::Harness,
                review.content.tool_name(),
            )
        })
        .collect()
}

/// The questions `agent`'s live invocation waits on, oldest first, as a view
/// lists them. The live owner says which wait and what each asks; one is
/// listed once its ask is on record in `snapshot` exactly as it waits (the
/// SDK admits an answer only to a recorded ask). An unreadable owner lists
/// none.
fn agent_question_entries(
    agent: &Agent,
    snapshot: Option<&SessionSnapshot>,
) -> Vec<ConversationQuestionEntry> {
    let questions = agent.pending_questions().unwrap_or_default();
    if questions.is_empty() {
        return Vec::new();
    }
    let mut recorded = HashMap::new();
    for record in snapshot.map_or(&[][..], |snapshot| &snapshot.invocations[..]) {
        for event in &record.events {
            if let ExecutionUpdate::QuestionAsked { id, question } = event.update() {
                recorded.insert((&record.request.execution_id, id), question);
            }
        }
    }
    questions
        .iter()
        .filter(|pending| {
            recorded
                .get(&(&pending.execution_id, &pending.id))
                .is_some_and(|asked| **asked == *pending.question)
        })
        .map(|pending| question_entry(&pending.key(), pending.question.message()))
        .collect()
}

/// The range of `shown`'s UTF-8 JSON from `offset`, at most `max_bytes`,
/// and its whole length. `InvalidInput` for an offset past its end.
fn range_of(
    shown: &impl serde::Serialize,
    offset: u64,
    max_bytes: usize,
) -> Result<(Vec<u8>, u64), ConversationError> {
    let bytes = serde_json::to_vec(shown).map_err(|_| ConversationError::InvalidInput)?;
    let total = bytes.len() as u64;
    if offset > total || (offset == total && total != 0) {
        return Err(ConversationError::InvalidInput);
    }
    let start = offset as usize;
    let end = start.saturating_add(max_bytes).min(bytes.len());
    Ok((bytes[start..end].to_vec(), total))
}

/// The export cut's hold on admission. Dropping it lets the next command in.
pub(crate) struct ExportAdmission {
    _guard: OwnedRwLockWriteGuard<()>,
    retired: watch::Receiver<bool>,
}

impl ExportAdmission {
    /// Whether [`ConversationService::retire`] signaled while this hold
    /// still owns admission. Retirement is waiting on the same lock.
    pub(crate) fn retirement_signaled(&self) -> bool {
        *self.retired.borrow()
    }
}

/// Why an export hold was not granted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExportHoldFailure {
    /// The gateway is retiring.
    Retired,
}

impl ConversationService {
    /// Canonical approval selection when creation supplies no override.
    pub fn default_creation_approval_mode() -> ConversationApprovalMode {
        ConversationApprovalMode::Ask
    }

    /// Own every configured agent, and the one a caller gets by default. No
    /// MCP call shows a UI: see [`Self::with_tool_uis`].
    pub fn new(
        dependencies: ConversationDependencies,
        limits: ConversationLimits,
        workspace: Option<String>,
    ) -> Result<Self, ConversationError> {
        Self::with_tool_uis(dependencies, limits, workspace, Arc::new(NoMcpToolUis))
    }
    /// As [`Self::new`], with each MCP call's UI looked up in `tool_uis` when
    /// a view is read. No MCP App is admitted: see [`Self::with_mcp_apps`].
    pub fn with_tool_uis(
        dependencies: ConversationDependencies,
        limits: ConversationLimits,
        workspace: Option<String>,
        tool_uis: Arc<dyn McpToolUis>,
    ) -> Result<Self, ConversationError> {
        Self::build(dependencies, limits, workspace, tool_uis, None)
    }
    /// As [`Self::with_tool_uis`], with MCP Apps' calls answered through
    /// `apps`.
    pub fn with_mcp_apps(
        dependencies: ConversationDependencies,
        limits: ConversationLimits,
        workspace: Option<String>,
        tool_uis: Arc<dyn McpToolUis>,
        apps: McpAppPorts,
    ) -> Result<Self, ConversationError> {
        Self::build(dependencies, limits, workspace, tool_uis, Some(apps))
    }
    fn build(
        dependencies: ConversationDependencies,
        limits: ConversationLimits,
        workspace: Option<String>,
        tool_uis: Arc<dyn McpToolUis>,
        mcp_apps: Option<McpAppPorts>,
    ) -> Result<Self, ConversationError> {
        let ConversationDependencies {
            agents,
            storage,
            message_commit_clock,
            metadata,
            creation_audit,
            mode_audit,
            file_link_audit,
            deletion_audit,
            attachments,
            summaries,
            listing,
            provider_sessions,
            deletion_budgets,
            environment,
            clock,
            access_policy,
        } = dependencies;
        // The input bound is never larger than what the protocol carries of
        // a message, the person's or an app's (`MAX_MCP_MESSAGE_BYTES`, held
        // equal to `conversation.send`'s by the generator).
        if workspace.as_ref().is_some_and(|value| value.len() > 4096)
            || limits.max_conversations == 0
            || limits.max_input_bytes == 0
            || limits.max_input_bytes > MAX_MCP_MESSAGE_BYTES
        {
            return Err(ConversationError::InvalidInput);
        }
        Ok(Self {
            inner: Arc::new(Inner {
                workspace,
                agents,
                storage,
                message_commit_clock,
                metadata,
                creation_audit,
                mode_audit,
                file_link_audit,
                deletion_audit,
                attachments,
                summaries,
                listing,
                summary_writes: ConversationLocks::default(),
                deletions: ConversationLocks::default(),
                mode_changes: ConversationLocks::default(),
                creations: ConversationLocks::default(),
                provider_sessions,
                deletion_budgets,
                environment,
                clock,
                agent_tool_audit: OnceLock::new(),
                agent_tool_admissions: Arc::new(SourcePermits::new(
                    super::MAX_AGENT_TOOL_ADMISSIONS,
                )),
                workspace_windows: super::WorkspaceWindows::new(),
                thread_reports: Default::default(),
                ownership: OnceLock::new(),
                thread_admission: Arc::new(Mutex::new(())),
                ownership_preparations: Mutex::new(HashMap::new()),
                access_policy,
                limits,
                conversations: Mutex::new(HashMap::new()),
                creating: AtomicUsize::new(0),
                creation: ConversationLocks::default(),
                retirement: OnceLock::new(),
                admission: Arc::new(RwLock::new(())),
                stops: watch::channel(0).0,
                retired: watch::channel(false).0,
                agents_asked: Arc::new(Semaphore::new(MAX_AGENTS_ASKED_AT_ONCE)),
                retries: Arc::new(DeletionRetries::default()),
                tool_uis,
                mcp_apps,
                apps: std::sync::Mutex::default(),
                live_changes: Arc::default(),
                app_calls: Arc::new(Semaphore::new(app_calls::MAX_APP_CALLS)),
                fetch_reads: Arc::new(Semaphore::new(artifact_fetch::FETCH_READS)),
                commands: Arc::new(OnceLock::new()),
                note_authors: Arc::new(OnceLock::new()),
                unfinished_turns: OnceLock::new(),
                unfinished_marked: std::sync::Mutex::default(),
                failure_backoff: OnceLock::new(),
                retry_scheduled: tokio::sync::Notify::new(),
                drive: Arc::new(OnceLock::new()),
                permission_rules: Arc::new(OnceLock::new()),
                fence_capture: Arc::new(OnceLock::new()),
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
        requested: RequestedConversation,
    ) -> Result<(), ConversationError> {
        let service = self.clone();
        supervised(async move {
            let _admission = service.admit().await?;
            service
                .create_admitted(id, caller, requested, false)
                .await
                .map(|_| ())
        })
        .await
    }
    // The caller retains this service's original admission guard through the
    // complete command, including receipt persistence after provider opening.
    async fn create_admitted(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        requested: RequestedConversation,
        new_only: bool,
    ) -> Result<Arc<LiveConversation>, ConversationError> {
        let service = self.clone();
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
        let creation_guard = service.inner.creation.lock(&id).await;
        if let Some(record) = load_conversation(service.inner.metadata.as_ref(), &id).await? {
            // A deleted conversation is not created again under its
            // identity, whoever asks; only its owner is told why
            // (`a_deleted_conversation_refuses_every_command_on_it`).
            service.access().admit(&record, &caller).map_err(|cause| {
                if new_only {
                    ConversationError::ChildCreationRefused(Box::new(cause.into()))
                } else {
                    ConversationError::from(cause)
                }
            })?;
            if new_only {
                return Err(ConversationError::ChildCreationRefused(Box::new(
                    ConversationError::RequestConflict,
                )));
            }
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
                let _creation = service.inner.creation.lock(&id).await;
                let current = load_conversation(service.inner.metadata.as_ref(), &id)
                    .await?
                    .ok_or(ConversationError::NotFound)?;
                service.access().admit(&current, &caller)?;
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
            return service.resolve(&id, &caller).await;
        }
        // Only now does the agent this caller asked for matter. Checking it
        // before the record above would have refused to reopen somebody's
        // existing Claude conversation because the panel's remembered choice
        // names an agent this server is no longer configured for — a
        // conversation that does not need that agent at all. The same is
        // true of a name no adapter exists for, which is why that one is
        // carried this far instead of being refused where it was parsed.
        let agent_request = match requested.agent {
            Some(RequestedAgent::Known(agent)) => Some(agent),
            Some(RequestedAgent::Unknown) => return Err(ConversationError::InvalidInput),
            None => None,
        };
        let agent = service.inner.agents.select(agent_request)?;
        let approval_mode = requested
            .approval_mode
            .unwrap_or_else(Self::default_creation_approval_mode);
        let configured = match requested.model.as_deref() {
            Some(model) => {
                service
                    .inner
                    .agents
                    .resolve_for(agent, model, approval_mode)
                    .await?
            }
            None => {
                let default = service.inner.agents.resolve(agent).await?;
                if approval_mode == ConversationApprovalMode::Ask {
                    default
                } else {
                    service
                        .inner
                        .agents
                        .resolve_for(agent, default.provider.identity().model_id(), approval_mode)
                        .await?
                }
            }
        };
        let selected_model = configured.provider.identity();
        let proposed = Conversation::new(
            id.clone(),
            caller.organization_id.clone(),
            caller.principal_id.clone(),
            caller.surface_id.clone(),
            caller.action_id.clone(),
            requested_at_ms,
            agent,
            ConversationModelId::new(selected_model.model_id())
                .map_err(|_| ConversationError::InvalidInput)?,
            approval_mode,
        )
        .map_err(|_| ConversationError::InvalidInput)?;
        // Past the cap, an idle agent is let go to make room; only when
        // every live agent is at work is the creation refused.
        let place = loop {
            let owners = service.inner.conversations.lock().await;
            if service.inner.retirement.get().is_some() {
                return Err(ConversationError::Unavailable);
            }
            if owners.contains_key(&id) {
                break None;
            }
            match CreationPlace::take(
                &service.inner.creating,
                owners.len(),
                service.inner.limits.max_conversations,
            ) {
                Ok(place) => break Some(place),
                Err(ConversationError::Capacity) => {
                    drop(owners);
                    if !service.make_room(&id).await {
                        return Err(ConversationError::Capacity);
                    }
                }
                Err(error) => return Err(error),
            }
        };
        // Where it runs is recorded before the conversation exists, under the
        // creation lock, so it never exists without knowing; a host the
        // configuration does not name creates nothing.
        service
            .inner
            .environment
            .place(&id, requested.environment.as_deref())
            .await?;
        let outcome = service.inner.metadata.create(proposed.clone()).await?;
        let record = &outcome.conversation;
        validate_target(&id, record)?;
        service.access().admit(record, &caller).map_err(|cause| {
            if new_only && outcome.disposition == super::ConversationCreationDisposition::Existing {
                ConversationError::ChildCreationRefused(Box::new(cause.into()))
            } else {
                ConversationError::from(cause)
            }
        })?;
        if new_only && outcome.disposition == super::ConversationCreationDisposition::Existing {
            return Err(ConversationError::ChildCreationRefused(Box::new(
                ConversationError::RequestConflict,
            )));
        }
        // Created acknowledges this exact admitted proposal. Existing alone
        // retains another creator's historical evidence. The regressions are
        // `creation_created_acknowledgement_preserves_the_admitted_origin` and
        // `creation_mislabeled_historical_created_refuses_before_audit_and_recovers`.
        if outcome.disposition == super::ConversationCreationDisposition::Created
            && record != &proposed
        {
            return Err(ConversationError::Metadata);
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
        let slot = loop {
            let mut owners = service.inner.conversations.lock().await;
            if service.inner.retirement.get().is_some() {
                return Err(ConversationError::Unavailable);
            }
            // This creation's own place, taken above, is the one its slot
            // fills; without one (its owner was live then and has gone since)
            // it is checked as any other.
            if place.is_none()
                && !owners.contains_key(&id)
                && owners.len() + service.inner.creating.load(Ordering::SeqCst)
                    >= service.inner.limits.max_conversations
            {
                drop(owners);
                if !service.make_room(&id).await {
                    return Err(ConversationError::Capacity);
                }
                continue;
            }
            let slot = match owners.entry(id.clone()) {
                std::collections::hash_map::Entry::Occupied(_) if new_only => {
                    return Err(ConversationError::ChildCreationRefused(Box::new(
                        ConversationError::RequestConflict,
                    )));
                }
                std::collections::hash_map::Entry::Occupied(entry) => entry.get().clone(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let slot = Arc::new(Slot::new(self.inner.clock.unix_milliseconds()));
                    // Audit and admission checks already passed. From this
                    // transfer through publication/start there is no fallible
                    // I/O and no unowned slot can supply this preparation.
                    let owned_preparation = if new_only {
                        service
                            .claim_owned_preparation(&id, &actor, slot.clone())
                            .await?
                    } else {
                        None
                    };
                    entry.insert(slot.clone());
                    service.start_slot(id.clone(), slot.clone(), actor.clone(), owned_preparation);
                    slot
                }
            };
            slot.touch(service.inner.clock.unix_milliseconds());
            drop(place);
            break slot;
        };
        drop(creation_guard);
        service.wait_for_slot(&id, slot).await
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
        Ok(self
            .resolve_opening(id, caller, true)
            .await
            .map(|(_, live)| live)?)
    }
    /// The conversation live now, for an answer to what its agent holds (a
    /// review). One with no agent live holds nothing to answer, so it is not
    /// opened, which past `limits.liveConversations` could be refused
    /// `conversation_capacity` or park another: the answer is stale.
    async fn resolve_holding_reviews(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<Arc<LiveConversation>, ConversationError> {
        if self.inner.metadata.pending_mode_change(id).await?.is_some()
            || self
                .inner
                .metadata
                .pending_model_change(id)
                .await?
                .is_some()
        {
            return self
                .live_now(id, caller)
                .await?
                .ok_or_else(|| AgentError::StalePermission.into());
        }
        match self.resolve_opening(id, caller, false).await {
            Ok((_, live)) => Ok(live),
            Err(Halt::Failed(ConversationError::McpApp(McpAppError::Cancelled))) => {
                Err(AgentError::StalePermission.into())
            }
            Err(halt) => Err(halt.into()),
        }
    }
    /// [`Self::resolve`]; with `open` false, only a conversation live now:
    /// one with none is refused `mcp_cancelled`, and not opened. The gateway
    /// retiring is [`Halt::Retired`]; an opening that failed is its own
    /// error, whatever it was. With the agent, the slot it was found in: what
    /// a submission asks whether the agent still takes work ([`Slot::stopping`]).
    async fn resolve_opening(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
        open: bool,
    ) -> Result<(Arc<Slot>, Arc<LiveConversation>), Halt> {
        if self.inner.metadata.pending_mode_change(id).await?.is_some() {
            return Err(ConversationError::ApprovalModeUncertain.into());
        }
        self.resolve_unchecked_opening(id, caller, open).await
    }
    async fn resolve_unchecked(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<Arc<LiveConversation>, ConversationError> {
        Ok(self
            .resolve_unchecked_opening(id, caller, true)
            .await
            .map(|(_, live)| live)?)
    }
    async fn resolve_unchecked_opening(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
        open: bool,
    ) -> Result<(Arc<Slot>, Arc<LiveConversation>), Halt> {
        let actor = caller.actor()?;
        let record = load_conversation(self.inner.metadata.as_ref(), id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        self.access()
            .admit(&record, caller)
            .map_err(ConversationError::from)?;
        let slot = loop {
            let mut owners = self.inner.conversations.lock().await;
            if self.inner.retirement.get().is_some() {
                return Err(Halt::Retired);
            }
            let slot = if let Some(slot) = owners.get(id) {
                slot.clone()
            } else if !open {
                return Err(ConversationError::McpApp(McpAppError::Cancelled).into());
            } else {
                if owners.len() + self.inner.creating.load(Ordering::SeqCst)
                    >= self.inner.limits.max_conversations
                {
                    drop(owners);
                    if !self.make_room(id).await {
                        return Err(ConversationError::Capacity.into());
                    }
                    continue;
                }
                let slot = Arc::new(Slot::new(self.inner.clock.unix_milliseconds()));
                owners.insert(id.clone(), slot.clone());
                self.start_slot(id.clone(), slot.clone(), actor.clone(), None);
                slot
            };
            slot.touch(self.inner.clock.unix_milliseconds());
            break slot;
        };
        let live = self.wait_for_slot(id, slot.clone()).await?;
        Ok((slot, live))
    }
    /// Park the agent used longest ago that could be parked now
    /// (`service/parking.rs`), so one more conversation can open past the
    /// bound on live agents rather than being refused. Nothing of it is
    /// lost: its records and owed work stay, and the next read or message
    /// opens it again on its provider session. An agent at work, followed,
    /// waiting on an app's review, or unable to resume is never parked.
    /// Answers whether one was.
    async fn make_room(&self, keep: &ConversationId) -> bool {
        let mut owners: Vec<(u64, ConversationId, Arc<Slot>)> = self
            .inner
            .conversations
            .lock()
            .await
            .iter()
            .filter(|(id, slot)| *id != keep && !slot.stopping.load(Ordering::SeqCst))
            .map(|(id, slot)| {
                (
                    slot.touched.load(Ordering::SeqCst),
                    id.clone(),
                    slot.clone(),
                )
            })
            .collect();
        owners.sort_by_key(|(touched, _, _)| *touched);
        for (touched, id, slot) in owners {
            // A submission or read holds this while it uses the agent; one
            // held now is a use. Never waited for: two openings at the bound
            // would each wait for the other's.
            let Some(submissions) = self.inner.mode_changes.try_lock(&id) else {
                continue;
            };
            if self
                .park_held(&id, slot, ParkWhen::UnusedSince(touched), submissions)
                .await
            {
                return true;
            }
        }
        false
    }
    fn start_slot(
        &self,
        id: ConversationId,
        slot: Arc<Slot>,
        actor: ActionContext,
        owned_preparation: Option<Arc<dyn OwnedLifetime>>,
    ) {
        let service = self.clone();
        let owner = slot.clone();
        if !slot.started.swap(true, Ordering::SeqCst) {
            tokio::spawn(async move {
                let result = owner
                    .value
                    .get_or_init(|| async {
                        let preparation = async {
                            let record = load_conversation(service.inner.metadata.as_ref(), &id)
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
                                resolved = service.inner.agents.resolve_for(
                                    agent,
                                    record.model().as_str(),
                                    record.approval_mode(),
                                ) => {
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
                            let session_id = conversation_session(&id);
                            let storage = service.inner.storage.clone();
                            let clock = service.inner.message_commit_clock.clone();
                            // The stopped agent's last handles may still hold
                            // this history. Wait them out, within the lease
                            // bound, instead of answering Busy once. A stop
                            // ends that wait: the opening must not sit out the
                            // lease and then launch a provider
                            // (`an_opening_waiting_on_a_history_lease_stops_with_the_service`).
                            let opened = tokio::select! {
                                opened = service.while_history_busy(|| {
                                    let storage = storage.clone();
                                    let clock = clock.clone();
                                    let session_id = session_id.clone();
                                    async move {
                                        SessionManager::open(Some(session_id), storage, clock)
                                            .await
                                    }
                                }) => opened,
                                _ = stops.changed() => {
                                    return Err(OpeningFailure {
                                        cause: ConversationError::Unavailable,
                                        holds: false,
                                    });
                                }
                            };
                            let mut manager = opened.map_err(|error| {
                                tracing::error!(conversation_id = %id, %error, "conversation storage opening failed");
                                OpeningFailure {
                                    cause: ConversationError::Storage(error),
                                    holds: false,
                                }
                            })?;
                            // The Agent is given the lease's fence and never
                            // the binding's provider itself: nothing reaches
                            // the agent except under this lease.
                            let identity = configured.provider.identity();
                            let work = AgentWork::new(agent.name(), identity.model_id())
                                .map_err(|_| OpeningFailure {
                                    cause: ConversationError::InvalidInput,
                                    holds: false,
                                })?;
                            // Where it runs was decided when it was
                            // created; a placement that cannot be read, or
                            // names a host no longer configured, opens it
                            // nowhere. On any worker of the pool, a worker
                            // that refuses it as it starts draining is left
                            // for another.
                            let ownership = match owned_preparation {
                                Some(ref gate) => Some(gate.clone()),
                                None => service.ownership_gate(&id, &actor, Some(owner.acquisition.clone())).await
                                    .map_err(|cause| OpeningFailure { cause, holds: false })?,
                            };
                            let mut avoid = Vec::new();
                            // The last acknowledged refusal is the fallback if
                            // placement cannot find another worker.
                            let mut last_refusal = None;
                            let selection = loop {
                                let place = match service
                                    .inner
                                    .environment
                                    .for_lease(&id, &avoid)
                                    .await
                                {
                                    Ok(place) => place,
                                    // Another worker could not be placed: the
                                    // refusal already given stands, and is
                                    // recorded as any refusal is.
                                    Err(_) if last_refusal.is_some() => {
                                        break Err(OpeningFailure {
                                            cause: ConversationError::LeaseRefused(last_refusal.expect("one refusal left")),
                                            holds: false,
                                        });
                                    }
                                    Err(cause) => {
                                        break Err(OpeningFailure {
                                            cause,
                                            holds: false,
                                        })
                                    }
                                };
                                let worker =
                                    place.context.as_ref().map(|context| context.worker());
                                // No other worker to ask: the refusal it gave stands.
                                if worker.as_ref().is_some_and(|worker| avoid.contains(worker)) {
                                    if let Some(refusal) = last_refusal {
                                        break Err(OpeningFailure {
                                            cause: ConversationError::LeaseRefused(refusal),
                                            holds: false,
                                        });
                                    }
                                }
                                // Placement may wait or select another worker.
                                // Check the same lifetime after that wait and
                                // before each environment lease is acquired.
                                if ownership.as_ref().is_some_and(|gate| gate.is_sealed()) {
                                    break Err(OpeningFailure { cause: ConversationError::Unavailable, holds: false });
                                }
                                let opening = open_lease_on(
                                    place.environment.as_ref(),
                                    LeaseRequest {
                                        work: work.clone(),
                                        sandbox: SandboxProfile::HarnessDefault,
                                        binding: configured.sandbox,
                                    },
                                    configured.provider.clone(),
                                    service.inner.clock.as_ref(),
                                    place.context,
                                )
                                .await;
                                match worker {
                                    Some(worker)
                                        if opening.refusal.is_some()
                                            && !avoid.contains(&worker)
                                            && service
                                                .inner
                                                .environment
                                                .left_the_pool(&worker) =>
                                    {
                                        tracing::info!(
                                            conversation_id = %id,
                                            worker = worker.name(),
                                            refusal = ?opening.refusal,
                                            "a worker that left the pool refused the lease; asking another"
                                        );
                                        // The refusal is consequential evidence,
                                        // including when the next placement waits
                                        // until this lifetime has sealed.
                                        manager.prepare_for_lease_record(opening.fence.as_ref()).await.map_err(|cause| OpeningFailure {
                                            cause: ConversationError::Agent(cause), holds: false,
                                        })?;
                                        let recorded = issue(&manager, place.environment.as_ref(), &opening, &actor).await;
                                        let recorded = match recorded {
                                            Err(LeaseIssueError::Record(LeaseRecordError::Storage(_))) => manager
                                                .save_retained_lease_records().await
                                                .map_err(|error| LeaseIssueError::Record(LeaseRecordError::Storage(error))),
                                            result => result,
                                        };
                                        if let Err(error) = recorded {
                                            tracing::error!(conversation_id = %id, %error, "a draining worker refusal could not be recorded");
                                            break Err(OpeningFailure { cause: error.into(), holds: false });
                                        }
                                        avoid.push(worker);
                                        last_refusal = opening.refusal;
                                    }
                                    _ => break Ok((place.environment, opening)),
                                }
                            };
                            let (environment, opening) = selection?;
                            let lease = Arc::new(LiveLease::new(&opening));
                            owner.acquisition.acquired(lease.clone());
                            let tool_source = checked_source(agent);
                            let agent = Agent::prepare(
                                opening.fence.clone(),
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
                            if let Some(gate) = ownership {
                                agent.install_owned_lifetime(gate).map_err(|error| OpeningFailure { cause: ConversationError::Agent(error), holds: true })?;
                            }
                            // Before anything can run: every call a Drive
                            // grant's turn asks about is checked against its
                            // policy. An agent that does not ask about every
                            // call admits no Drive turn, so it needs no hook.
                            let admitted_policies = Arc::new(AdmittedPolicies::default());
                            if let Some(source) = tool_source {
                                agent.add_invocation_hook(Arc::new(DriveToolHook {
                                    drive: service.inner.drive.clone(),
                                    admitted: admitted_policies.clone(),
                                    source,
                                }));
                            }
                            // Every agent: whether a send to this thread
                            // waits on a report is decided as each turn ends.
                            agent.add_invocation_hook(Arc::new(
                                thread_reports::ThreadReportHook {
                                    service: Arc::downgrade(&service.inner),
                                    child: id.clone(),
                                },
                            ));
                            let issued = issue(
                                agent.session_manager(),
                                environment.as_ref(),
                                &opening,
                                &actor,
                            )
                            .await;
                            // A lease issued whose record only failed to save is
                            // retained, and the close below saves it: it is ended
                            // with that close, so the stream never keeps a Live
                            // lease nothing runs under.
                            let retained = opening.refusal.is_none()
                                && matches!(
                                    issued,
                                    Err(LeaseIssueError::Record(LeaseRecordError::Storage(_)))
                                );
                            let refused = match (issued, opening.refusal) {
                                (Ok(()), None) => None,
                                // Nothing ran. The refusal is the cause once its
                                // record is durable; until then the cause is the
                                // storage failure, as for an issued lease whose
                                // record failed (one rule: a lease record a
                                // command made is saved, or it answers a storage
                                // failure).
                                // Any other failure to issue falls through to
                                // its own answer below: nothing was retained, so
                                // nothing of the refusal is in the records.
                                (
                                    issued @ (Ok(())
                                    | Err(LeaseIssueError::Record(LeaseRecordError::Storage(_)))),
                                    Some(refusal),
                                ) => {
                                    let saved = match issued {
                                        Ok(()) => Ok(()),
                                        Err(error) => {
                                            tracing::error!(conversation_id = %id, %error, "a refused lease could not be recorded");
                                            // A refused opening is never attached,
                                            // so its close saves nothing: what the
                                            // failed write left retained is written
                                            // here
                                            // (`l2_a_refusal_whose_record_failed_to_save_is_still_the_refusal`).
                                            agent
                                                .session_manager()
                                                .save_retained_lease_records()
                                                .await
                                        }
                                    };
                                    Some(match saved {
                                        Ok(()) => ConversationError::LeaseRefused(refusal),
                                        Err(error) => {
                                            tracing::error!(conversation_id = %id, %error, ?refusal, "a refused lease could not be saved");
                                            ConversationError::Storage(error)
                                        }
                                    })
                                }
                                (Err(error), _) => {
                                    tracing::error!(conversation_id = %id, %error, "a lease could not be recorded");
                                    Some(error.into())
                                }
                            };
                            if let Some(cause) = refused {
                                // Nothing was attached; the Agent holds only
                                // the history, which its close lets go of.
                                let closed = if retained {
                                    lease
                                        .close(
                                            &agent,
                                            LeaseEndCause::Closed,
                                            actor.clone(),
                                            service.inner.deletion_budgets.stop,
                                            AgentEnd::Close,
                                        )
                                        .await
                                } else {
                                    agent.close(actor.clone()).await
                                };
                                if let Err(error) = &closed {
                                    tracing::error!(conversation_id = %id, %error, "a failed opening's close did not finish");
                                }
                                // What it holds is the Agent's own fact, not the
                                // close's diagnostic: a close whose only failure
                                // is a record it could not save holds nothing, and
                                // a failed opening that holds nothing is let go,
                                // so the next command opens it again
                                // (`l2_a_refusal_that_cannot_be_saved_is_a_storage_failure_and_is_retried_once_storage_recovers`).
                                let holds = agent.may_hold_provider_resources();
                                return Err(OpeningFailure { cause, holds });
                            }
                            let authorization = agent
                                .authorize_attachment(AttachmentRequest::CallerRequested(actor.clone()))
                                .map_err(|error| OpeningFailure {
                                    cause: ConversationError::Agent(error),
                                    holds: true,
                                })?;
                            let snapshot = agent.session_manager().snapshot().await;
                            let operation_capabilities = agent.operation_capabilities();
                            let capabilities = ConversationCapabilities {
                                queue: true,
                                steer: true,
                                resume: operation_capabilities.session_resume(),
                                permissions: agent.capabilities().features().tool_use(),
                                // Until the agent answers, the view admits what a
                                // send admits: unknown is not no.
                                image_input: service
                                    .takes_images(&agent, operation_capabilities)
                                    != Some(false),
                                agent_features: operation_capabilities.into(),
                            };
                            let mut projection =
                                Projection::new(id.to_string(), capabilities, snapshot.as_ref())
                                    .with_tool_uis(service.inner.tool_uis.clone());
                            projection.selection(ConversationSelectionView {
                                agent: record.agent().expect("agent was resolved"),
                                model: record.model().as_str().into(),
                                approval_mode: record.approval_mode(),
                            });
                            projection.lifecycle(lifecycle_view(&agent));
                            // Where its agent works: what its environment
                            // reported (a host's own directory), else this
                            // gateway's.
                            let workspace = opening
                                .workspace
                                .as_ref()
                                .or(service.inner.workspace.as_ref());
                            if let Some(workspace) = workspace {
                                let identity = configured.provider.identity();
                                projection.runtime(ConversationRuntime {
                                    model: identity.model_id().into(),
                                    provider: identity.name().into(),
                                    workspace: workspace.clone(),
                                });
                            }
                            let app_reviews = service.apps_of(&id);
                            // Every opening is ended before the next begins,
                            // so this drops nothing (a close then a reopening
                            // records the close's drop alone:
                            // `c15_the_openings_end_drops_every_context_unsent`);
                            // should one not be, what it held is dropped by
                            // the system, and reported
                            // (`c14_c15_contexts_are_dropped_with_their_mount_the_openings_end_a_new_opening_and_a_delete`).
                            let app_epoch = app_reviews.begin();
                            let attachment = agent.clone();
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
                                match attachment.start_attachment(authorization) {
                                    Ok(wait) => {
                                        if let Err(error) = wait.wait().await {
                                            report_opening_failure(&attachment_id, &error);
                                        }
                                    }
                                    Err(AgentError::Closed) => {}
                                    Err(error) => report_opening_failure(&attachment_id, &error),
                                }
                                // Attached or failed: the view's lifecycle
                                // and capabilities follow the agent's.
                                attachment_service
                                    .inner
                                    .live_changes
                                    .publish(&attachment_id);
                            });
                            let live = Arc::new(LiveConversation {
                                model_budget: Mutex::new(ModelBudget {
                                    identity: agent.provider_identity(),
                                    reserved_output_tokens: configured.reserved_output_tokens,
                                }),
                                agent,
                                lease,
                                projection: Mutex::new(projection),
                                watched: Mutex::new(HashSet::new()),
                                attachment_owner: Mutex::new(AttachmentOwner::Running(owner)),
                                app_reviews,
                                app_epoch,
                                publishes: Arc::default(),
                                fetches: Arc::default(),
                                organization_id: record.organization().clone(),
                                tool_source,
                                admitted_policies,
                            });
                            // What the host publishes under the lease is
                            // kept for as long as the lease is held.
                            if let Some(offers) = live.lease.artifacts() {
                                service.collect_artifacts(
                                    Arc::downgrade(&live),
                                    offers,
                                    ArtifactCollector::new(
                                        record.organization().clone(),
                                        id.clone(),
                                        live.lease.id().clone(),
                                        actor.clone(),
                                        service.inner.attachments.clone(),
                                        ArtifactBudget::LEASE,
                                    ),
                                );
                            }
                            Ok(live)
                        };
                        let result = match AssertUnwindSafe(preparation).catch_unwind().await {
                            Ok(result) => result,
                            Err(payload) => {
                                mem::forget(payload);
                                Err(OpeningFailure {
                                    cause: ConversationError::Unavailable,
                                    holds: true,
                                })
                            }
                        };
                        match &result {
                            Ok(_) => { owner.acquisition.transferred(); owner.acquisition.complete(true, service.inner.deletion_budgets.stop).await; }
                            Err(failure) => owner.acquisition.complete(failure.holds, service.inner.deletion_budgets.stop).await,
                        }
                        result
                    })
                    .await;
                owner.ready.notify_waiters();
                if result.is_err() && owner.acquisition.can_retire() {
                    let mut owners = service.inner.conversations.lock().await;
                    if owners
                        .get(&id)
                        .is_some_and(|known| Arc::ptr_eq(known, &owner))
                    {
                        owners.remove(&id);
                    }
                }
                // Opened or not, a view's lifecycle says so now.
                service.inner.live_changes.publish(&id);
                if let Ok(live) = result {
                    if let Some(ends) = live.lease.ends() {
                        service.stop_when_environment_ends(
                            id.clone(),
                            Arc::downgrade(&owner),
                            ends,
                        );
                    }
                    // A conversation a restart left with unfinished turns
                    // carries them on as soon as it is open again. The slot
                    // is let go first: the resume waits for the submission
                    // lock, and a close and a reopening ahead of it there
                    // must find this agent's history free, not held by a
                    // slot only this task still keeps
                    // (`a_read_behind_a_close_of_an_opening_agent_opens_it_again`).
                    let live = live.clone();
                    drop(owner);
                    service.resume_owed(&id, live).await;
                }
            });
        }
    }

    /// Stop the opening `slot` holds once its environment can no longer run
    /// anything for its lease: its SSH connection lost (row L10), the
    /// environment ending it past the duration it granted (row I7), or a
    /// paired environment's credential revoked while it runs (row I8). The
    /// lease ends for that cause, the system's stop and never a person's,
    /// recorded first like any other end; its End is still asked, and the
    /// environment answers it with what its own end took. There is no
    /// reconnect budget: a connection lost is the lease lost. Nothing is
    /// stopped once the slot is let go, stopping, or holds another opening;
    /// `ends` also resolves when the lease ends, so this never outlives the
    /// opening.
    fn stop_when_environment_ends(
        &self,
        id: ConversationId,
        slot: std::sync::Weak<Slot>,
        ends: super::EnvironmentFuture<'static, LeaseEndCause>,
    ) {
        let service = self.clone();
        tokio::spawn(async move {
            let cause = ends.await;
            let Some(slot) = slot.upgrade() else {
                return;
            };
            let current = service.inner.conversations.lock().await.get(&id).cloned();
            if !current.is_some_and(|current| Arc::ptr_eq(&current, &slot))
                || slot.stopping.load(Ordering::SeqCst)
            {
                return;
            }
            let (operation, prefix) = match cause {
                LeaseEndCause::Expired => ("environment_expired", "expired"),
                LeaseEndCause::Revoked => ("environment_revoked", "revoked"),
                LeaseEndCause::Stopped => ("environment_drained", "drained"),
                _ => ("environment_lost", "lost"),
            };
            let Ok(actor) =
                ActionContext::new("gateway", operation, format!("{prefix}-{}", Uuid::new_v4()))
            else {
                return;
            };
            tracing::warn!(conversation_id = %id, ?cause, "the environment ended this conversation's lease; stopping it");
            if let Err(stop) = service
                .stop_and_release(
                    &id,
                    slot,
                    &actor,
                    &McpAppInitiator::System,
                    cause,
                    AgentEnd::Close,
                )
                .await
            {
                tracing::warn!(conversation_id = %id, ?stop, "an environment-ended lease's stop did not confirm cleanup");
            }
        });
    }
    /// Keep each file published under a live conversation's lease, one at
    /// a time, until the lease is no longer held and its offers end. An
    /// offer that arrives once the conversation is let go is answered not
    /// kept, by its answer's drop.
    fn collect_artifacts(
        &self,
        live: std::sync::Weak<LiveConversation>,
        mut offers: tokio::sync::mpsc::Receiver<ArtifactOffer>,
        mut collector: ArtifactCollector,
    ) {
        tokio::spawn(async move {
            while let Some(offer) = offers.recv().await {
                let Some(live) = live.upgrade() else {
                    continue;
                };
                collector
                    .collect(offer, &live.agent, &live.lease, &live.publishes)
                    .await;
            }
        });
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
                if result.is_err() && slot.acquisition.can_retire() {
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
    async fn retire_mode_session(&self, id: &ConversationId) -> Result<(), ConversationError> {
        let actor = ActionContext::new(
            "gateway",
            "mode_recovery",
            format!("recovery-{}", Uuid::new_v4()),
        )
        .map_err(|_| ConversationError::ApprovalModeUncertain)?;
        self.retire_selection_session(id, &actor).await
    }
    async fn retire_selection_session(
        &self,
        id: &ConversationId,
        actor: &ActionContext,
    ) -> Result<(), ConversationError> {
        let slot = self.inner.conversations.lock().await.get(id).cloned();
        if let Some(slot) = slot {
            self.retire_selection_slot(id, slot, actor).await?;
        }
        Ok(())
    }

    async fn retire_selection_slot(
        &self,
        id: &ConversationId,
        slot: Arc<Slot>,
        actor: &ActionContext,
    ) -> Result<(), ConversationError> {
        self.stop_and_release(
            id,
            slot,
            actor,
            &McpAppInitiator::System,
            LeaseEndCause::Stopped,
            AgentEnd::Suspend,
        )
        .await
        .map_err(|stop| match stop {
            StopFailure::Failed(error) if confirmed_despite(&error).is_some() => {
                ConversationError::Agent(error)
            }
            _ => ConversationError::ApprovalModeUncertain,
        })
    }

    /// Recovery may retry a retained stop; publication may not read it.
    /// Capture the stopped slot before awaiting so cleanup cannot retarget a
    /// replacement owner. Healthy attached owners are not retired here.
    async fn finish_stopped_selection(&self, id: &ConversationId) -> Result<(), ConversationError> {
        let stopped = self
            .inner
            .conversations
            .lock()
            .await
            .get(id)
            .cloned()
            .filter(|slot| slot.stopping.load(Ordering::SeqCst));
        let Some(slot) = stopped else {
            return Ok(());
        };
        let actor = ActionContext::new(
            "gateway",
            "selection_recovery",
            format!("recovery-{}", Uuid::new_v4()),
        )
        .map_err(|_| ConversationError::Unavailable)?;
        self.retire_selection_slot(id, slot, &actor).await
    }

    async fn recover_mode_change(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<(), ConversationError> {
        let record = load_conversation(self.inner.metadata.as_ref(), id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        self.access().admit(&record, caller)?;
        let Some(request) = self.inner.metadata.pending_mode_change(id).await? else {
            return Ok(());
        };
        let mut request = pending_for(id, &record, request)?;
        if request.prior == request.requested {
            match request.application {
                None
                | Some(
                    ConversationModeApplication::Applied | ConversationModeApplication::Deferred,
                ) => {}
                Some(
                    ConversationModeApplication::Refused | ConversationModeApplication::Uncertain,
                ) => return Err(ConversationError::ApprovalModeUncertain),
            }
        }
        if request.prior != request.requested {
            self.finish_stopped_selection(id).await?;
        }
        let committed = self.committed_if_present_with_order(id).await?;
        let configuration = committed
            .as_ref()
            .and_then(|(committed, _)| committed.snapshot())
            .and_then(|snapshot| snapshot.current_configuration.as_ref());
        if request.prior == request.requested
            || configuration.is_some_and(|configuration| {
                configuration.kind() == SessionConfigurationKind::ApprovalMode
                    && configuration.actor().principal_id()
                        == request.initiator_principal_id.as_str()
                    && configuration.actor().surface_id() == request.initiator_surface_id
                    && configuration.actor().request_id() == request.request_id
                    && configuration.after().approval_mode()
                        == Some(provider_approval_mode(request.requested))
                    && configuration.before().is_some_and(|prior| {
                        prior.approval_mode() == Some(provider_approval_mode(request.prior))
                    })
            })
        {
            let application = if request.prior == request.requested {
                request
                    .application
                    .unwrap_or(ConversationModeApplication::Applied)
            } else {
                ConversationModeApplication::Applied
            };
            let observed = ConversationModeRequest {
                application: Some(application),
                ..request
            };
            let saved = self
                .inner
                .metadata
                .observe_mode_application(id, &observed.request_id, application)
                .await?;
            if saved != observed {
                return Err(ConversationError::ApprovalModeUncertain);
            }
            self.inner
                .mode_audit
                .record(
                    observed.clone(),
                    ConversationModeAuditPhase::RecoveryApplied,
                )
                .await
                .map_err(|_| ConversationError::Audit)?;
            if let Some(live) = self.live_now(id, caller).await? {
                if live.agent.approval_mode() != Some(provider_approval_mode(observed.requested)) {
                    if observed.prior == observed.requested {
                        return Err(ConversationError::ApprovalModeUncertain);
                    }
                    self.retire_mode_session(id).await?;
                }
            }
            let expected = ConversationModeRequest {
                state: ConversationModeRequestState::Applied,
                ..observed
            };
            let written = contain_mode_terminal(async {
                self.inner
                    .metadata
                    .finish_mode_change(
                        id,
                        &expected.request_id,
                        ConversationModeRequestState::Applied,
                    )
                    .await
            })
            .await;
            let terminal = match written {
                Ok(Ok(request)) => Some(request),
                _ => contain_mode_terminal(async {
                    self.inner
                        .metadata
                        .mode_change(id, &expected.request_id)
                        .await
                })
                .await
                .ok()
                .and_then(Result::ok)
                .flatten(),
            };
            if terminal.as_ref() != Some(&expected) {
                return Err(ConversationError::ApprovalModeUncertain);
            }
            if let Some(live) = self.live_now(id, caller).await? {
                live.projection
                    .lock()
                    .await
                    .approval_mode(expected.requested);
            }
            return Ok(());
        }
        if configuration.is_some_and(|configuration| {
            configuration.after().approval_mode() != Some(provider_approval_mode(request.prior))
        }) {
            return Err(ConversationError::ApprovalModeUncertain);
        }
        self.retire_mode_session(id).await?;
        if request.application.is_none() {
            self.inner
                .metadata
                .observe_mode_application(
                    id,
                    &request.request_id,
                    ConversationModeApplication::Uncertain,
                )
                .await?;
            request = ConversationModeRequest {
                application: Some(ConversationModeApplication::Uncertain),
                ..request
            };
        }
        self.inner
            .mode_audit
            .record(request.clone(), ConversationModeAuditPhase::Application)
            .await
            .map_err(|_| ConversationError::Audit)?;
        // The ownership row still carries the prior committed mode. Opening a
        // fresh Agent from that row re-applies it at session startup; no
        // requested-mode mutation is replayed.
        let live = self.resolve_unchecked(id, caller).await?;
        let _ = live.join_attachment_owner().await;
        // The newly attached provider was constructed from the committed row.
        // Verify that choice rather than sending a second mutation: fixed-mode
        // providers such as OpenCode have no live set-mode operation.
        if live.agent.attachment_status().phase() != AttachmentPhase::Attached
            || live.agent.approval_mode() != Some(provider_approval_mode(request.prior))
        {
            return Err(ConversationError::ApprovalModeUncertain);
        }
        self.inner
            .mode_audit
            .record(
                request.clone(),
                ConversationModeAuditPhase::RecoveryRestored,
            )
            .await
            .map_err(|_| ConversationError::Audit)?;
        self.inner
            .metadata
            .finish_mode_change(
                id,
                &request.request_id,
                ConversationModeRequestState::NotApplied,
            )
            .await?;
        Ok(())
    }

    /// Change a native preset on the current owner, including during a turn.
    /// The SDK enforces checked-turn safety; the per-owner admission lock
    /// spans intent, native acknowledgement, audit and terminal storage.
    pub async fn set_approval_mode(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        mode: ConversationApprovalMode,
    ) -> Result<ConversationApprovalMode, ConversationError> {
        let service = self.clone();
        let changed = (self.inner.live_changes.clone(), id.clone());
        supervised(async move {
            let result = async move {
            let _admission = service.admit().await?;
            let _mode = service.inner.mode_changes.lock(&id).await;
            let actor = caller.actor()?;
            let record = load_conversation(service.inner.metadata.as_ref(), &id)
                .await?
                .ok_or(ConversationError::NotFound)?;
            service.access().admit(&record, &caller)?;
            let matches_call = |request: &ConversationModeRequest| {
                request.conversation_id == id
                    && request.request_id == caller.action_id
                    && request.requested == mode
                    && request.organization_id == caller.organization_id
                    && request.initiator_principal_id == caller.principal_id
                    && request.initiator_surface_id == caller.surface_id
            };
            let saved_result = |request: &ConversationModeRequest| match request.state {
                ConversationModeRequestState::Applied if matches!(request.application, Some(ConversationModeApplication::Applied | ConversationModeApplication::Deferred)) => Ok(request.requested),
                ConversationModeRequestState::Applied | ConversationModeRequestState::Pending => Err(ConversationError::ApprovalModeUncertain),
                ConversationModeRequestState::NotApplied if request.application.is_some() => Err(ConversationError::ApprovalModeNotApplied),
                ConversationModeRequestState::NotApplied => Err(ConversationError::ApprovalModeUncertain),
            };
            if let Some(existing) = service
                .inner
                .metadata
                .mode_change(&id, &caller.action_id)
                .await?
            {
                if !matches_call(&existing) {
                    return Err(ConversationError::RequestConflict);
                }
                return saved_result(&existing);
            }
            service.refuse_pending_model_change(&id, &caller).await?;
            if service.inner.metadata.pending_mode_change(&id).await?.is_some() {
                return Err(ConversationError::ApprovalModeUncertain);
            }
            let agent = record.agent().ok_or(ConversationError::AgentUnsupported)?;
            service
                .inner
                .agents
                .resolve_for(agent, record.model().as_str(), mode)
                .await?;
            let live_slot = service.inner.conversations.lock().await.get(&id).cloned();
            let live = if let Some(slot) = live_slot {
                Some(service.wait_for_slot(&id, slot).await?)
            } else if mode != record.approval_mode()
                && service.committed_if_present_with_order(&id).await?.is_some_and(|(committed, _)| {
                    committed.snapshot().is_some_and(|snapshot| snapshot.current_configuration.is_some())
                })
            {
                // Retained SDK settings own the next attachment. Advance that
                // owner through its normal native control before acknowledging
                // metadata; Deferred cannot supersede its saved configuration.
                Some(service.resolve(&id, &caller).await?)
            } else {
                None
            };
            if let Some(live) = &live {
                let _ = live.join_attachment_owner().await;
                if live.agent.attachment_status().phase() != AttachmentPhase::Attached {
                    return Err(ConversationError::ApprovalModeUncertain);
                }
                if live.agent.approval_mode()
                    != Some(provider_approval_mode(record.approval_mode()))
                {
                    return Err(ConversationError::ApprovalModeNotApplied);
                }
            }
            let intent = ConversationModeRequest {
                conversation_id: id.clone(),
                organization_id: caller.organization_id.clone(),
                    request_id: caller.action_id.clone(),
                    initiator_principal_id: caller.principal_id.clone(),
                    initiator_surface_id: caller.surface_id.clone(),
                    prior: record.approval_mode(),
                    requested: mode,
                    state: ConversationModeRequestState::Pending,
                application: None,
                requested_at_ms: service.inner.clock.unix_milliseconds(),
            };
            let request = match service.inner.metadata.begin_mode_change(intent.clone()).await {
                Ok(request) => request,
                Err(error) => {
                    // A lost write acknowledgement must not cause a second
                    // provider mutation. Correlate the durable intent first.
                    return match service.inner.metadata.mode_change(&id, &caller.action_id).await {
                        Ok(Some(_)) | Err(_) => Err(ConversationError::ApprovalModeUncertain),
                        Ok(None) => Err(error),
                    };
                }
            };
            if request.state != ConversationModeRequestState::Pending {
                if !matches_call(&request) {
                    return Err(ConversationError::RequestConflict);
                }
                return saved_result(&request);
            }
            // The admitted prior and actor are the original intent's, not an
            // adapter reply's. A changed pending record cannot authorize an effect.
            if request != intent {
                return Err(ConversationError::ApprovalModeUncertain);
            }
            // The durable intent is visible before the native call can wait.
            // Its overlay retains the committed preset and history cursor.
            if intent.prior != intent.requested {
                service.inner.live_changes.publish(&id);
            }
            let application = match &live {
                None => ConversationModeApplication::Deferred,
                Some(_) if mode == intent.prior => ConversationModeApplication::Applied,
                Some(live) => match live
                    .agent
                    .set_approval_mode(provider_approval_mode(mode), actor)
                    .await
                {
                    Ok(()) => ConversationModeApplication::Applied,
                    Err(failure) => match failure.stage() {
                        ProviderOperationStage::BeforeEffect => ConversationModeApplication::Refused,
                        ProviderOperationStage::EffectUncertain => ConversationModeApplication::Uncertain,
                    },
                },
            };
            match service
                .inner
                .metadata
                .observe_mode_application(&id, &intent.request_id, application)
                .await {
                    Ok(_) => {},
                    Err(error) => {
                        drop(live);
                        if intent.prior != intent.requested {
                            let _ = service.retire_mode_session(&id).await;
                        }
                        return Err(error);
                    }
                };
            // The application audit describes the effect this owner observed.
            // Metadata's response cannot replace its target, actor or meaning.
            let request = ConversationModeRequest {
                application: Some(application),
                ..intent
            };
            if service
                .inner
                .mode_audit
                .record(request.clone(), ConversationModeAuditPhase::Application)
                .await
                .is_err()
            {
                drop(live);
                let _ = service.recover_mode_change(&id, &caller).await;
                return Err(ConversationError::Audit);
            }
            if application == ConversationModeApplication::Uncertain {
                drop(live);
                if let Err(error) = service.recover_mode_change(&id, &caller).await {
                    tracing::warn!(conversation_id = %id, %error, "approval-mode recovery remains pending");
                }
                return Err(ConversationError::ApprovalModeUncertain);
            }
            let state = if application == ConversationModeApplication::Refused {
                ConversationModeRequestState::NotApplied
            } else { ConversationModeRequestState::Applied };
            let expected = ConversationModeRequest {state, ..request};
            // Keep the admitted owner through invocation and polling faults:
            // a lost terminal acknowledgement still needs durable settlement.
            let write = contain_mode_terminal(async {
                service.inner.metadata.finish_mode_change(
                    &id, &expected.request_id, state,
                ).await
            }).await;
            let terminal = match write {
                Ok(Ok(terminal)) => Some(terminal),
                Ok(Err(_)) | Err(_) => contain_mode_terminal(async {
                    service.inner.metadata.mode_change(&id, &expected.request_id).await
                }).await.ok().and_then(Result::ok).flatten(),
            }.filter(|terminal| terminal == &expected);
            let Some(terminal) = terminal else {
                drop(live);
                // A changed selection may leave a stale held projection when
                // the terminal acknowledgement is lost. An unchanged request
                // owns no native effect and must not retire that owner.
                if expected.prior == expected.requested
                    || service.retire_mode_session(&id).await.is_ok()
                {
                    let _ = service.recover_mode_change(&id, &caller).await;
                }
                return Err(ConversationError::ApprovalModeUncertain);
            };
            if terminal.state == ConversationModeRequestState::NotApplied {
                return Err(ConversationError::ApprovalModeNotApplied);
            }
            if let Some(live) = live {
                live.projection.lock().await.approval_mode(terminal.requested);
            }
            Ok(terminal.requested)
            }
            .await;
            // However it ended — applied, refused, left pending — a view
            // shows the mode and any change still in progress.
            changed.0.publish(&changed.1);
            result
        })
        .await
    }

    /// Read a bounded current replacement projection; never replay provider input to reconstruct it.
    pub async fn read(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
    ) -> Result<ConversationView, ConversationError> {
        self.read_at(id, caller, ReadOpening::Open)
            .await
            .map(|(view, _)| view)
    }

    /// Capture the current locally owned execution after target admission.
    /// Opens no provider and does not infer activity from a bounded display view.
    pub async fn active_execution(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
    ) -> Result<Option<String>, ConversationError> {
        let _admission = self.admit().await?;
        let live = self.live_now(&id, &caller).await?;
        // Waiting for an already opening slot must not retain old target authority.
        self.check_view_access(&id, &caller).await?;
        Ok(live
            .and_then(|live| live.agent.active_execution_id())
            .map(|execution| execution.as_str().to_owned()))
    }

    /// Follow `id`'s live facts: subscribe before [`Self::read_at`], and a
    /// change during or after that read is not missed.
    pub fn live_changes(&self, id: &ConversationId) -> tokio::sync::watch::Receiver<u64> {
        self.inner.live_changes.subscribe(id)
    }

    /// [`Self::read`], with where in the committed history the view was
    /// folded through: the projection's accepted position, so the cursor
    /// never runs ahead of, or behind, what the view shows. `opening` says
    /// whether a conversation with no agent live is opened to be read
    /// ([`ReadOpening`]).
    pub async fn read_at(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        opening: ReadOpening,
    ) -> Result<(ConversationView, Option<CommittedCursor>), ConversationError> {
        let _admission = self.admit().await?;
        // An explicit Open can recover a stopped owner; a follower may only
        // publish a readable owner. The same ordering applies to both controls.
        let ready = if matches!(opening, ReadOpening::Open) {
            self.inner.mode_changes.try_lock(&id)
        } else {
            None
        };
        let _selection = if let Some(guard) = ready {
            guard
        } else {
            if let Some(read) = self.pending_change_view(&id, &caller).await? {
                return Ok(read);
            }
            self.inner.mode_changes.lock(&id).await
        };
        match opening {
            ReadOpening::Open => {
                let recovered = match self.recover_model_change(&id, &caller).await {
                    Ok(()) => self.recover_mode_change(&id, &caller).await,
                    Err(error) => Err(error),
                };
                if let Err(error) = recovered {
                    // Read again after recovery: cleanup may have ended or
                    // replaced the owner a preceding view would have captured.
                    if let Some(read) = self.pending_change_view(&id, &caller).await? {
                        return Ok(read);
                    }
                    return Err(error);
                }
            }
            ReadOpening::LiveOnly => {
                if let Some(read) = self.pending_change_view(&id, &caller).await? {
                    return Ok(read);
                }
            }
        }
        let live = match opening {
            ReadOpening::Open => self.resolve(&id, &caller).await?,
            ReadOpening::LiveOnly => match self.live_now(&id, &caller).await? {
                Some(live) => live,
                None => return self.read_unopened(&id, &caller).await,
            },
        };
        let session_id = conversation_session(&id);
        let committed = self
            .inner
            .storage
            .read_committed(session_id.clone())
            .await?
            .ok_or(ConversationError::NotFound)?;
        if committed.id() != &session_id {
            return Err(ConversationError::Storage(StorageError::IdentityMismatch));
        }
        let order = committed
            .snapshot()
            .map(SessionSnapshot::pending_order)
            .transpose()?
            .unwrap_or_default();
        let active = live.agent.active_execution_id();
        let record = load_conversation(self.inner.metadata.as_ref(), &id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        let model_effect = self
            .confirmed_model_effect(&record, committed.snapshot())
            .await?;
        let mut projection = live.projection.lock().await;
        if projection.replace_committed(&committed, &order, active.as_ref()) {
            projection.transcript_state(committed.state().into());
        }
        let operation_capabilities = live.agent.operation_capabilities();
        // The last known answer stands while the agent is being restored, so the
        // view never says no to what a send at the same moment would admit.
        let image_input = self
            .takes_images(&live.agent, operation_capabilities)
            .unwrap_or(projection.view().capabilities.image_input);
        projection.capabilities(ConversationCapabilities {
            queue: true,
            steer: true,
            resume: operation_capabilities.session_resume(),
            permissions: live.agent.capabilities().features().tool_use(),
            image_input,
            agent_features: operation_capabilities.into(),
        });
        projection.lifecycle(lifecycle_view(&live.agent));
        if let Some(effect) = model_effect {
            projection.model_selection(
                record.model().as_str().into(),
                record.approval_mode(),
                effect,
            );
        }
        let mut view = projection.read();
        let cursor = projection.committed_cursor();
        drop(projection);
        view.title = self.title(&id).await;
        // Every review waiting is listed from its live owner, never from
        // history: an answer by a person or a remembered choice leaves no
        // event there, so only the owner knows it is settled.
        let view = app_calls::with_reviews(
            view,
            agent_review_entries(&live.agent, committed.snapshot()),
            live.app_reviews.entries(),
            agent_question_entries(&live.agent, committed.snapshot()),
        );
        self.check_view_access(&id, &caller).await?;
        let slot = self
            .readable_slot(&id)
            .await?
            .ok_or(ConversationError::Agent(AgentError::Closed))?;
        let same_owner = slot
            .value
            .get()
            .and_then(|value| value.as_ref().ok())
            .is_some_and(|known| Arc::ptr_eq(known, &live));
        if !same_owner {
            return Err(ConversationError::Agent(AgentError::Closed));
        }
        Ok((view, cursor))
    }
    /// The agent of `id` if one is live or opening (waited for), after the
    /// caller's access is checked; `None` when nothing has it open, or its
    /// opening failed. Opens nothing. A failed opening is not the follower's
    /// to report: it reads the committed history, and `conversation.read` or
    /// a send says why the agent did not open (row S28).
    async fn live_now(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<Option<Arc<LiveConversation>>, ConversationError> {
        self.check_view_access(id, caller).await?;
        let slot = self.readable_slot(id).await?;
        match slot {
            // `wait_for_slot` fails only with the slot's own opening failure.
            Some(slot) => Ok(self.wait_for_slot(id, slot).await.ok()),
            None => Ok(None),
        }
    }
    /// The existing slot owns both submission admission and current publication.
    /// A retained stop cannot produce a clean replacement from its old projection
    /// (`retained_stopping_publication_refuses_current_reads_until_confirmed_cleanup`).
    async fn readable_slot(
        &self,
        id: &ConversationId,
    ) -> Result<Option<Arc<Slot>>, ConversationError> {
        let slot = self.inner.conversations.lock().await.get(id).cloned();
        if slot
            .as_ref()
            .is_some_and(|slot| slot.stopping.load(Ordering::SeqCst))
        {
            return Err(ConversationError::Agent(AgentError::Closed));
        }
        Ok(slot)
    }
    /// A conversation nothing has open, as its committed records fold with
    /// no agent: read-only, lifecycle absent, nothing awaiting an answer.
    async fn read_unopened(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<(ConversationView, Option<CommittedCursor>), ConversationError> {
        let record = load_conversation(self.inner.metadata.as_ref(), id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        self.access().admit(&record, caller)?;
        let (committed, order) = self.committed_with_order(id).await?;
        let mut projection = self.detached(id, &committed, &order);
        projection.selection(ConversationSelectionView {
            agent: record.agent().ok_or(ConversationError::AgentUnsupported)?,
            model: record.model().as_str().into(),
            approval_mode: record.approval_mode(),
        });
        if let Some(effect) = self
            .confirmed_model_effect(&record, committed.snapshot())
            .await?
        {
            projection.model_selection(
                record.model().as_str().into(),
                record.approval_mode(),
                effect,
            );
        }
        let read = (projection.read(), projection.committed_cursor());
        let (mut view, cursor) = self.read_only(read, &record, id, caller).await?;
        // A message opens it again, so images may be sent if this gateway
        // takes them; an agent that does not is refused at submission.
        view.capabilities.image_input = self
            .inner
            .attachments
            .as_ref()
            .is_some_and(|attachments| attachments.offers_images());
        Ok((view, cursor))
    }
    /// The committed history of `id`, and its pending order.
    async fn committed_with_order(
        &self,
        id: &ConversationId,
    ) -> Result<(CommittedSession, Vec<ExecutionId>), ConversationError> {
        self.committed_if_present_with_order(id)
            .await?
            .ok_or(ConversationError::NotFound)
    }

    /// Metadata-only creation and an admitted cold preset may precede the
    /// first SDK record. Missing history is distinct from unreadable history.
    async fn committed_if_present_with_order(
        &self,
        id: &ConversationId,
    ) -> Result<Option<(CommittedSession, Vec<ExecutionId>)>, ConversationError> {
        let session_id = conversation_session(id);
        let Some(committed) = self
            .inner
            .storage
            .read_committed(session_id.clone())
            .await?
        else {
            return Ok(None);
        };
        if committed.id() != &session_id {
            return Err(ConversationError::Storage(StorageError::IdentityMismatch));
        }
        let order = committed
            .snapshot()
            .map(SessionSnapshot::pending_order)
            .transpose()?
            .unwrap_or_default();
        Ok(Some((committed, order)))
    }
    /// The one fold of committed records with no agent: a fresh read-only
    /// projection, no execution active.
    fn detached(
        &self,
        id: &ConversationId,
        committed: &CommittedSession,
        order: &[ExecutionId],
    ) -> Projection {
        let mut projection =
            Projection::new(id.to_string(), ConversationCapabilities::read_only(), None)
                .with_tool_uis(self.inner.tool_uis.clone());
        projection.replace_committed(committed, order, None);
        projection.transcript_state(committed.state().into());
        projection
    }
    /// A view nothing can act on through: the selection from the record,
    /// every capability off, nothing awaiting an answer, bounded.
    async fn read_only(
        &self,
        (mut view, cursor): (ConversationView, Option<CommittedCursor>),
        record: &Conversation,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<(ConversationView, Option<CommittedCursor>), ConversationError> {
        view.selection = Some(ConversationSelectionView {
            agent: record.agent().ok_or(ConversationError::AgentUnsupported)?,
            model: record.model().as_str().into(),
            approval_mode: record.approval_mode(),
        });
        view.capabilities.queue = false;
        view.capabilities.steer = false;
        view.capabilities.resume = false;
        view.capabilities.permissions = false;
        view.capabilities.image_input = false;
        view.permissions.clear();
        view.questions.clear();
        view.title = self.title(id).await;
        self.check_view_access(id, caller).await?;
        Ok((bound_view(view), cursor))
    }
    /// Both journals exclude each other; this is the one pending-view lookup
    /// used before waiting for a control and after explicit recovery.
    async fn pending_change_view(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<Option<(ConversationView, Option<CommittedCursor>)>, ConversationError> {
        if let Some(read) = self.read_pending_model_change(id, caller).await? {
            return Ok(Some(read));
        }
        self.read_pending_mode_change(id, caller).await
    }

    async fn read_pending_mode_change(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<Option<(ConversationView, Option<CommittedCursor>)>, ConversationError> {
        let record = load_conversation(self.inner.metadata.as_ref(), id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        self.access().admit(&record, caller)?;
        let Some(request) = self.inner.metadata.pending_mode_change(id).await? else {
            return Ok(None);
        };
        let request = pending_for(id, &record, request)?;
        let change =
            (request.prior != request.requested).then(|| ConversationApprovalModeChangeView {
                request_id: request.request_id,
                requested_mode: request.requested.as_str().into(),
                status: if request.application.is_some() {
                    ConversationApprovalModeChangeStatus::RecoveryRequired
                } else {
                    ConversationApprovalModeChangeStatus::Changing
                },
            });
        let slot = self.readable_slot(id).await?;
        let live = slot
            .as_ref()
            .and_then(|slot| slot.value.get())
            .and_then(|result| result.as_ref().ok())
            .cloned();
        let committed = self.committed_if_present_with_order(id).await?;
        let read = if let Some((committed, order)) = &committed {
            if let Some(live) = &live {
                let active = live.agent.active_execution_id();
                let mut projection = live.projection.lock().await;
                let accepted = projection.replace_committed(committed, order, active.as_ref());
                if accepted {
                    projection.transcript_state(committed.state().into());
                }
                (
                    projection.read_with_mode_change(change.clone()),
                    projection.committed_cursor(),
                )
            } else {
                let projection = self.detached(id, committed, order);
                (
                    projection.read_with_mode_change(change),
                    projection.committed_cursor(),
                )
            }
        } else {
            if live.is_some() {
                return Err(ConversationError::ApprovalModeUncertain);
            }
            let projection =
                Projection::new(id.to_string(), ConversationCapabilities::read_only(), None);
            (projection.read_with_mode_change(change), None)
        };
        self.pending_selection_read(
            read,
            &record,
            id,
            caller,
            live.as_ref(),
            committed
                .as_ref()
                .and_then(|(committed, _)| committed.snapshot()),
        )
        .await
        .map(Some)
    }
    async fn check_view_access(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<(), ConversationError> {
        let record = load_conversation(self.inner.metadata.as_ref(), id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        self.access().admit(&record, caller)?;
        Ok(())
    }
    /// The title a list shows for the conversation, which a read shows too:
    /// one rule, the summary's, whichever of the two is drawing it
    /// (`a_read_carries_the_title_the_list_shows`). A summary that cannot be
    /// read is logged and the view goes without a title.
    pub(crate) async fn title(&self, id: &ConversationId) -> Option<String> {
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
    /// The command bytes a receipt may name. Invalid input is refused here, before
    /// any durable stage, because a request that was never sent must stay reusable
    /// once its bytes are corrected.
    fn canonical_user_message(
        max_input_bytes: usize,
        execution_id: &str,
        message: SubmittedMessage,
    ) -> Result<(ExecutionId, UserMessage), ConversationError> {
        if message.text.len() > max_input_bytes {
            return Err(ConversationError::InvalidInput);
        }
        let execution =
            ExecutionId::new(execution_id).map_err(|_| ConversationError::InvalidInput)?;
        // Blank text is no text. The message's own rules then decide whether
        // what remains is a message: some text, some images, or both.
        let prompt = (!message.text.trim().is_empty())
            .then(|| PromptText::new(message.text))
            .transpose()
            .map_err(|_| ConversationError::InvalidInput)?;
        let images = message
            .images
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
        let files = message
            .files
            .into_iter()
            .map(|file| LinkedFile::new(file.path).map_err(|_| ConversationError::InvalidInput))
            .collect::<Result<Vec<_>, _>>()?;
        let message =
            UserMessage::new(prompt, images, files).map_err(|_| ConversationError::InvalidInput)?;
        Ok((execution, message))
    }

    /// Refuse a message linking files by path for a conversation that runs
    /// where this machine's files are not its own (an SSH host): before
    /// anything of it is recorded or sent, so the same request, corrected,
    /// can be sent again. Images carry their bytes and are not refused here.
    async fn refuse_linked_files_elsewhere(
        &self,
        id: &ConversationId,
        links_files: bool,
    ) -> Result<(), ConversationError> {
        if links_files && !self.inner.environment.reads_this_machine(id).await? {
            return Err(ConversationError::LinkedFileUnreachable);
        }
        Ok(())
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
        self.submit_as(
            id,
            caller,
            execution_id,
            message,
            mode,
            Writer::Person(Initiation::owner(), true),
        )
        .await
        .map_err(SubmitFailure::into_error)
    }
    /// [`Self::submit`], written by `writer`. A message admitted while the
    /// conversation is idle — nothing runs, nothing waits — takes the
    /// contexts its apps hold, under the submission lock, and carries them
    /// ahead of what it says: they leave their mounts there and then. If it
    /// is then refused, or fails before the agent was asked, they went
    /// nowhere, and each is reported dropped as the task's
    /// [`Taken`](super::app_reviews::Taken) goes (`ContextDropped{not_sent}`,
    /// by the system); if whether the agent has it is unknown, they follow
    /// it, with no drop recorded. A message refused before the take, queued
    /// behind a turn or steered into one takes none, and leaves them held
    /// (`docs/design/mcp-app-calls.md`, rows C9–C11b).
    ///
    /// An app's message is refused [`ConversationError::TurnRunning`] while a
    /// turn runs or input waits, so it never queues behind the person's own
    /// or fills the queue
    /// (`m11_an_apps_message_waits_for_nobody_it_is_refused_while_a_turn_runs`).
    ///
    /// What M10 guarantees of an app's message (`docs/design/mcp-app-calls.md`):
    /// one admitted in one opening is never sent into another, and its
    /// submission opens none — a closed conversation is not opened again for
    /// it (`m10_a_message_admitted_in_one_opening_is_not_sent_into_another`,
    /// `m10_a_close_that_took_the_lock_first_refuses_an_allowed_message_and_opens_nothing`).
    ///
    /// A failure says how far the submission got: whether the agent was
    /// asked to take it, so that one refused or failed before then is known
    /// not to have reached it ([`SubmitFailure`]). An app's message refused
    /// before then, for whatever reason, while its own apps say its opening
    /// had ended or its mount been released when it was refused, is refused
    /// as that ([`McpAppError::Cancelled`], row M10): the stop or release
    /// came first
    /// (`m10_a_stop_that_ended_the_messages_opening_before_the_enqueue_is_m10_by_the_system`).
    /// One that came after the refusal leaves it as it was
    /// (`m10_a_release_after_a_turn_running_refusal_keeps_turn_running`).
    /// Admit a turn `grant` sends to `live`, or refuse it
    /// `DrivePolicyUnenforceable` (row J26): its agent must ask about every
    /// call outside its Ask-mode floor (Claude or Codex, not OpenCode), run
    /// in the Ask approval mode, attached, with native hooks off, and the
    /// grant's policy at the admitted revision must be read from its journal.
    /// The policy is kept for the conversation, where its calls are checked.
    async fn admit_drive_turn(
        &self,
        live: &LiveConversation,
        id: &ConversationId,
        grant: &GrantInitiator,
        non_default_mode: bool,
    ) -> Result<(), ConversationError> {
        self.check_drive_turn(live, id, grant, non_default_mode)
            .await
            .unwrap_or(Err(ConversationError::DrivePolicyUnenforceable))
    }

    /// [`Self::admit_drive_turn`]'s verdict, or `Err` when the grant's policy
    /// could not be read at all: a resume tells "this grant no longer admits
    /// it" from "not known now", and leaves the second for the next try.
    async fn check_drive_turn(
        &self,
        live: &LiveConversation,
        id: &ConversationId,
        grant: &GrantInitiator,
        non_default_mode: bool,
    ) -> Result<Result<(), ConversationError>, ConversationError> {
        let refused = Ok(Err(ConversationError::DrivePolicyUnenforceable));
        let Some(drive) = self.inner.drive.get() else {
            return refused;
        };
        if live.tool_source.is_none() || non_default_mode {
            return refused;
        }
        // What the agent asks about is decided as it attaches: its approval
        // mode and whether its own hooks may answer first.
        let _ = live.join_attachment_owner().await;
        if live.agent.attachment_status().phase() != AttachmentPhase::Attached
            || live.agent.approval_mode() != Some(ProviderApprovalMode::Ask)
            || live.agent.operation_capabilities().native_hooks_allowed()
        {
            return refused;
        }
        match drive
            .grants
            .policy_at(id, grant.grant(), grant.policy_revision())
            .await
        {
            Ok(Some(policy)) => {
                live.admitted_policies
                    .admit(grant.grant(), grant.policy_revision(), policy);
                Ok(Ok(()))
            }
            Ok(None) => refused,
            Err(error) => Err(error),
        }
    }

    /// Bind what checks a Drive grant's tools. A second bind is ignored;
    /// unbound, no Drive turn is admitted.
    pub fn bind_drive_tools(&self, drive: DriveTools) {
        let _ = self.inner.drive.set(drive);
    }

    /// Bind the remembered permission rules. A second bind is ignored.
    /// Unbound, reviews are asked and no remember choice is offered.
    pub fn bind_permission_rules(&self, rules: Arc<dyn PermissionRules>) {
        let _ = self.inner.permission_rules.set(rules);
    }

    /// Bind the deletion inventory recorded after a tombstone commits. A
    /// second bind is ignored. Unbound, a delete still fences and finishes.
    pub fn bind_fence_capture(&self, capture: Arc<dyn FenceCapture>) {
        let _ = self.inner.fence_capture.set(capture);
    }

    pub(super) async fn submit_as(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        execution_id: String,
        message: SubmittedMessage,
        mode: SubmissionMode,
        writer: Writer,
    ) -> Result<SubmissionReceipt, SubmitFailure> {
        self.submit_admitted_as(id, caller, execution_id, message, mode, writer, false)
            .await
    }
    /// [`Self::submit_as`] for a caller already holding admission for the
    /// whole call (`held`), which is not asked for again. Tokio's lock is
    /// fair: a second read queues behind a waiting retirement, which waits on
    /// the first, and neither moves until retirement gives up waiting.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn submit_admitted_as(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        execution_id: String,
        message: SubmittedMessage,
        mode: SubmissionMode,
        writer: Writer,
        held: bool,
    ) -> Result<SubmissionReceipt, SubmitFailure> {
        // A message that reaches an agent between a failed turn and its
        // recovery is refused with nothing taken (`Halt::Again`): the agent
        // is stopping, or would run it at a mode other than the
        // conversation's. It is offered again until the agent has recovered,
        // or is found stuck and the conversation opened again, for as long
        // as a stop may take. Every offer carries the same message identity,
        // so one the agent did take is answered with its own delivery
        // (`messages_sent_while_a_failed_turn_settles_are_not_refused_for_the_mode`).
        // What an offer took from the apps and was refused with is offered
        // again with the message, not reported dropped and then missing
        // from it; should no offer be taken, it is reported dropped unsent
        // as this goes (row C11).
        let deadline = std::time::Instant::now() + self.inner.deletion_budgets.stop;
        let carried = Carried::default();
        let (asked, outcome) = loop {
            let (asked, submission) = self.submission_future(
                id.clone(),
                caller.clone(),
                execution_id.clone(),
                message.clone(),
                mode,
                writer.clone(),
                held,
                carried.clone(),
            );
            let outcome = tokio::spawn(submission).await;
            if !matches!(outcome, Ok(Err(Halt::Again(_)))) || std::time::Instant::now() >= deadline
            {
                break (asked, outcome);
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        let asked = asked.load(Ordering::SeqCst);
        let failure = match outcome {
            Ok(Ok(receipt)) => return Ok(receipt),
            // Only before anything is asked: admission, and the resolve.
            Ok(Err(Halt::Retired)) => SubmitFailure::Retired,
            Ok(Err(Halt::Failed(error) | Halt::Again(error))) if asked => {
                SubmitFailure::Asked(error)
            }
            Ok(Err(Halt::Failed(error) | Halt::Again(error))) => SubmitFailure::NotAsked(error),
            Err(_) => SubmitFailure::TaskFailed { asked },
        };
        Err(failure)
    }
    /// The submission task, shared by normal supervision and controlled polling
    /// in the desktop-stop ordering test. Refusal attribution stays on this task.
    #[allow(clippy::too_many_arguments)]
    fn submission_future(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        execution_id: String,
        message: SubmittedMessage,
        mode: SubmissionMode,
        writer: Writer,
        held: bool,
        carried: Carried,
    ) -> (
        Arc<AtomicBool>,
        impl Future<Output = Result<SubmissionReceipt, Halt>> + Send + 'static,
    ) {
        let service = self.clone();
        // Set just before the agent is asked to take it, on the submission's
        // own task: what the caller reads to know whether it was.
        let asked = Arc::new(AtomicBool::new(false));
        let asking = asked.clone();
        // Who wrote it, read as it is refused (M10).
        let refused_for = writer.clone();
        let asked_when_refused = asked.clone();
        let submission = async move {
            let _admission = if held {
                None
            } else {
                Some(service.admit().await?)
            };
            let _mode = service.inner.mode_changes.lock(&id).await;
            service.recover_mode_change(&id, &caller).await?;
            service.recover_model_change(&id, &caller).await?;
            service.refuse_pending_model_change(&id, &caller).await?;
            let actor = caller.actor()?;
            let (execution, message) = Self::canonical_user_message(
                service.inner.limits.max_input_bytes,
                &execution_id,
                message,
            )?;
            service
                .refuse_linked_files_elsewhere(&id, !message.files().is_empty())
                .await?;
            // An app's message is for a live opening, and opens none: one
            // ended by the desktop stopping the agent is refused below, not
            // opened again for it.
            let (mut owner, mut live) = service
                .resolve_opening(&id, &caller, !writer.is_app())
                .await?;
            // A message is use: an agent is not parked from under it.
            owner.touch(service.inner.clock.unix_milliseconds());
            // Opened again at most once for this message: by the stuck agent
            // it found, or by one that failed while it was being checked.
            let mut reopened = false;
            let (original, saved_initiation, known, non_default_mode) = {
                // An agent that stopped itself and waits for its owner's
                // close takes nothing more (`Agent::awaits_close`): the
                // conversation is stopped, not gone, so a person's message
                // closes it and opens the conversation again, as a desktop
                // stop would have. So is one whose attachment failed, or that
                // lost the conversation's approval mode with it
                // (`resumption::stuck`): it would open again at the mode it
                // was first opened with, and the check below would refuse
                // every message for as long as it lived. Its failed turn and
                // what was queued behind it settle first, as they do before
                // the gateway's own retry closes it; a message that came
                // while they settle waits for that rather than being refused
                // (`messages_sent_while_a_failed_turn_settles_are_not_refused_for_the_mode`).
                // An app's message opens nothing, and is refused below as
                // before.
                if !reopened
                    && !writer.is_app()
                    && !owner.stopping.load(Ordering::SeqCst)
                    && (live.agent.awaits_close()
                        || (resumption::stuck(&live).await && service.settles(&live).await))
                {
                    // The ended agent holds its history's lease until the last
                    // handle goes, so this one goes before the conversation
                    // opens again.
                    drop(live);
                    service.reopen_ended(&id, owner).await;
                    (owner, live) = service.resolve_opening(&id, &caller, true).await?;
                    reopened = true;
                }
                // A submission the agent already has is the agent's to answer:
                // the same message recovers its original delivery and any
                // other is a conflict, whatever this conversation holds today.
                // Its images were checked when it was first accepted. Asking
                // again would turn a retry of a delivered turn into "not
                // found" once its upload was let go, where the same retry of a
                // text turn succeeds. A retry carries what its saved record
                // holds, never what is held now: the agent compares it with
                // the message it has
                // (`c12_a_retry_of_a_message_the_agent_has_carries_what_its_record_holds`).
                let original = live
                    .agent
                    .session_manager()
                    .snapshot()
                    .await
                    .and_then(|snapshot| {
                        snapshot
                            .invocations
                            .iter()
                            .find(|record| record.request.execution_id == execution)
                            .map(|record| (record.request.clone(), record.initiation.clone()))
                    });
                let (original, saved_initiation) = original.unzip();
                let known = original.is_some();
                // Asked under the submission lock, which a desktop stop takes
                // to mark the owner: an owner being stopped is handed no new
                // message (`an_owner_marked_as_stopping_is_handed_no_message`,
                // #528), before anything about it is recorded. A message it
                // already has is still answered with its own delivery
                // (`a_retry_of_a_message_the_stopping_owner_has_recovers_its_delivery`).
                if owner.stopping.load(Ordering::SeqCst) && !known {
                    return Err(ConversationError::Agent(AgentError::Closed).into());
                }
                let non_default_mode = live
                    .projection
                    .lock()
                    .await
                    .view()
                    .selection
                    .as_ref()
                    .is_some_and(|selection| {
                        selection.approval_mode != ConversationApprovalMode::Ask
                    });
                if (non_default_mode
                    || service
                        .inner
                        .metadata
                        .requires_mode_verification(&id)
                        .await?)
                    && !mode_holds(&live).await
                {
                    // Its attachment failed, lost its mode, or is opening
                    // again at the agent's first mode, after it was looked at
                    // above, or it has not settled yet: offered again (see
                    // `submit_as`), it is found settled and stuck, and the
                    // conversation opened again.
                    if !reopened && !writer.is_app() && !owner.stopping.load(Ordering::SeqCst) {
                        return Err(Halt::Again(ConversationError::ApprovalModeNotApplied));
                    }
                    return Err(ConversationError::ApprovalModeNotApplied.into());
                }
                (original, saved_initiation, known, non_default_mode)
            };
            // Checked above, and again by the agent as it admits the turn:
            // its attachment can fail in between, and a turn admitted then
            // would run at the mode the agent was opened with.
            let required_mode = if non_default_mode
                || service
                    .inner
                    .metadata
                    .requires_mode_verification(&id)
                    .await?
            {
                live.projection
                    .lock()
                    .await
                    .view()
                    .selection
                    .as_ref()
                    .map(|selection| provider_approval_mode(selection.approval_mode))
            } else {
                None
            };
            // A Drive grant's turn is admitted only where every call it asks
            // about is checked against the grant's policy, and is saved as
            // checked (rows J26 and J27).
            // A retry by the same initiator is answered from what was saved,
            // checked or not, and is not admitted again; one by another is
            // the agent's conflict to refuse.
            let retried = saved_initiation
                .filter(|saved| saved.initiator() == writer.initiation().initiator());
            // Read before `retried` moves into `initiation`. A saved turn's
            // retry is not checked again.
            let fresh = retried.is_none();
            let initiation = match (retried, writer.initiation().initiator()) {
                (Some(saved), _) => saved,
                (None, Initiator::Owner) => Initiation::owner(),
                (None, Initiator::Grant(grant)) => {
                    service
                        .admit_drive_turn(&live, &id, grant, non_default_mode)
                        .await?;
                    Initiation::checked_grant(grant.clone())
                }
            };
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
                        return Err(ConversationError::AttachmentNotFound.into());
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
            // A saved request owns its admission budget after model changes.
            // The incoming message is replaced below so SDK recovery still
            // checks content, actor, initiation and operation conflicts.
            let request = if let Some(original) = original {
                original
            } else {
                // ACP owns hidden context/tokenization. Fresh submissions
                // conservatively reserve the verified effective context budget.
                let capabilities = live.agent.capabilities();
                let limits = capabilities.limits();
                let budget = live.model_budget.lock().await;
                if budget.identity != live.agent.provider_identity() {
                    return Err(ConversationError::ModelChangeUncertain.into());
                }
                let reserved = budget.reserved_output_tokens;
                drop(budget);
                if reserved > limits.max_output() || reserved >= limits.max_context_window() {
                    return Err(ConversationError::InvalidInput.into());
                }
                ExecutionRequest {
                    execution_id: execution.clone(),
                    user_message: message.clone(),
                    estimated_input_tokens: u64::from(limits.max_context_window() - reserved),
                    reserved_output_tokens: reserved,
                }
            };
            // Just before the enqueue, still under the submission lock, which
            // every submission to this conversation takes: an app's message is
            // for the opening it was admitted in, its mount not released
            // (`m10_a_release_before_the_submission_lock_refuses_the_message`).
            writer.still_admitted(&live)?;
            // Idle: nothing runs and nothing waits, so a message admitted now
            // starts at once, with nothing ahead of it to reorder or remove.
            // Only such a message carries what apps hold
            // (`c10_a_message_queued_behind_a_turn_carries_no_context_and_leaves_it_held`),
            // and an app's own message is only ever admitted so
            // (`m11_an_apps_message_waits_for_nobody_it_is_refused_while_a_turn_runs`).
            // A retry is the agent's to settle, whatever runs now.
            let idle = !known && live.agent.idle_for_approval_change().await;
            if writer.is_app() && !known && !idle {
                return Err(ConversationError::TurnRunning.into());
            }
            // Past every refusal that takes nothing (M10's recheck above, and
            // `turn_running`): a message admitted while idle takes what is
            // held, and from here they are its own. A release or an end finds
            // them gone, and a mount's newer update is held anew
            // (`c9_a_message_takes_the_contexts_and_a_release_meanwhile_drops_none`).
            // What it took is this task's own: should the task end, however,
            // before the agent was asked, each is reported dropped unsent as
            // it goes (row C11).
            // An earlier offer of this message to this same agent, refused
            // to be offered again, took them already: they are offered again
            // as they were. One taken from an agent since replaced, or not
            // carried now, is reported dropped unsent as it goes.
            let mut taken = None;
            let offered_before = carried
                .lock()
                .expect("carried contexts")
                .take()
                .filter(|(from, _)| std::sync::Weak::ptr_eq(from, &Arc::downgrade(&live)))
                .map(|(_, took)| took);
            let contexts = match known {
                true => request.user_message.app_model_context().to_vec(),
                false if idle => {
                    let took = offered_before.unwrap_or_else(|| live.app_reviews.take_held());
                    let contexts = took.contexts();
                    taken = Some(took);
                    contexts
                }
                false => Vec::new(),
            };
            let message = message
                .sent_by(writer.sender())
                .with_app_model_context(contexts)
                .map_err(|_| ConversationError::InvalidInput)?;
            if let Some(rules) = service.inner.permission_rules.get() {
                rules.arm(
                    &id.to_string(),
                    &execution,
                    writer.offers_remember() && matches!(initiation.initiator(), Initiator::Owner),
                );
            }
            let request = ExecutionRequest {
                user_message: message.clone(),
                ..request
            };
            // A fresh Drive turn names the grant and revision admission
            // read. The note verdict is taken before that grant is held, so
            // a slow verdict does not block a revoke. A retry of a turn
            // already saved is answered from that save and is not checked
            // again (row J31).
            let fresh_grant = if fresh {
                match initiation.initiator() {
                    Initiator::Grant(grant) => {
                        Some((grant.grant().to_owned(), grant.policy_revision()))
                    }
                    Initiator::Owner => None,
                }
            } else {
                None
            };
            // A person's queued message may start the next turn, so it may
            // take the notes waiting for one: each author is checked now, and
            // the agent decides under its own lock whether the turn takes any
            // (only when admitted while idle) and which of those still
            // allowed (row J23). Never a steer, an app's message, or a retry.
            let notes = if offers_notes(known, writer.is_app(), &mode) {
                Some(service.note_candidates(&id, &live).await)
            } else {
                None
            };
            // The hold is this grant only. Taken before the agent is asked,
            // so a revoke that already landed is `NotAsked`. A queued turn
            // keeps it until enqueue returns. A native steer moves it into
            // the save check, which drops the first permit after the save,
            // compares the live revision, and keeps that second permit until
            // the provider operation starts (row J31).
            let mut revision_hold = None;
            let mut saved_grant = None;
            if let Some((receiver, revision)) = fresh_grant {
                let grants = Arc::clone(
                    &service
                        .inner
                        .drive
                        .get()
                        .ok_or(ConversationError::DrivePolicyUnenforceable)?
                        .grants,
                );
                revision_hold = Some(grants.hold_revision(&id, &receiver, revision).await?);
                saved_grant = Some((grants, receiver, revision));
            }
            // Marked before the agent is asked, so a restart from here on
            // finds this conversation and resumes the turn (`resumption`).
            service.mark_unfinished(&id).await?;
            // Asked from here: whatever becomes of this task, what it took
            // follows the message, and only the agent's own refusal hands
            // it back (C11b).
            if let Some(took) = taken.as_mut() {
                took.asking();
            }
            asking.store(true, Ordering::SeqCst);
            // Saved as started by whom it was admitted for: the owner, or
            // the Drive grant as it stood at admission, its tools checked
            // (rows J15 and J27).
            let mut submitter = Submitter::new(actor, initiation);
            if let Some(mode) = required_mode {
                submitter = submitter.with_approval_mode(mode);
            }
            if matches!(mode, SubmissionMode::Steer)
                && live.agent.operation_capabilities().native_steering()
            {
                if let (Some(hold), Some((grants, receiver, revision))) =
                    (revision_hold.take(), saved_grant)
                {
                    submitter = submitter.with_after_save(Arc::new(ConfirmSavedGrant {
                        hold: std::sync::Mutex::new(Some(hold)),
                        grants,
                        id: id.clone(),
                        receiver,
                        revision,
                    }));
                }
            }
            let delivery = match (mode, notes) {
                (SubmissionMode::Queue, Some(candidates)) => live
                    .agent
                    .enqueue_with_notes(request, submitter, candidates)
                    .await
                    .map(SubmissionDelivery::Queued),
                (SubmissionMode::Queue, None) => live
                    .agent
                    .enqueue(request, submitter)
                    .await
                    .map(SubmissionDelivery::Queued),
                (SubmissionMode::Steer, _)
                    if !live.agent.operation_capabilities().native_steering() =>
                {
                    live.agent
                        .enqueue_steering(request, submitter)
                        .await
                        .map(SubmissionDelivery::Queued)
                }
                (SubmissionMode::Steer, _) => {
                    live.agent
                        .steer(request, submitter)
                        .await
                        .map(|delivery| match delivery {
                            SteeringDelivery::Queued(receipt) => {
                                SubmissionDelivery::Queued(receipt)
                            }
                            SteeringDelivery::Injected { evidence, .. } => {
                                SubmissionDelivery::Injected { evidence }
                            }
                        })
                }
            };
            // Enqueue kept the permit through the save and through starting
            // the runner, and drops it here, before the turn is watched. A
            // native steer released its second permit when the provider
            // operation started, before the response wait. A revoke of this
            // grant applies from here, not across the turn itself.
            drop(revision_hold);
            // Refused: what it took went nowhere, and each is reported
            // dropped unsent (`c11_a_refused_submission_drops_what_it_took_on_record`).
            // Admitted, and saved before the agent answered: what it carried
            // went with it, and should the turn then fail, it is lost (C13).
            // A steer refused or left unchecked after that save is the same:
            // the contexts stay with the saved turn
            // (`c13_a_saved_steer_refused_after_save_does_not_drop_the_contexts_it_carried`,
            // `c13_a_saved_steer_unavailable_after_save_does_not_drop_the_contexts_it_carried`).
            // Unresolved: it follows the message, whose record covers it.
            let delivery = delivery.map_err(|error| match error {
                AgentError::RefusedAfterSave => ConversationError::RevokedAfterSave,
                AgentError::AfterSaveUnavailable => ConversationError::UnavailableAfterSave,
                AgentError::ApprovalModeChanged => ConversationError::ApprovalModeNotApplied,
                other => ConversationError::Agent(other),
            });
            // Refused for now, to be offered again: what it took goes with
            // the next offer (`a_message_offered_again_carries_the_contexts_it_took`).
            let again = matches!(
                delivery,
                Err(ConversationError::ApprovalModeNotApplied
                    | ConversationError::Agent(AgentError::Closed))
            ) && !writer.is_app()
                && !owner.stopping.load(Ordering::SeqCst);
            if let (Err(error), Some(mut took)) = (&delivery, taken) {
                if Reach::of_asked(error) == Reach::NotTaken {
                    took.handed_back();
                    if again {
                        *carried.lock().expect("carried contexts") =
                            Some((Arc::downgrade(&live), took));
                    }
                }
            }
            let delivery = match delivery {
                Err(error) if again => return Err(Halt::Again(error)),
                delivery => delivery?,
            };
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
                        return Err(error.into());
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
                SubmissionDelivery::Injected { evidence, .. } => {
                    if let SteeringEvidence::Failed(failure) = &evidence {
                        return Err(admission_evidence_error(failure).into());
                    }
                    Ok(SubmissionReceipt {
                        execution_id,
                        disposition: ConversationDisposition::Injected,
                    })
                }
            }
        };
        let outcome = async move {
            let submitted: Result<SubmissionReceipt, Halt> = submission.await;
            // An app's message refused before the agent was asked, while its
            // own apps say its opening had ended or its mount been released:
            // the stop or the release came first, whatever the refusal says
            // (row M10). Read here, on its own task, as it is refused — not
            // once its caller has the answer, when a release that came after
            // a refusal for its own reason would take that reason's place
            // (`m10_a_release_after_a_turn_running_refusal_keeps_turn_running`).
            match submitted {
                Err(Halt::Failed(_) | Halt::Again(_))
                    if !asked_when_refused.load(Ordering::SeqCst)
                        && refused_for.opening_ended() =>
                {
                    Err(ConversationError::McpApp(McpAppError::Cancelled).into())
                }
                submitted => submitted,
            }
        };
        (asked, outcome)
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
        if let Some(_result) = completion.as_mut().now_or_never() {
            self.wake_thread_reports(conversation);
            let snapshot = live.agent.session_manager().snapshot().await;
            let reply = completed_reply(snapshot.as_ref(), &id).filter(|_| new_submission);
            // Decided once whoever submitted it lets the conversation go.
            let service = self.clone();
            let settled_in = conversation.clone();
            let settled = live.clone();
            tokio::spawn(async move { service.after_settle(&settled_in, settled).await });
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
            let _result = completion.await;
            // The SDK receipt follows final settlement, unlike invocation hooks.
            // Wake reports before summaries or resumption can wait on other work.
            service.wake_thread_reports(&conversation);
            let snapshot = live.agent.session_manager().snapshot().await;
            let reply = completed_reply(snapshot.as_ref(), &id);
            live.watched.lock().await.remove(&id);
            // As above: the agent is let go of before the summary lock is
            // waited for, so a watcher cannot hold a stopped agent's history
            // leased past a delete's wait for it.
            service.after_settle(&conversation, live).await;
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
    /// Read from [`ConversationListing`], which reads only this caller's
    /// conversations and at most one more than the bound, and from what is
    /// live on this gateway for those it returns. Nothing is resolved: no
    /// provider is opened and no session storage is opened to draw a list
    /// (`listing_shows_only_the_callers_conversations_and_opens_nothing`).
    /// What a list costs is the caller's own conversations, never the
    /// gateway's (`a_list_reads_only_the_callers_conversations_however_many_others_there_are`).
    ///
    /// A conversation with no summary is left out: nothing was ever said in
    /// it — one opened only to hold an upload, say — and a list of what was
    /// said has nothing to show for it
    /// (`a_conversation_nothing_was_said_in_is_not_listed`). Only something
    /// said makes a summary — archiving makes none
    /// (`archiving_a_conversation_nothing_was_said_in_changes_nothing`) — so
    /// "has a summary" and "something was said" are the one rule. That includes
    /// conversations from before summaries were kept, which are not given a
    /// summary after the fact. A conversation naming an agent this build
    /// cannot open is listed like any other: it is its owner's to see and to
    /// delete. The bound is applied after ownership and the archive filter
    /// (`the_archive_filter_applies_before_the_bound`), and the list says
    /// whether it is whole ([`ConversationList::complete`]): a list exactly at
    /// the bound with nothing behind it is complete
    /// (`a_list_says_whether_the_bound_left_any_out`), and one that met a
    /// record or summary of this caller's it could not read is not — only
    /// this caller's, since nobody else's is read
    /// (`an_unreadable_row_makes_its_owners_list_incomplete_and_nobody_elses`).
    pub async fn list(
        &self,
        caller: ConversationCaller,
        archived: bool,
    ) -> Result<ConversationList, ConversationError> {
        let _admission = self.admit().await?;
        caller.actor()?;
        // One past the bound, so the list can say whether it left any out.
        let listed = self
            .inner
            .listing
            .list(
                &caller.organization_id,
                &caller.principal_id,
                archived,
                MAX_LISTED_CONVERSATIONS + 1,
            )
            .await?;
        let complete =
            listed.unreadable == 0 && listed.conversations.len() <= MAX_LISTED_CONVERSATIONS;
        // Found by the listing, decided by the access policy: a row it
        // returns that is not this caller's, or is deleted, is not shown
        // whatever listing is wired here
        // (`a_list_shows_only_what_the_domain_lets_the_caller_see`).
        let owned: Vec<ListedConversation> = self
            .admitted(listed.conversations, &caller)?
            .into_iter()
            .take(MAX_LISTED_CONVERSATIONS)
            .collect();
        let running = self.running_ids(&owned).await;
        Ok(ConversationList {
            conversations: list_entries(&owned, &running),
            complete,
        })
    }
    /// One page of every summary this caller owns under `archived`, in
    /// creation order, from [`ConversationListing::observe`].
    ///
    /// The newest-first list stays bounded by [`MAX_LISTED_CONVERSATIONS`].
    /// This walks the catalogue page instead, and still opens no provider
    /// (`observing_every_stored_summary_opens_nothing`). A page that is not
    /// [`ConversationObservation::complete`] is not the whole catalogue. The
    /// running flag is the same question [`Self::list`] asks, and only for
    /// the rows this page returns.
    pub async fn observe(
        &self,
        caller: ConversationCaller,
        archived: bool,
        cursor: Option<ConversationObservationCursor>,
    ) -> Result<ConversationObservation, ConversationError> {
        let _admission = self.admit().await?;
        caller.actor()?;
        let cursor = match cursor {
            None => None,
            Some(cursor) => Some(ObservationCursor {
                incarnation: cursor.incarnation,
                boundary: cursor.boundary,
                creation: cursor.creation,
                id: ConversationId::new(&cursor.id).map_err(|_| ConversationError::InvalidInput)?,
            }),
        };
        let observed = self
            .inner
            .listing
            .observe(
                &caller.organization_id,
                &caller.principal_id,
                archived,
                cursor,
            )
            .await?;
        let ObservedConversations {
            conversations,
            complete,
            cursor,
        } = observed;
        let owned: Vec<ListedConversation> = self.admitted(conversations, &caller)?;
        let running = self.running_ids(&owned).await;
        Ok(ConversationObservation {
            conversations: list_entries(&owned, &running),
            complete,
            cursor: cursor.map(|cursor| ConversationObservationCursor {
                incarnation: cursor.incarnation,
                boundary: cursor.boundary,
                creation: cursor.creation,
                id: cursor.id.to_string(),
            }),
        })
    }
    /// The access policy, as every command's admission asks it.
    fn access(&self) -> OwnerAccess<'_> {
        OwnerAccess::new(self.inner.access_policy.as_ref())
    }
    /// The listed conversations `caller` may act on, in order: a listing is
    /// an index, and each row it found is admitted again here, rows that
    /// differ only in what the policy never sees sharing one decision
    /// ([`OwnerAccess::admits_each`]). A policy that cannot decide fails the
    /// list rather than shortening it.
    fn admitted(
        &self,
        listed: Vec<ListedConversation>,
        caller: &ConversationCaller,
    ) -> Result<Vec<ListedConversation>, ConversationError> {
        let admitted = self
            .access()
            .admits_each(listed.iter().map(|row| &row.conversation), caller)?;
        Ok(listed
            .into_iter()
            .zip(admitted)
            .filter_map(|(row, admitted)| admitted.then_some(row))
            .collect())
    }
    /// Which of `owned` are open here with a turn under way.
    ///
    /// Only a conversation that finished opening has anything to say about
    /// running; one still opening, or whose opening failed, is not waited on.
    /// Asked only after the rows are in hand, and let go of before the answer
    /// is: a list waiting on storage holds nothing live, so it never keeps a
    /// stopped agent, or its history's lease
    /// (`a_list_waiting_on_its_listing_does_not_keep_a_deleted_history_leased`).
    async fn running_ids(&self, owned: &[ListedConversation]) -> HashSet<ConversationId> {
        let live: HashMap<ConversationId, Arc<LiveConversation>> = {
            let owners = self.inner.conversations.lock().await;
            owned
                .iter()
                .filter_map(|listed| {
                    let id = listed.conversation.id();
                    let slot = owners.get(id)?;
                    let live = slot.value.get()?.as_ref().ok()?;
                    Some((id.clone(), live.clone()))
                })
                .collect()
        };
        let mut running = HashSet::new();
        for (id, live) in live {
            if turn_in_progress(live.agent.session_manager().snapshot().await) {
                running.insert(id);
            }
        }
        running
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
    /// settling after the delete cannot put the summary back
    /// (`a_late_reply_cannot_write_back_a_deleted_summary`).
    async fn change_summary(
        &self,
        id: &ConversationId,
        change: impl FnOnce(Option<&ConversationSummary>) -> Option<ConversationSummary>,
    ) {
        let _writes = self.inner.summary_writes.lock(id).await;
        match load_conversation(self.inner.metadata.as_ref(), id).await {
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
            return;
        }
        // A view shows the summary's title.
        self.inner.live_changes.publish(id);
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
            if result == QueueRemoval::Removed {
                service.wake_thread_reports(&id);
            }
            Ok(result == QueueRemoval::Removed)
        })
        .await
    }
    /// Answer one question the agent asked, or decline it.
    ///
    /// `choices` of `None` declines: the agent is told, and carries on. Nothing
    /// is authorised here — a question is not a review — so there is no
    /// selection state to report back, only whether the answer was written.
    pub async fn answer_question(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        execution: String,
        question: String,
        key: String,
        choices: Option<Vec<QuestionChoiceInput>>,
    ) -> Result<(), ConversationError> {
        let service = self.clone();
        supervised(async move {
            let _admission = service.admit().await?;
            // Kept, not merely checked: who answered is the one fact an audit of
            // an explicit answer exists to record.
            let actor = caller.actor()?;
            let execution_id =
                ExecutionId::new(&execution).map_err(|_| ConversationError::InvalidInput)?;
            let question_id =
                QuestionId::new(&question).map_err(|_| ConversationError::InvalidInput)?;
            let choices = choices
                .map(|choices| {
                    choices
                        .into_iter()
                        .map(|choice| {
                            QuestionChoice::new(choice.key, choice.values, choice.own_words)
                                .map_err(|_| ConversationError::InvalidInput)
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?;
            let live = service.resolve_holding_reviews(&id, &caller).await?;
            // Only the question the person was shown: one asked again under
            // the same identities has a key of its own, and the agent that
            // holds it refuses another as stale.
            live.agent
                .answer_question(QuestionAnswer {
                    key: Some(key),
                    actor,
                    execution_id,
                    id: question_id,
                    choices,
                })
                .await
                .map_err(|error| match error {
                    AgentError::StalePermission => error.into(),
                    _ => ConversationError::Unavailable,
                })
        })
        .await
    }
    // The review is named three ways, each checked: its execution, its id,
    // and the key it was shown under.
    #[allow(clippy::too_many_arguments)]
    pub async fn answer(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        execution: String,
        permission: String,
        review: String,
        option: String,
        permission_caller: PermissionCaller,
    ) -> Result<(), ConversationError> {
        let service = self.clone();
        supervised(async move {
            let _admission = service.admit().await?;
            let actor = caller.actor()?;
            let execution_id =
                ExecutionId::new(&execution).map_err(|_| ConversationError::InvalidInput)?;
            let permission_id =
                PermissionId::new(&permission).map_err(|_| ConversationError::InvalidInput)?;
            // Malformed input is refused before the conversation is opened
            // (`malformed_controls_do_not_open_a_dormant_owned_provider`).
            let option_id = PermissionOptionId::new(option.as_str())
                .map_err(|_| ConversationError::InvalidInput)?;
            if is_remember_option(option_id.as_str()) {
                if let Some(rules) = service.inner.permission_rules.get() {
                    rules.note_answerer(
                        &id.to_string(),
                        &execution_id,
                        actor.request_id(),
                        permission_caller,
                    );
                }
            }
            let live = service.resolve_holding_reviews(&id, &caller).await?;
            // An app's review first: it is the gateway's own, not the agent's.
            match live.app_reviews.answer(
                &execution,
                &permission,
                &review,
                &option,
                answerer(&caller),
            ) {
                ReviewAnswer::Ended => return Ok(()),
                ReviewAnswer::Stale => return Err(AgentError::StalePermission.into()),
                ReviewAnswer::NotAnAppReview => {}
            }
            // Only the review the person was shown: the agent refuses another
            // key as stale, and whatever waits under these identities waits on.
            let answer = live
                .agent
                .answer_permission(PermissionAnswer {
                    key: Some(review),
                    attribution: ApprovalAttribution::new(actor, ApprovalBasis::Explicit),
                    execution_id,
                    id: permission_id,
                    option_id,
                })
                .await;
            // Permission selection changes live authority without an execution
            // event. A consumed failure must wake readers just like success.
            service.inner.live_changes.publish(&id);
            answer.map(|_| ()).map_err(|failure| {
                let (error, selection) = failure.into_parts();
                ConversationError::PermissionAnswer { error, selection }
            })
        })
        .await
    }
    /// The waiting review a view lists under `review`, with all it shows,
    /// from its live owner: the app review store, or the agent's live
    /// invocation. `StalePermission` once nothing live holds it.
    pub(crate) async fn waiting_review(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        review: &str,
    ) -> Result<ConversationPermission, ConversationError> {
        let _admission = self.admit().await?;
        let stale = || ConversationError::from(AgentError::StalePermission);
        let Some(live) = self.live_now(&id, &caller).await? else {
            return Err(stale());
        };
        if let Some(shown) = live.app_reviews.review(review) {
            return Ok(shown);
        }
        live.agent
            .pending_reviews()
            .unwrap_or_default()
            .iter()
            .find(|pending| pending.content.key() == review)
            .and_then(agent_review)
            .ok_or_else(stale)
    }
    /// One range of what the waiting review a view lists under `review`
    /// shows: the UTF-8 JSON of its [`ConversationPermission`], from the
    /// review's live owner — the app review store, or the agent's live
    /// invocation — never from history, and the review's total length.
    ///
    /// # Errors
    /// `StalePermission` once nothing live holds a review under that key;
    /// `InvalidInput` for an offset past its end.
    pub async fn permission_review(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        review: String,
        offset: u64,
        max_bytes: usize,
    ) -> Result<(Vec<u8>, u64), ConversationError> {
        let shown = self.waiting_review(id, caller, &review).await?;
        range_of(&shown, offset, max_bytes)
    }
    /// The waiting question a view lists under `question`, with all it
    /// asks, from the agent's live invocation. `StalePermission` once
    /// nothing live holds it.
    pub(crate) async fn waiting_question(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        question: &str,
    ) -> Result<ConversationQuestion, ConversationError> {
        let _admission = self.admit().await?;
        let stale = || ConversationError::from(AgentError::StalePermission);
        let Some(live) = self.live_now(&id, &caller).await? else {
            return Err(stale());
        };
        live.agent
            .pending_questions()
            .unwrap_or_default()
            .iter()
            .find(|pending| pending.key() == question)
            .map(agent_question)
            .ok_or_else(stale)
    }
    /// One range of all the waiting question a view lists under `question`
    /// asks: the UTF-8 JSON of its [`ConversationQuestion`], from the
    /// agent's live invocation, never from history, and its total length.
    ///
    /// # Errors
    /// `StalePermission` once nothing live holds a question under that key;
    /// `InvalidInput` for an offset past its end.
    pub async fn question_review(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        question: String,
        offset: u64,
        max_bytes: usize,
    ) -> Result<(Vec<u8>, u64), ConversationError> {
        let shown = self.waiting_question(id, caller, &question).await?;
        range_of(&shown, offset, max_bytes)
    }
    pub async fn cancel_permission(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        execution: String,
        permission: String,
        review: String,
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
            let live = service.resolve_holding_reviews(&id, &caller).await?;
            match live
                .app_reviews
                .cancel(&execution, &permission, &review, answerer(&caller))
            {
                ReviewAnswer::Ended => return Ok(()),
                ReviewAnswer::Stale => return Err(AgentError::StalePermission.into()),
                ReviewAnswer::NotAnAppReview => {}
            }
            let _cancellation = live
                .agent
                .cancel_permission(PermissionCancellationRequest {
                    key: Some(review),
                    execution_id,
                    id: permission_id,
                    reason: PermissionCancellationReason::custom(reason),
                    actor,
                })
                .await?;
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
        let closing = id.clone();
        let closed = supervised(async move {
            let _admission = service.admit().await?;
            let _mode = service.inner.mode_changes.lock(&id).await;
            let actor = caller.actor()?;
            // Whose conversation this is comes from the ownership record, before
            // anything else. Letting go of files must not depend on being able
            // to start a provider, so it cannot depend on `resolve` for this.
            let record = load_conversation(service.inner.metadata.as_ref(), &id)
                .await?
                .ok_or(ConversationError::NotFound)?;
            service.access().admit(&record, &caller)?;
            let pending_mode = service.inner.metadata.pending_mode_change(&id).await?;
            // Its apps' end drops what they still hold, each drop reported
            // to the drop recorder there and then: the close answers its own
            // result, not their records (row C15b).
            let (closed, released) = if pending_mode.is_some() {
                let slot = service.inner.conversations.lock().await.get(&id).cloned();
                match slot {
                    Some(slot) => match service
                        .finish_pending_close(&id, slot, &actor, &initiator_of(&actor), &caller)
                        .await
                    {
                        // Uploads were let go before the slot, inside the close.
                        PendingClose::Closed { uploads } => (Ok(()), uploads),
                        // The carry-on lets the uploads go, then the slot. This
                        // answer does not mean they stay held.
                        PendingClose::Stopped(StopFailure::OverBudget) => {
                            (Err(ConversationError::ApprovalModeUncertain), Ok(()))
                        }
                        // The agent was let go, and only the record of it was
                        // not saved: the carry-on lets the uploads go, then
                        // the slot, and the close answers the failure.
                        PendingClose::Stopped(StopFailure::Failed(error))
                            if confirmed_despite(&error).is_some() =>
                        {
                            (Err(ConversationError::Agent(error)), Ok(()))
                        }
                        PendingClose::Stopped(_) => {
                            if service.inner.attachments.is_some() {
                                tracing::warn!(
                                    conversation_id = %id,
                                    "a conversation that did not close keeps its uploads"
                                );
                            }
                            (Err(ConversationError::ApprovalModeUncertain), Ok(()))
                        }
                    },
                    None => (Ok(()), service.release_closed_uploads(&id, &caller).await),
                }
            } else {
                let (closed, may_release) = match service.resolve_opening(&id, &caller, true).await
                {
                    Ok((slot, live)) => {
                        let result = service
                            .stopping(
                                &id,
                                slot,
                                None,
                                StopBy {
                                    actor: &actor,
                                    ended_by: &initiator_of(&actor),
                                    cause: LeaseEndCause::Closed,
                                    end: AgentEnd::Close,
                                },
                                ConfirmedClose::ReleaseSlot,
                            )
                            .await;
                        // Let go once its cleanup is confirmed, even when the
                        // record of that was not saved: the close answers that
                        // failure, and the next opening accounts for the lease
                        // (`l7_a_close_whose_cleanup_record_cannot_be_saved_says_so_and_the_next_opening_accounts_for_it`).
                        let closed = match &result {
                            Ok(_) => true,
                            Err(error) => confirmed_despite(error).is_some(),
                        };
                        let snapshot = live.agent.session_manager().snapshot().await;
                        // A closed agent runs nothing more, so nothing can still
                        // need the files. An agent that did not close keeps its
                        // saved invocations, and one of those may be a turn that
                        // names images and has not settled.
                        let may_release = closed || !awaits_images(snapshot.as_ref());
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
                    Err(error) => (Err(error.into()), false),
                };
                let released = if may_release {
                    service.release_closed_uploads(&id, &caller).await
                } else if service.inner.attachments.is_some() {
                    tracing::warn!(
                        conversation_id = %id,
                        "a conversation that did not close keeps its uploads"
                    );
                    Ok(())
                } else {
                    Ok(())
                };
                (closed, released)
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
        .await;
        // Turns a closed agent held were cancelled with no invocation to
        // follow. Every close attempt looks, whatever it answers: one that did
        // not close finds nothing new.
        self.wake_thread_reports(&closing);
        closed
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
            let record = load_conversation(service.inner.metadata.as_ref(), &id)
                .await?
                .ok_or(ConversationError::NotFound)?;
            service.access().admit(&record, &caller)?;
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

    /// Rename the conversation: what `list` and a read call it from now on.
    /// A later message does not rename it back
    /// ([`ConversationSummary::after_renaming`]). Returns whether this changed
    /// it; `false` when it already has that title, including a repeat of the
    /// same request, and for a conversation nothing was said in, which is
    /// listed nowhere and is left so, as [`Self::archive`] leaves it.
    ///
    /// A person's decision like archiving, so failing to read or write it
    /// fails the command with the store's typed error
    /// (`a_rename_that_cannot_be_written_fails_visibly`).
    pub async fn rename(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        title: ConversationTitle,
    ) -> Result<bool, ConversationError> {
        let service = self.clone();
        supervised(async move {
            let _admission = service.admit().await?;
            caller.actor()?;
            // Under the summary lock, for the reason `archive` gives.
            let _writes = service.inner.summary_writes.lock(&id).await;
            let record = load_conversation(service.inner.metadata.as_ref(), &id)
                .await?
                .ok_or(ConversationError::NotFound)?;
            service.access().admit(&record, &caller)?;
            let Some(previous) = service.inner.summaries.load(&id).await? else {
                return Ok(false);
            };
            if previous.title() == Some(&title) {
                return Ok(false);
            }
            service
                .inner
                .summaries
                .record(&id, previous.after_renaming(title))
                .await?;
            // The store's write wakes the list; a view shows the title too.
            service.inner.live_changes.publish(&id);
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
            let mode = service.inner.mode_changes.lock(&id).await;
            let fenced = service.fence(&id, &caller).await?;
            // The tombstone now rejects new mode changes. Release the mode
            // admission lock before cleanup so repeat deletes preserve their
            // own ordering under the deletion lock.
            drop(mode);
            // The fence is committed, including when another attempt wrote it.
            // Capture does not take admission, and its failure leaves this
            // delete's result alone.
            service.record_inventory().await;
            let (record, applied, _deleting) = match fenced {
                Fence::Written {
                    record,
                    applied,
                    deleting,
                } => (*record, applied, deleting),
                // Whoever erased it has already told the worker it is done.
                Fence::FinishedByAnother { applied } => {
                    if let Some(record) =
                        load_conversation(service.inner.metadata.as_ref(), &id).await?
                    {
                        if let Some(deletion) = record.deletion() {
                            service
                                .end_deleted_ownership(&id, &deletion_actor(deletion)?)
                                .await?;
                        }
                    }
                    return Ok(applied);
                }
                Fence::LeftByAnother => {
                    return Err(ConversationError::DeletionIncomplete(Box::new(
                        DeletionFailures {
                            another_attempt: true,
                            ..DeletionFailures::default()
                        },
                    )))
                }
            };
            // Who deleted it, read once, from its tombstone — not this
            // caller, who may be repeating someone else's delete
            // (`c15_a_delete_finished_by_a_repeat_drops_as_the_first_deleter`).
            let deleted_by = deleter(&record);
            let finished = service.finish_deletion(record, &deleted_by).await;
            // Its agent stopped — its apps ended by the deleter — or not:
            // either way nothing names it again, and its apps take no more.
            // A context still held is dropped by the deleter, and reported;
            // the delete answers its own result, not the drop's record
            // (row C15b).
            service.close_apps_for_good(&id, &deleted_by);
            match finished {
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

    /// Reconcile the deletion inventory. A refusal or a failed append is
    /// logged. The tombstone is unchanged, and the caller keeps its result.
    async fn record_inventory(&self) {
        let Some(capture) = self.inner.fence_capture.get() else {
            return;
        };
        if let Err(error) = capture.capture().await {
            tracing::warn!(
                ?error,
                "the deletion inventory was not updated after a committed fence"
            );
        }
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
        let record = load_conversation(self.inner.metadata.as_ref(), id)
            .await
            .map_err(RepositoryFailure::from_load)?
            .ok_or(FenceFailure::NotFound)?;
        // The owner deletes their conversation, deleted already or not: a
        // second delete answers from the tombstone the first left.
        match self
            .access()
            .locates(&record, &caller.organization_id, &caller.principal_id)
        {
            Ok(true) => {}
            Ok(false) => return Err(FenceFailure::NotFound),
            Err(_) => return Err(FenceFailure::Repository(RepositoryFailure::Metadata)),
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
            let current = load_conversation(self.inner.metadata.as_ref(), id)
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
            let _creation = self.inner.creation.lock(id).await;
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
    /// it has, once the service is retired. Tombstones that could not be read
    /// are counted too: a deletion nothing here can see, let alone finish.
    /// Only unfinished deletions are read, not every conversation
    /// (`unfinished_deletions_are_read_by_their_index_and_an_unnamed_one_is_counted`).
    ///
    /// # Errors
    /// The repository's error when the records cannot be enumerated at all.
    pub async fn finish_deletions(&self) -> Result<DeletionsLeft, ConversationError> {
        let UnfinishedDeletions {
            conversations: unfinished,
            unreadable,
        } = self.inner.metadata.unfinished_deletions().await?;
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
            let record = load_conversation(service.inner.metadata.as_ref(), &id)
                .await
                .map_err(RepositoryFailure::from_load)?
                .ok_or(ConversationError::NotFound)?;
            let deleted_by = deleter(&record);
            service.finish_deletion(record, &deleted_by).await?;
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
    ///    history and the summary (under the summary lock) only once the
    ///    record is acknowledged. A history this build cannot read is
    ///    discarded then, without replaying it. A history that opened is
    ///    erased under the same lease.
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
    ///
    /// `deleted_by` is who deleted it, read from its tombstone by the caller
    /// ([`deleter`]): its stop ends the apps as theirs.
    async fn finish_deletion(
        &self,
        record: Conversation,
        deleted_by: &McpAppInitiator,
    ) -> Result<(), ConversationError> {
        let id = record.id().clone();
        match AssertUnwindSafe(self.finish_deletion_steps(record, deleted_by))
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
    async fn finish_deletion_steps(
        &self,
        record: Conversation,
        deleted_by: &McpAppInitiator,
    ) -> Result<(), ConversationError> {
        let id = record.id().clone();
        let deletion = record
            .deletion()
            .cloned()
            .ok_or(ConversationError::Metadata)?;
        if deletion.erased() {
            return self
                .end_deleted_ownership(&id, &deletion_actor(&deletion)?)
                .await;
        }
        let actor = deletion_actor(&deletion)?;
        let slot = self.inner.conversations.lock().await.get(&id).cloned();
        if let Some(slot) = slot {
            // Retirement stops this agent itself, and must not wait here for
            // this delete to do it first. Answered as every step after the
            // tombstone is: the delete happened and did not finish
            // (`a_shutdown_ends_a_delete_waiting_to_stop_or_to_lease_and_it_is_left_unfinished`).
            // Its apps' drops are reported as the stop ends them, before it
            // awaits anything, so retirement winning loses none of them
            // (`c15_a_delete_cut_short_by_retirement_records_each_drop_once`).
            let stopped = tokio::select! {
                stopped = self.stop_slot(
                    &id,
                    slot,
                    StopBy {
                        actor: &actor,
                        ended_by: deleted_by,
                        cause: LeaseEndCause::Closed,
                        end: AgentEnd::Close,
                    },
                ) => stopped,
                () = self.retired() => Err(StopFailure::Failed(AgentError::Closed)),
            };
            match stopped {
                Ok(()) => {}
                // The agent was let go and only the record of it was not
                // saved. That record is in the history this erases, so it is
                // not what the delete waits for
                // (`l7_a_delete_whose_cleanup_record_cannot_be_saved_still_erases_the_history`).
                Err(StopFailure::Failed(error)) if confirmed_despite(&error).is_some() => {
                    tracing::warn!(conversation_id = %id, %error, "a deletion's stop confirmed cleanup whose record was not saved");
                }
                Err(stop) => {
                    return Err(ConversationError::DeletionIncomplete(Box::new(
                        DeletionFailures {
                            stop: Some(stop),
                            ..DeletionFailures::default()
                        },
                    )))
                }
            }
        }
        let mut failures = DeletionFailures::default();
        // `None` when it could not be taken. `Some(None)` when nothing is
        // erased under a lease: the history was never opened, or this build
        // cannot read it. That history is discarded only after the deletion
        // record is acknowledged.
        let leased = tokio::select! {
            leased = self.erasure_lease(&id) => leased.map_err(ConversationError::Storage),
            () = self.retired() => Err(ConversationError::Unavailable),
        };
        let mut lost_unreadable = false;
        let lease = match leased {
            Ok(lease) => Some(lease),
            // Still held after the wait for it: the one place a deletion
            // learns its history is leased elsewhere. `Busy` from reading or
            // erasing under its own lease is only storage failing
            // (`busy_under_the_deletion_s_own_lease_is_left_not_carried_on`).
            Err(ConversationError::Storage(StorageError::Busy)) => {
                failures.history_leased_elsewhere = true;
                None
            }
            // This conversation's saved bytes do not decode, or they name
            // another `schemaVersion`. Settle the provider link as unknown so
            // the delete can be recorded. The bytes stay until that record is
            // acknowledged. A records-database failure is [`StorageError::Store`]
            // and falls through: the tombstone stays unread and the delete retries.
            Err(ConversationError::Storage(
                StorageError::Corrupt(_) | StorageError::AnotherVersion { .. },
            )) => {
                lost_unreadable = true;
                Some(None)
            }
            Err(error) => {
                failures.history = Some(error);
                None
            }
        };
        let mut record = record;
        if lost_unreadable && matches!(deletion.provider_session(), ProviderSessionLink::Unread) {
            match self
                .inner
                .metadata
                .record_deletion(record.id(), deletion.after_losing_history())
                .await
            {
                Ok(kept) => record = kept,
                Err(error) => failures.tombstone = Some(error),
            }
        } else if let (ProviderSessionLink::Unread, Some(lease)) =
            (deletion.provider_session(), &lease)
        {
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
            if lost_unreadable {
                match self
                    .inner
                    .storage
                    .discard_unreadable(conversation_session(&id))
                    .await
                {
                    Ok(()) => {}
                    Err(StorageError::Busy) => {
                        failures.history_leased_elsewhere = true;
                    }
                    Err(error) => {
                        failures.history = Some(ConversationError::Storage(error));
                    }
                }
            }
            if let Some(Some(lease)) = &lease {
                if let Err(error) = lease.erase().await {
                    failures.history = Some(ConversationError::Storage(error));
                }
            }
            self.forget_thread_reports_of(&id).await;
            let _writes = self.inner.summary_writes.lock(&id).await;
            if let Err(error) = self.inner.summaries.erase(&id).await {
                failures.summary = Some(error);
            }
            // Where it ran goes with its history; failing to forget it is
            // the summary's kind of failure, retried with the erasure.
            if let Err(error) = self.inner.environment.erase(&id).await {
                failures.summary.get_or_insert(error);
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
                    Ok(_) => return self.end_deleted_ownership(&id, &actor).await,
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
        let storage = self.inner.storage.clone();
        let session = conversation_session(id);
        self.while_history_busy(|| {
            let storage = storage.clone();
            let session = session.clone();
            async move { storage.open_existing(session).await }
        })
        .await
    }

    /// Call `attempt` again while it answers [`StorageError::Busy`], for at
    /// most [`ConversationDeletionBudgets::history_lease`]. One wait, for a
    /// delete taking the lease and for an opening taking it: a stopped
    /// agent's last handles let the history go soon after the stop, not at
    /// it. Still busy at the bound is [`StorageError::Busy`]
    /// (`a_read_right_after_a_persons_close_is_not_busy`,
    /// `an_opening_stops_waiting_for_a_history_lease_at_its_bound`,
    /// `a_deletion_left_for_a_held_lease_is_finished_once_it_is_let_go`).
    async fn while_history_busy<T, F, Fut>(&self, mut attempt: F) -> Result<T, StorageError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, StorageError>>,
    {
        let deadline = Instant::now() + self.inner.deletion_budgets.history_lease;
        loop {
            match attempt().await {
                Err(StorageError::Busy) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                result => return result,
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
                let key = conversation_session(record.id());
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
        // A conversation placed on a host kept its harness's session there,
        // which no handler on this machine can reach: said as no handler,
        // never asked of a local binding.
        if self
            .inner
            .environment
            .placement(record.id())
            .await
            .map_err(AskFailure::Failed)?
            .is_some()
        {
            return Ok(ProviderSessionErasure::NoHandler);
        }
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
        if !self
            .inner
            .attachments
            .as_ref()
            .is_some_and(|attachments| attachments.offers_images())
            || !agent.capabilities().features().input().image()
        {
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
    /// Admission, refused only when the gateway is retiring.
    async fn admit(&self) -> Result<RwLockReadGuard<'_, ()>, Halt> {
        let permit = self.inner.admission.read().await;
        if self.inner.retirement.get().is_some() {
            return Err(Halt::Retired);
        }
        Ok(permit)
    }

    /// Join admitted commands and hold admission until the returned guard
    /// drops. Retirement before the hold, or signaled while waiting for it,
    /// aborts and releases. This is the export cut's lease: reversible,
    /// unlike [`Self::retire`].
    pub(crate) async fn hold_for_export(&self) -> Result<ExportAdmission, ExportHoldFailure> {
        if self.inner.retirement.get().is_some() {
            return Err(ExportHoldFailure::Retired);
        }
        let guard = self.inner.admission.clone().write_owned().await;
        if self.inner.retirement.get().is_some() || *self.inner.retired.borrow() {
            drop(guard);
            return Err(ExportHoldFailure::Retired);
        }
        Ok(ExportAdmission {
            _guard: guard,
            retired: self.inner.retired.subscribe(),
        })
    }

    /// Stop admission and join every initialized or initializing owner before process exit.
    pub async fn shutdown(&self) -> Result<(), ConversationError> {
        self.retire("server_shutdown", &Uuid::new_v4().to_string())
            .await?;
        self.inner
            .storage
            .shutdown()
            .await
            .map_err(ConversationError::Storage)
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
        // Admission stays open, so a submission can be between its last check
        // and its enqueue when this pass comes: each owner's stop waits for the
        // conversation's submission lock first (#528).
        self.close_agents(&actor, Submissions::Admitted).await
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
            Ok(_exclusive) => self.close_agents(actor, Submissions::Drained).await,
            Err(_) => {
                let cleanup_error = self
                    .close_agents(actor, Submissions::Drained)
                    .await
                    .err()
                    .map(Box::new);
                Err(ConversationError::RetirementAdmission { cleanup_error })
            }
        };
        // Each app call runs on a task of its own, and records its last step
        // there: a review withdrawn as its conversation ended, a call sent
        // that its server's stop ended. The runtime that ends after this
        // would cut those records short.
        if !self.app_calls_finished(APP_CALLS_SETTLE).await {
            tracing::warn!("MCP App calls were still running when the gateway stopped");
        }
        // Deletions whose agents were being asked when retirement ended them
        // are still stopping those agents on tasks of their own; the runtime
        // that ends after this would cut them short and leave what they made
        // behind. Each binding bounds its own wait
        // (`retirement_waits_for_abandoned_agent_deletions_to_settle`).
        self.inner.provider_sessions.settled().await;
        retired
    }

    /// Whether any conversation this service opened may still hold provider or
    /// storage resources: an agent whose release the SDK has not confirmed, an
    /// agent whose provider open is still in flight, whoever started it (the
    /// SDK arms its cleanup fact only once an open returns), an opening that
    /// has not settled, a failed opening that holds what it launched, or a
    /// deletion whose process is still being stopped. A successful stop
    /// releases its slot, so it is not counted.
    ///
    /// It reads the SDK's own fact, `Agent::may_hold_provider_resources`,
    /// never the variant of the error a stop returned: that variant is a
    /// diagnostic and does not say whether resources remain (ADR 221).
    #[cfg(any(target_os = "macos", target_os = "linux", test))]
    pub(crate) async fn owns_unreleased_resources(&self) -> bool {
        let slots: Vec<_> = self
            .inner
            .conversations
            .lock()
            .await
            .values()
            .cloned()
            .collect();
        for slot in slots {
            let holds = match slot.value.get() {
                None => true,
                Some(Ok(live)) => {
                    live.agent.may_hold_provider_resources() || live.lease.cleanup().is_none()
                }
                Some(Err(_)) => !slot.acquisition.can_retire(),
            };
            if holds {
                return true;
            }
        }
        // A deletion abandoned during retirement stops its own process on a
        // task of its own, past `settled`'s bound if it must.
        self.inner.provider_sessions.cleanup_outstanding()
    }

    #[cfg(any(target_os = "macos", target_os = "linux", test))]
    pub(crate) fn retirement_cause(&self) -> Option<ActionContext> {
        self.inner.retirement.get().cloned()
    }

    async fn close_agents(
        &self,
        actor: &ActionContext,
        submissions: Submissions,
    ) -> Result<(), ConversationError> {
        let slots: Vec<_> = self
            .inner
            .conversations
            .lock()
            .await
            .iter()
            .map(|(id, slot)| (id.clone(), slot.clone()))
            .collect();
        // Give every owner its own bounded attempt. A stalled opening or provider
        // cannot consume another owner's cleanup opportunity, and neither can
        // a crowd: every stop saves its end through the one record store, so
        // thousands started at once all share it and all run out of budget
        // together. At most `RETIRING_AT_ONCE` start at a time, an owner's
        // budget starts when its stop does, and a stop still running after
        // `RETIRING_PLACE_HELD` gives its place to the next.
        let places = tokio::sync::Semaphore::new(RETIRING_AT_ONCE);
        let places = &places;
        let attempts = slots.into_iter().map(|(id, slot)| {
            // Not held while this owner waits for a place, or through the
            // wait for submissions: an agent that a command ahead of this
            // stop lets go of must not keep its history leased here, or the
            // conversation opened again after it answers `Busy`
            // (`a_desktop_stop_stops_the_owner_live_when_it_takes_the_lock`).
            let slot = match submissions {
                Submissions::Drained => Some(slot),
                Submissions::Admitted => {
                    drop(slot);
                    None
                }
            };
            async move {
                let place = places
                    .acquire()
                    .await
                    .expect("retirement's places are never closed");
                let stop = async {
                    match slot {
                        Some(slot) => {
                            self.stop_slot(
                                &id,
                                slot,
                                StopBy {
                                    actor,
                                    ended_by: &McpAppInitiator::System,
                                    cause: LeaseEndCause::Stopped,
                                    // The gateway is going away, not the work:
                                    // what its agents were doing is resumed at
                                    // the next start (`resume_unfinished`).
                                    end: AgentEnd::Suspend,
                                },
                            )
                            .await
                        }
                        None => self.stop_after_submissions(&id, actor).await,
                    }
                };
                tokio::pin!(stop);
                let stopped = tokio::select! {
                    stopped = &mut stop => stopped,
                    () = tokio::time::sleep(RETIRING_PLACE_HELD) => {
                        drop(place);
                        stop.await
                    }
                };
                // Retirement reports every stop as the agent's error: over its
                // budget is a deadline there.
                stopped.err().map(|stop| {
                    let error = match stop {
                        StopFailure::OverBudget => AgentError::Deadline,
                        StopFailure::Failed(error) => error,
                    };
                    (id.to_string(), error)
                })
            }
        });
        let failures: Vec<_> = join_all(attempts).await.into_iter().flatten().collect();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(ConversationError::Retirement(failures))
        }
    }

    /// Stop one owner, waiting for it to finish opening if it has not, and
    /// release its slot once its cleanup is confirmed ([`let_go`]), which a
    /// stop that answers a failure to save that cleanup's record has also
    /// done. Bounded by
    /// [`ConversationDeletionBudgets::stop`]: running out of time is
    /// [`StopFailure::OverBudget`], its own outcome and never the stop's own
    /// error, which means only that cleanup is unconfirmed; the slot and
    /// supervised SDK work remain owned
    /// (`a_close_that_fails_with_a_deadline_of_its_own_is_not_the_stop_budget`).
    /// Stop the agent of `slot`, its apps ended first by `ended_by` — so
    /// they end, even when the stop does not. What their end drops is
    /// reported there and then, before the agent's close is first awaited:
    /// a caller that gives up on the stop — a select, a budget, a panic —
    /// cannot lose a drop's record, and none takes anything of the stop's
    /// budget (`c15_a_close_whose_stop_runs_past_its_budget_drops_once_as_the_closers`,
    /// where nothing else would end the apps).
    async fn stop_slot(
        &self,
        id: &ConversationId,
        slot: Arc<Slot>,
        by: StopBy<'_>,
    ) -> Result<(), StopFailure> {
        match tokio::time::timeout(
            self.inner.deletion_budgets.stop,
            self.stopping(id, slot, None, by, ConfirmedClose::ReleaseSlot),
        )
        .await
        {
            Ok(result) => result.map_err(StopFailure::Failed),
            Err(_) => Err(StopFailure::OverBudget),
        }
    }

    /// A desktop stop of one owner, ordered with its submissions: take the
    /// conversation's submission lock just long enough to mark the owner it
    /// has then as stopping, then stop that owner. A submission between its
    /// checks and its enqueue holds the lock, so it finishes its enqueue
    /// first and the message settles on the stopped agent; one that takes
    /// the lock after the mark is refused (#528). Nothing waits for the lock
    /// while the agent closes.
    ///
    /// The wait for the lock and the stop run on a task of their own, and
    /// share the owner's one stop budget
    /// (`the_wait_for_the_lock_and_the_stop_share_one_budget`). Past it this
    /// answers [`StopFailure::OverBudget`], and the task carries on: it
    /// marks and stops the owner once the lock is free, and lets its slot go
    /// once the close is confirmed
    /// (`a_desktop_stop_that_cannot_take_the_lock_within_its_budget_carries_on`,
    /// `a_stop_that_runs_past_its_budget_carries_on_and_lets_the_agent_go`).
    async fn stop_after_submissions(
        &self,
        id: &ConversationId,
        actor: &ActionContext,
    ) -> Result<(), StopFailure> {
        let service = self.clone();
        let id = id.clone();
        let actor = actor.clone();
        let stop = tokio::spawn(async move {
            let submissions = service.inner.mode_changes.lock(&id).await;
            // The owner the lock's last holder left: a submission may have
            // released it or opened the conversation again.
            let Some(slot) = service.inner.conversations.lock().await.get(&id).cloned() else {
                return Ok(());
            };
            service
                .stopping(
                    &id,
                    slot,
                    Some(submissions),
                    StopBy {
                        actor: &actor,
                        ended_by: &McpAppInitiator::System,
                        cause: LeaseEndCause::Stopped,
                        end: AgentEnd::Close,
                    },
                    ConfirmedClose::ReleaseSlot,
                )
                .await
        });
        self.join_stop(stop).await
    }

    /// Stop one owner, as [`Self::stop_slot`] does, except a stop past the
    /// budget is not abandoned. This answers [`StopFailure::OverBudget`], and
    /// the stop carries on until the close is confirmed and the slot is
    /// released, as the desktop stop does ([`Self::stop_after_submissions`]).
    /// The slot is gone before the next submission, which opens the
    /// conversation again
    /// (`a_pending_mode_close_past_its_budget_lets_the_agent_go`,
    /// `a_mode_retirement_past_its_budget_lets_the_agent_go`).
    async fn stop_and_release(
        &self,
        id: &ConversationId,
        slot: Arc<Slot>,
        actor: &ActionContext,
        ended_by: &McpAppInitiator,
        cause: LeaseEndCause,
        end: AgentEnd,
    ) -> Result<(), StopFailure> {
        let service = self.clone();
        let id = id.clone();
        let actor = actor.clone();
        let ended_by = ended_by.clone();
        let stop = tokio::spawn(async move {
            service
                .stopping(
                    &id,
                    slot,
                    None,
                    StopBy {
                        actor: &actor,
                        ended_by: &ended_by,
                        cause,
                        end,
                    },
                    ConfirmedClose::ReleaseSlot,
                )
                .await
        });
        self.join_stop(stop).await
    }

    /// Stop one owner for a close that retires a pending mode change, as
    /// [`Self::stop_and_release`] does. The slot stays until this close's
    /// uploads are let go, so a conversation that opens again cannot have
    /// those new uploads retired by this close
    /// (`a_pending_mode_close_past_its_budget_lets_the_uploads_go_before_the_slot`).
    ///
    /// Within the budget the caller lets the uploads go, then the task
    /// releases the slot. Past the budget the caller has already answered, so
    /// the task lets the uploads go in that caller's name and only then
    /// releases the slot. Either way the uploads are let go once
    /// (`a_pending_mode_close_within_its_budget_releases_uploads_once`,
    /// `a_pending_mode_close_past_its_budget_lets_the_uploads_go`).
    /// A release that fails after the caller has gone is logged with its
    /// error: the attempt was made, and the failure is not discarded
    /// (`a_pending_mode_close_past_its_budget_still_asks_to_let_the_uploads_go_when_that_fails`).
    async fn finish_pending_close(
        &self,
        id: &ConversationId,
        slot: Arc<Slot>,
        actor: &ActionContext,
        ended_by: &McpAppInitiator,
        caller: &ConversationCaller,
    ) -> PendingClose {
        let (confirmed_tx, confirmed_rx) = oneshot::channel();
        // `true` once the caller has let the uploads go. `false` means this
        // task does, which is the path past the budget.
        let (handoff_tx, handoff_rx) = oneshot::channel();
        let stopping_service = self.clone();
        let releasing = self.clone();
        let stop_id = id.clone();
        let release_id = id.clone();
        let release_caller = caller.clone();
        let stop_actor = actor.clone();
        let ended_by = ended_by.clone();
        let slot_for_release = Arc::clone(&slot);
        tokio::spawn(async move {
            let stopped = stopping_service
                .stopping(
                    &stop_id,
                    slot,
                    None,
                    StopBy {
                        actor: &stop_actor,
                        ended_by: &ended_by,
                        cause: LeaseEndCause::Closed,
                        end: AgentEnd::Close,
                    },
                    ConfirmedClose::HoldSlot,
                )
                .await;
            let confirmed = let_go(&stopped);
            let _ = confirmed_tx.send(stopped);
            if !confirmed {
                return;
            }
            let caller_released = handoff_rx.await.unwrap_or(false);
            if !caller_released {
                if let Err(error) = releasing
                    .release_closed_uploads(&release_id, &release_caller)
                    .await
                {
                    tracing::error!(
                        conversation_id = %release_id,
                        %error,
                        "a close confirmed after its budget could not let its uploads go"
                    );
                }
            }
            releasing.release_slot(&release_id, &slot_for_release).await;
        });
        match tokio::time::timeout(self.inner.deletion_budgets.stop, confirmed_rx).await {
            Ok(Ok(Ok(()))) => {
                let uploads = self.release_closed_uploads(id, caller).await;
                let _ = handoff_tx.send(true);
                PendingClose::Closed { uploads }
            }
            Ok(Ok(Err(error))) => PendingClose::Stopped(StopFailure::Failed(error)),
            Ok(Err(_ended)) => {
                PendingClose::Stopped(StopFailure::Failed(AgentError::CleanupUncertain))
            }
            Err(_budget) => {
                let _ = handoff_tx.send(false);
                PendingClose::Stopped(StopFailure::OverBudget)
            }
        }
    }

    /// Let go of the uploads `caller` closed `id` to hold. One release, used
    /// by the caller when the close finishes within its budget and by the
    /// carry-on when it confirms later.
    async fn release_closed_uploads(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<(), ConversationError> {
        let Some(attachments) = &self.inner.attachments else {
            return Ok(());
        };
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

    /// Join `stop` for at most the owner's stop budget. Past that, this
    /// answers [`StopFailure::OverBudget`] and detaches the task, which
    /// carries on until the close is confirmed
    /// (`a_stop_that_runs_past_its_budget_carries_on_and_lets_the_agent_go`).
    /// A panic, or the runtime ending the task, is cleanup unconfirmed.
    async fn join_stop(&self, stop: JoinHandle<Result<(), AgentError>>) -> Result<(), StopFailure> {
        match tokio::time::timeout(self.inner.deletion_budgets.stop, stop).await {
            Ok(Ok(stopped)) => stopped.map_err(StopFailure::Failed),
            Ok(Err(_ended)) => Err(StopFailure::Failed(AgentError::CleanupUncertain)),
            Err(_) => Err(StopFailure::OverBudget),
        }
    }

    /// Stop the agent of `slot`, however long that takes: mark it as
    /// stopping, so it takes no more work, wait for it to finish opening if
    /// it has not, end its apps by `ended_by`, and close it. Once the close
    /// is confirmed, [`ConfirmedClose::ReleaseSlot`] releases the slot.
    /// [`ConfirmedClose::HoldSlot`] leaves it, so the caller can let the
    /// uploads go before the slot is released.
    ///
    /// `submissions` is the conversation's submission lock when the caller
    /// took it to order this stop with them. It is held while the owner is
    /// marked, through an opening it waits for, and until its apps are
    /// ended; never while the agent closes, so nothing else on the
    /// conversation waits for the close
    /// (`a_send_during_a_desktop_stop_is_refused_and_the_next_opens_again`).
    async fn stopping(
        &self,
        id: &ConversationId,
        slot: Arc<Slot>,
        submissions: Option<OwnedMutexGuard<()>>,
        by: StopBy<'_>,
        on_confirm: ConfirmedClose,
    ) -> Result<(), AgentError> {
        let StopBy {
            actor,
            ended_by,
            cause,
            end,
        } = by;
        slot.stopping.store(true, Ordering::SeqCst);
        let mut submissions = submissions;
        loop {
            let ready = slot.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            if let Some(value) = slot.value.get() {
                return match value {
                    Ok(live) => {
                        self.end_apps(id, live, ended_by);
                        // Its apps ended before the lock is let go of: an
                        // app's message refused by the mark finds its opening
                        // ended, and is M10
                        // (`m10_a_message_after_a_desktop_stops_mark_is_refused_and_opens_nothing`).
                        drop(submissions.take());
                        // Closed for good, it owes nothing: a turn a restart
                        // cut off, or a failure left to retry, ends with it
                        // rather than waiting unresolved for a resume that
                        // will not come
                        // (`a_close_ends_the_turns_a_restart_cut_off`).
                        let mut owes_nothing = true;
                        if end == AgentEnd::Close {
                            for turn in live.agent.owed_turns().await {
                                if let Err(error) =
                                    live.agent.decline_turn(&turn, actor.clone()).await
                                {
                                    owes_nothing = false;
                                    tracing::warn!(conversation_id = %id, execution_id = %turn.execution_id().as_str(), %error, "a turn the close ends could not be recorded ended; it stays marked, and the next start resumes it");
                                }
                            }
                        }
                        let closed = self
                            .close_leased(live, cause, actor.clone(), end)
                            .await
                            .map(|_| ());
                        // Its cleanup confirmed, the agent holds nothing, and
                        // the slot goes even when the record of that was not
                        // saved: the stop answers that failure, and the next
                        // opening accounts for the lease
                        // (`l5_a_desktop_stop_whose_cleanup_record_cannot_be_saved_says_so_and_the_conversation_opens_again`).
                        if let_go(&closed) {
                            // Closed for good: whatever it was doing ended
                            // with it, so a restart has nothing to resume.
                            if end == AgentEnd::Close && owes_nothing {
                                self.forget_unfinished(id).await;
                            }
                            let _ = live.join_attachment_owner().await;
                            if on_confirm == ConfirmedClose::ReleaseSlot {
                                self.release_slot(id, &slot).await;
                            }
                        }
                        closed
                    }
                    // One rule for a failed opening: while it may still hold
                    // what it launched, its stop cannot be confirmed.
                    Err(failed) if failed.holds => Err(AgentError::CleanupUncertain),
                    Err(_) => {
                        if !slot
                            .acquisition
                            .released(self.inner.deletion_budgets.stop)
                            .await
                        {
                            return Err(AgentError::CleanupUncertain);
                        }
                        if on_confirm == ConfirmedClose::ReleaseSlot {
                            self.release_slot(id, &slot).await;
                        }
                        Ok(())
                    }
                };
            }
            ready.await;
        }
    }

    /// Close `live`'s agent under its lease for `cause`, on a task of its
    /// own: a caller that stops waiting — a budget, a dropped request — does
    /// not stop the lease's end from being recorded, the close from being
    /// confirmed, or an interrupted lease from being accounted for later.
    /// The cleanup deadline is the stop budget, the one bound this gateway
    /// already gives a stop.
    async fn close_leased(
        &self,
        live: &Arc<LiveConversation>,
        cause: LeaseEndCause,
        actor: ActionContext,
        end: AgentEnd,
    ) -> Result<CloseOutcome, AgentError> {
        let live = live.clone();
        let deadline = self.inner.deletion_budgets.stop;
        tokio::spawn(async move {
            live.lease
                .close(&live.agent, cause, actor, deadline, end)
                .await
        })
        .await
        .unwrap_or(Err(AgentError::CleanupUncertain))
    }

    /// Close the agent `slot` holds, which stopped itself and waits for that
    /// close, and let the slot go so the next resolve opens the conversation
    /// again. A close that cannot be confirmed keeps the slot, stopping, and
    /// the message that asked is refused as closed.
    async fn reopen_ended(&self, id: &ConversationId, slot: Arc<Slot>) {
        let Ok(actor) = ActionContext::new(
            "gateway",
            "agent_ended",
            format!("ended-{}", Uuid::new_v4()),
        ) else {
            return;
        };
        tracing::warn!(conversation_id = %id, "the agent stopped itself; closing it to open the conversation again");
        // What it owed stays owed: the agent opened next resumes it.
        if let Err(stop) = self
            .stop_and_release(
                id,
                slot,
                &actor,
                &McpAppInitiator::System,
                LeaseEndCause::Stopped,
                AgentEnd::Suspend,
            )
            .await
        {
            tracing::warn!(conversation_id = %id, ?stop, "an ended agent's close did not confirm cleanup");
        }
    }

    async fn release_slot(&self, id: &ConversationId, slot: &Arc<Slot>) {
        let released = {
            let mut owners = self.inner.conversations.lock().await;
            let owned = owners.get(id).is_some_and(|known| Arc::ptr_eq(known, slot));
            if owned {
                owners.remove(id);
            }
            owned
        };
        if released {
            self.inner.live_changes.publish(id);
            // Ended, however it ended: its apps too. Every stop ends them
            // first, by whoever stopped it; this is for any other way out,
            // and does nothing to apps already ended.
            if let Some(Ok(live)) = slot.value.get() {
                self.end_apps(id, live, &McpAppInitiator::System);
            }
        }
    }
}

/// What a pending-mode close did with the agent and its uploads.
enum PendingClose {
    /// The close confirmed within the budget. `uploads` is this caller's
    /// release, done before the slot is let go.
    Closed {
        uploads: Result<(), ConversationError>,
    },
    /// The close did not confirm within the budget, or it failed. Over budget,
    /// or failed having confirmed the cleanup it could not record
    /// ([`let_go`]), the carry-on lets the uploads go and then releases the
    /// slot; the caller lets nothing go.
    Stopped(StopFailure),
}

/// Who stops an owner, by whom its apps are ended, and the cause its lease's
/// end records.
#[derive(Clone, Copy)]
struct StopBy<'a> {
    actor: &'a ActionContext,
    ended_by: &'a McpAppInitiator,
    cause: LeaseEndCause,
    /// Only the gateway's own retirement and a mode recovery, which opens the
    /// agent again at once, suspend; every other stop is final.
    end: AgentEnd,
}

/// Whether a stop let its agent go: it closed, or it confirmed the cleanup
/// and only saving the record of it failed.
fn let_go(stopped: &Result<(), AgentError>) -> bool {
    match stopped {
        Ok(()) => true,
        Err(error) => confirmed_despite(error).is_some(),
    }
}

/// Whether a confirmed close releases its slot here.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfirmedClose {
    /// The slot goes once the close is confirmed.
    ReleaseSlot,
    /// The slot stays. The caller lets the uploads go, then releases it.
    HoldSlot,
}

/// Whether a pass of stops can meet a submission still being admitted.
#[derive(Clone, Copy)]
enum Submissions {
    /// Admission is open: each owner's stop is ordered with its submissions
    /// under the submission lock ([`ConversationService::stop_after_submissions`]).
    Admitted,
    /// Retirement fenced admission and waited for it: nothing is being
    /// admitted, or what still is after that wait is stopped regardless.
    Drained,
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
    (still_stopping || failures.history_leased_elsewhere).then_some(Waiting::ForRelease)
}

/// Who deleted `record`, by its tombstone: the one source for whom its
/// deletion ends its apps, withdraws their reviews, lets go of their tickets
/// and drops what they held, whoever finishes it. The system when it has
/// none that names a command.
fn deleter(record: &Conversation) -> McpAppInitiator {
    record
        .deletion()
        .and_then(|deletion| deletion_actor(deletion).ok())
        .map_or(McpAppInitiator::System, |actor| initiator_of(&actor))
}

/// The caller recorded on a deletion, as the command that decided it.
fn deletion_actor(deletion: &ConversationDeletion) -> Result<ActionContext, ConversationError> {
    ActionContext::new(
        deletion.initiator().as_str(),
        deletion.surface(),
        deletion.request(),
    )
    .map_err(|_| ConversationError::Metadata)
}

/// Who ended a conversation's apps, by the command `actor` took: the
/// gateway's own actor is the system, anyone else a person.
fn initiator_of(actor: &ActionContext) -> McpAppInitiator {
    match nessa_auth::domain::PrincipalId::new(actor.principal_id()) {
        Ok(principal_id) if actor.principal_id() != "gateway" => McpAppInitiator::Person {
            principal_id,
            surface_id: actor.surface_id().to_owned(),
            request_id: actor.request_id().to_owned(),
        },
        _ => McpAppInitiator::System,
    }
}

/// Who answered an app's review: the caller of `conversation.answer`.
fn answerer(caller: &ConversationCaller) -> ReviewAnswerer {
    ReviewAnswerer {
        principal_id: caller.principal_id.clone(),
        surface_id: caller.surface_id.clone(),
        request_id: caller.action_id.clone(),
    }
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
    /// How many tombstones of unfinished deletions could not be read, so
    /// whose deletion is unfinished cannot be known.
    pub unreadable: usize,
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
/// deletion or taken for a reason to wait
/// (`a_repository_s_error_before_the_fence_is_never_answered_as_a_deletion`,
/// `a_repository_error_in_a_background_try_is_never_a_reason_to_wait`).
///
/// Past the fence, a repository's error is kept whole inside
/// [`DeletionFailures`], and only its own fields decide a wait
/// ([`waiting_for`];
/// `a_repository_error_past_the_fence_is_never_a_reason_to_wait`).
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

fn list_entries(
    owned: &[ListedConversation],
    running: &HashSet<ConversationId>,
) -> Vec<ConversationListEntry> {
    owned
        .iter()
        .map(|listed| ConversationListEntry {
            conversation_id: listed.conversation.id().to_string(),
            title: listed
                .summary
                .title()
                .map(|title| title.as_str().to_owned()),
            preview: listed
                .summary
                .preview()
                .map(|preview| preview.as_str().to_owned()),
            created_at_ms: listed.conversation.creation_requested_at_ms(),
            updated_at_ms: listed.summary.updated_at_ms(),
            running: running.contains(listed.conversation.id()),
            archived: listed.summary.archived(),
        })
        .collect()
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
/// Whether a submission offers the agent the notes waiting for a turn: a
/// person's queued message not already admitted. It is offered whether or
/// not the conversation looks idle now; the agent decides that under the
/// lock that admits the turn, so a turn that ends in between cannot leave an
/// idle admission without the notes it should take.
/// Drops one grant's admission permit after the steer is saved, then
/// compares the live revision. A match keeps that second permit until the
/// provider operation starts. Drop releases it if `steer` never asks.
struct ConfirmSavedGrant {
    hold: std::sync::Mutex<Option<RevisionHold>>,
    grants: Arc<dyn ShareGrants>,
    id: ConversationId,
    receiver: String,
    revision: u64,
}

impl AfterSaveCheck for ConfirmSavedGrant {
    fn after_save<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<(), AfterSaveRefusal>> + Send + 'a>> {
        Box::pin(async move {
            // The permit taken before the save. Released before the
            // compare, so a change that waited on the save can land and
            // this compare can see it. The std mutex is not held across
            // the compare.
            drop(
                self.hold
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take(),
            );
            match self
                .grants
                .hold_revision(&self.id, &self.receiver, self.revision)
                .await
            {
                Ok(again) => {
                    *self
                        .hold
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(again);
                    Ok(())
                }
                Err(ConversationError::Revoked) => Err(AfterSaveRefusal::Refused),
                Err(_) => Err(AfterSaveRefusal::Unavailable),
            }
        })
    }

    fn provider_operation_started(&self) {
        drop(
            self.hold
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take(),
        );
    }
}

fn offers_notes(known: bool, is_app: bool, mode: &SubmissionMode) -> bool {
    !known && !is_app && matches!(mode, SubmissionMode::Queue)
}

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

mod artifact_fetch;
pub use artifact_fetch::{
    FetchAnswer, FetchCall, FetchCallRefusal, FETCH_BUSY_BACKOFF, FETCH_READS,
};
mod artifact_publish;
pub use artifact_publish::{PublishAnswer, PublishCall, PublishCallRefusal};
mod environment_tools;
mod event_read;
mod notes;
mod parking;
use parking::ParkWhen;
mod resumption;
pub use environment_tools::{
    CommandAnswer, CommandCall, CommandCallError, EnvironmentListing, HERE,
};
pub use event_read::{
    event_error_code, project_events, validate_event_cursor, ConversationEventError, EventProgress,
    ThreadEvent,
};
pub use resumption::FailureBackoff;

mod app_calls;
pub use app_calls::{
    McpAppCall, McpAppContextUpdate, McpAppMessage, McpAppRead, McpAppResource, MAX_APP_CALLS,
    MAX_RESOURCE_META_BYTES,
};

#[cfg(test)]
#[path = "../../../tests/conversation/close_release.rs"]
mod close_release_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/offers_notes.rs"]
mod offers_notes_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/retirement.rs"]
mod retirement_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/resumption.rs"]
mod resumption_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/desktop_stop.rs"]
mod desktop_stop_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/parking.rs"]
mod parking_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/stop_release.rs"]
mod stop_release_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/opening_diagnostics.rs"]
mod opening_diagnostics_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/deletion.rs"]
mod deletion_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/creation_metadata.rs"]
mod creation_metadata_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/approval_mode.rs"]
mod approval_mode_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/model_changes.rs"]
mod model_change_tests;
