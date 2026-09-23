//! Typed session snapshot persistence and exclusive access contracts.
#![deny(missing_docs)]

use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::{
    ExecutionEvent, ExecutionRequest, SubmissionMode,
};
use crate::application::agent_execution::permissions::ActionContext;
use crate::application::agent_execution::providers::{ExecutionReport, ProviderIdentity};
pub use crate::domain::agent_execution::sessions::ProviderContext;
use crate::domain::agent_execution::{
    executions::{
        ExecutionId, ExecutionOutcome, InvocationCancellation, InvocationKind, InvocationStage,
        QueueMutation, SchedulingCause, SchedulingInitiator, SchedulingTransition,
        SchedulingTransitionError,
    },
    sessions::SessionId,
};
use std::{error::Error, fmt, future::Future, pin::Pin};

/// Asynchronous storage result borrowing its adapter for `'a` and returning `T`.
pub type StorageFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, StorageError>> + Send + 'a>>;

/// Failure to acquire access, read, validate, or persist a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageError {
    /// Another owner already holds this session’s writer lease.
    Busy,
    /// Backend operation failed; the string carries diagnostic context.
    Io(String),
    /// Stored evidence is invalid; the string describes the validation failure.
    Corrupt(String),
    /// The session or provider identity differs from the requested context.
    IdentityMismatch,
}
impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "session storage: {self:?}")
    }
}
impl Error for StorageError {}

/// Retained application evidence, not the authoritative live execution aggregate.
#[derive(Clone, Debug)]
pub struct SessionSnapshot {
    /// Local conversation key under which this snapshot is stored.
    pub id: SessionId,
    /// Provider configuration identity required for restoration.
    pub provider: ProviderIdentity,
    /// Opaque provider context to resume without replaying input, or explicit absence.
    pub provider_context: ProviderContext,
    /// Submitted invocations in admission order, including unresolved work.
    pub invocations: Vec<InvocationRecord>,
    /// Append-only global queue membership/order facts. Local selection precedes
    /// provider dispatch; restoration clears pending membership without replay.
    pub queue_history: Vec<QueueHistoryRecord>,
}
impl SessionSnapshot {
    /// Maximum durable invocation records retained by one conversation.
    pub const MAX_INVOCATIONS: usize = 1024;
}

/// One queue membership decision, recorded in global scheduler order.
/// These records distinguish local dequeue from later provider dispatch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueHistoryRecord {
    /// Exact membership or order change at the exclusive scheduler boundary.
    pub mutation: QueueMutation,
    /// Verified explicit initiator; selection, automatic stop and restoration use None.
    pub actor: Option<ActionContext>,
    /// Scheduling prefix at this fact for single-input changes. A selection can
    /// precede Dispatched; withdrawal/stop records include their cancellation edge.
    pub scheduling_length: Option<usize>,
}
impl QueueHistoryRecord {
    /// Maximum actual reorder decisions retained; membership events have a
    /// separate bound proportional to admitted invocation history.
    pub const MAX_REORDERS: usize = 1024;
}

/// Saved input and observations. Never replay an input solely because its result
/// is missing: pending, cancelled, or injected input may have no execution result.
#[derive(Clone, Debug)]
pub struct InvocationRecord {
    /// Target observation count at steering admission; not a provider consumption acknowledgement.
    pub target_event_offset: Option<usize>,
    /// Original delivery operation; retries must preserve this intent.
    pub submission: SubmissionMode,
    /// Exact submitted input and execution identity.
    pub request: ExecutionRequest,
    /// Caller attribution verified by the host before submission.
    pub actor: ActionContext,
    /// Provider observations associated with this invocation, in received order.
    pub events: Vec<ExecutionEvent>,
    /// Local scheduling transitions in causal order, independent of provider output.
    pub scheduling: Vec<InvocationSchedulingEvent>,
    /// Cancellation before this immediate input reached the provider. The request
    /// identifies its target; this local decision does not confirm provider cleanup.
    pub cancellation: Option<InvocationCancellationEvent>,
    /// Provider settlement facts, separate from later local hook/storage failures.
    /// Absence means no settlement report was observed, not that dispatch failed.
    pub provider_report: Option<ExecutionReport>,
    /// First stop captured by this invocation owner before a local-cancellation report.
    /// Required exactly when the report source is `LocalCancellation`; it retains
    /// the cause and caller independently of physical cleanup confirmation. A missing
    /// provider reply alone does not establish local cancellation.
    pub local_cancellation: Option<InvocationCancellationEvent>,
    /// Earlier successful local outcome, retained if a later local failure replaces it.
    /// When the current result is successful, that result also supplies this fact.
    pub local_outcome: Option<ExecutionOutcome>,
    /// Saved settlement, or `None` if settlement has not been recorded.
    pub result: Option<Result<ExecutionOutcome, AgentError>>,
}

/// Cause and caller of a stop captured by an admitted invocation owner.
/// The enclosing request supplies the target. `InvocationRecord::cancellation`
/// records undispatched immediate cancellation; `local_cancellation` records
/// the stop underlying a dispatched local settlement without a provider reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvocationCancellationEvent {
    /// Why the owner stopped this invocation; validated by the domain cancellation value.
    pub cause: SchedulingCause,
    /// Verified caller for explicit closure; absent for an automatic stop.
    pub actor: Option<ActionContext>,
}
impl InvocationCancellationEvent {
    /// Validate this cause and caller presence without performing effects.
    /// Returns corruption when the pair cannot describe a supported local stop.
    pub fn cancellation(&self) -> Result<InvocationCancellation, StorageError> {
        InvocationCancellation::new(
            self.cause,
            if self.actor.is_some() {
                SchedulingInitiator::Caller
            } else {
                SchedulingInitiator::Automatic
            },
        )
        .map_err(|error| StorageError::Corrupt(error.to_string()))
    }
}

/// Retained scheduling evidence mapped by the application from local decisions.
///
/// The enclosing [`InvocationRecord::request`] identifies the affected input;
/// persist and inspect these edges with that record, never as standalone audit
/// entries. Its `result` retains any failure that stopped the queue runner, while
/// provider-session closure evidence separately describes provider cleanup.
/// These records describe
/// local lifecycle decisions; only an injected stage confirms provider acceptance
/// of steering input, and neither cancellation nor failure implies rollback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvocationSchedulingEvent {
    /// Priority with which the input was admitted.
    pub kind: InvocationKind,
    /// Active execution selected for attempted native injection. Retained after
    /// fallback or failure; only `Injected` confirms provider acceptance.
    pub target: Option<ExecutionId>,
    /// Previous local stage, or `None` for the first scheduling event.
    pub before: Option<InvocationStage>,
    /// Local stage after this transition.
    pub stage: InvocationStage,
    /// Lifecycle cause that produced this transition.
    pub cause: SchedulingCause,
    /// Verified initiator of an explicit action; automatic causes use `None`.
    pub actor: Option<ActionContext>,
}

impl InvocationSchedulingEvent {
    /// Map an ordered invocation history and validate its continuity in the domain.
    /// `events` borrows the retained boundary evidence and is never changed. Empty
    /// history is accepted; malformed edges or changed kind/target/prior stages
    /// return [`SchedulingTransitionError`]. Caller attribution stays in the DTOs.
    pub fn validate_history(events: &[Self]) -> Result<(), SchedulingTransitionError> {
        let transitions = events
            .iter()
            .map(Self::transition)
            .collect::<Result<Vec<_>, _>>()?;
        SchedulingTransition::validate_history(&transitions)
    }

    /// Validate this boundary projection through domain transition rules.
    /// Caller identities remain in this DTO; the domain checks whether the cause
    /// requires explicit attribution. Returns a typed error for impossible evidence.
    pub fn transition(&self) -> Result<SchedulingTransition, SchedulingTransitionError> {
        SchedulingTransition::new(
            self.kind,
            self.target.clone(),
            self.before,
            self.stage,
            self.cause,
            if self.actor.is_some() {
                SchedulingInitiator::Caller
            } else {
                SchedulingInitiator::Automatic
            },
        )
    }
}

/// Shared backend that acquires session-specific storage access.
///
/// Implementations may serve many sessions. Each returned lease owns the resources
/// needed to read and write one session independently of this backend's lifetime.
pub trait SessionStorage: Send + Sync {
    /// Acquires a writer lease for `id`, the local conversation key.
    ///
    /// Returns [`StorageError::Busy`] when already leased, or a backend error.
    /// Acquisition must exclude competing owners before returning. If an opening
    /// future is dropped, any acquired resources must eventually be released.
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>>;
}

/// Exclusive storage access to one local session, owned by its session manager.
///
/// A lease prevents two live agents from overwriting the same conversation.
/// It binds all reads and writes to the identity supplied to [`SessionStorage::open`].
/// The manager retains this handle instead of retaining both a backend and an
/// optional initialized store. It is a storage port, not a domain entity.
///
/// Access lasts until drop. Outstanding I/O must retain the lock until it finishes,
/// even if its caller stops waiting. Dropping a lease does not delete the session
/// or close its provider context. Implementations must document their exclusion
/// scope and durability; the supplied file adapter excludes other local processes,
/// while memory storage excludes owners sharing the same storage instance.
pub trait SessionStorageLease: Send + Sync {
    /// Loads the saved snapshot, returning `None` for a session with no snapshot.
    ///
    /// Returns a backend or validation error for unreadable or invalid evidence;
    /// invalid data must never be treated as an empty conversation.
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>>;

    /// Atomically saves this session's complete logical state from `snapshot`.
    /// Adapters may persist only changes; the file adapter appends JSONL records.
    ///
    /// Returns [`StorageError::IdentityMismatch`] for a different session and a
    /// validation or backend error if the write cannot be acknowledged. An error
    /// does not imply that provider work was rolled back, or that no write reached
    /// storage. Successful return must satisfy the adapter's durability contract.
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()>;
}

impl SessionSnapshot {
    /// Release rejected adapter evidence without recursive error-tree destruction.
    pub(crate) fn discard_rejected_errors(mut self) {
        for invocation in &mut self.invocations {
            if let Some(Err(error)) = invocation.result.take() {
                error.discard_iteratively();
            }
        }
    }
}

impl StorageError {
    /// Compact external diagnostics before an SDK error wrapper or clone retains them.
    pub(crate) fn bounded(self) -> Self {
        fn text(mut value: String) -> String {
            const LIMIT: usize = 4096;
            const SUFFIX: &str = " [diagnostic truncated]";
            if value.len() > LIMIT {
                let mut end = LIMIT - SUFFIX.len();
                while !value.is_char_boundary(end) {
                    end -= 1;
                }
                value.truncate(end);
                value.push_str(SUFFIX);
            }
            value.into_boxed_str().into_string()
        }
        match self {
            Self::Io(value) => Self::Io(text(value)),
            Self::Corrupt(value) => Self::Corrupt(text(value)),
            other => other,
        }
    }
}
