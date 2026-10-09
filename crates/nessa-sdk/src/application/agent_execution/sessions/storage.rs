//! Typed session snapshot persistence and exclusive access contracts.
#![deny(missing_docs)]

use super::{CommittedStatus, CommittedViewState};
use crate::application::agent_execution::agents::{AgentError, DiagnosticTreeLimits};
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
use nessa_sync::replication::domain::Id;
use std::{error::Error, fmt, future::Future, mem, pin::Pin, sync::Arc};

/// Asynchronous storage result borrowing its adapter for `'a` and returning `T`.
pub type StorageFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, StorageError>> + Send + 'a>>;

mod save;
pub use save::{
    SessionLoad, SessionLoadState, SessionSaveBackend, SessionSaveGeneration, SessionSaveReceipt,
    SessionSaveUnit,
};

/// Failure to acquire access, read, validate, or persist a session.
///
/// The variants are exhaustive. Matching every one is part of the contract, so
/// [`Self::AnotherVersion`] is a variant a caller names. [`Self::SCHEMA_VERSION`]
/// stays on this type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageError {
    /// Storage shutdown has closed admission.
    Closed,
    /// Bounded read or continuation admission is currently full.
    ReadCapacity,
    /// An owned committed read or source thread panicked before joining.
    ReadWorkerPanicked,
    /// Retained diagnostic text or error-tree structure exceeds its resource budget.
    DiagnosticLimit,
    /// Both read draining and runtime cleanup failed.
    ShutdownFailures(Box<StorageShutdownFailure>),
    /// Another owner already holds this session’s writer lease.
    Busy,
    /// Backend operation failed; the string carries diagnostic context.
    Io(String),
    /// Stored evidence is invalid; the string describes the validation failure.
    Corrupt(String),
    /// The session or provider identity differs from the requested context.
    IdentityMismatch,
    /// This adapter requires the semantic SDK decisions with the candidate snapshot.
    ChangesRequired,
    /// A prior write may still commit and must be reconciled before reading old state.
    Unresolved,
    /// An atomic encoded decision group exceeds the record writer's 160 MiB body limit.
    TooLarge,
    /// This backend cannot supply an independent committed read.
    CommittedReadUnavailable,
    /// The saved record is a format version this build does not read.
    ///
    /// `found` is the record's `schemaVersion` when that integer is not
    /// [`Self::SCHEMA_VERSION`]. `None` means the field is absent. The reader
    /// stops at this record and opens the chat from the prefix folded before
    /// it. Alpha does not interpret an unmarked record. The stored bytes are
    /// left unchanged. A record at this build's version whose body cannot be
    /// parsed is [`Self::Corrupt`], and that record ends the prefix the same
    /// way. A marker that is not an unsigned integer is [`Self::Corrupt`].
    AnotherVersion {
        /// The `schemaVersion` integer in the record, if it has one.
        found: Option<u64>,
    },
}

/// Independent typed failures from read draining and record runtime shutdown.
/// The immutable boxed payload owns failure evidence, never cleanup resources.
/// Construction validates the shared diagnostic tree before retaining it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageShutdownFailure {
    read: StorageError,
    runtime: StorageError,
}
impl StorageShutdownFailure {
    /// Preserve both typed causes and compact diagnostic spare allocations.
    /// This performs no cleanup and does not establish a shutdown outcome.
    ///
    /// # Errors
    /// Returns [`StorageError::DiagnosticLimit`] for over-budget text, nodes,
    /// depth or allocation before any clone or retention. The existing diagnostic
    /// tree limits are shared with AgentError and the snapshot codec.
    pub fn new(read: StorageError, runtime: StorageError) -> Result<Self, StorageError> {
        let mut candidate = Self { read, runtime };
        let usage = candidate.usage()?;
        if usage.text > StorageError::DIAGNOSTIC_BYTES {
            return Err(StorageError::DiagnosticLimit);
        }
        candidate.read.compact_diagnostics();
        candidate.runtime.compact_diagnostics();
        let usage = candidate.usage()?;
        if usage.allocations > DiagnosticTreeLimits::BYTES
            || usage.diagnostics > StorageError::DIAGNOSTIC_BYTES
        {
            return Err(StorageError::DiagnosticLimit);
        }
        Ok(candidate)
    }
    /// Failure from joining admitted read owners; this is an observation, not a reservation.
    pub fn read(&self) -> &StorageError {
        &self.read
    }
    /// Failure from the record runtime's cleanup attempt.
    pub fn runtime(&self) -> &StorageError {
        &self.runtime
    }
    pub(crate) fn into_parts(mut self) -> (StorageError, StorageError) {
        (
            mem::replace(&mut self.read, StorageError::Closed),
            mem::replace(&mut self.runtime, StorageError::Closed),
        )
    }
    fn usage(&self) -> Result<StorageErrorUsage, StorageError> {
        StorageError::measure(
            vec![(&self.read, 2), (&self.runtime, 2)],
            StorageErrorUsage {
                nodes: 1,
                depth: 1,
                allocations: mem::size_of::<Self>(),
                ..StorageErrorUsage::default()
            },
        )
    }
}
impl Drop for StorageShutdownFailure {
    fn drop(&mut self) {
        // Public construction bounds the tree; this also safely drains rejected
        // private/decoded trees without recursive Box destruction.
        let mut pending = vec![
            mem::replace(&mut self.read, StorageError::Closed),
            mem::replace(&mut self.runtime, StorageError::Closed),
        ];
        while let Some(error) = pending.pop() {
            if let StorageError::ShutdownFailures(failure) = error {
                let (read, runtime) = failure.into_parts();
                pending.push(read);
                pending.push(runtime);
            }
        }
    }
}
#[derive(Default)]
pub(crate) struct StorageErrorUsage {
    pub(crate) nodes: usize,
    pub(crate) depth: usize,
    pub(crate) allocations: usize,
    diagnostics: usize,
    text: usize,
}

/// A read of committed storage, independent of an agent's live observations.
/// `position` is monotone only within `incarnation`; it may count physical
/// records or snapshot revisions according to the storage adapter.
#[derive(Clone, Debug)]
pub struct CommittedSession {
    id: SessionId,
    incarnation: Id,
    position: u64,
    downloaded: u64,
    observed_head: u64,
    snapshot: Option<Arc<SessionSnapshot>>,
    status: CommittedStatus,
    /// Save groups this build could not fold, in stream order. Empty when
    /// every record folded. Not part of the snapshot a later save appends to.
    unreadable: Vec<super::UnreadablePart>,
}
impl CommittedSession {
    /// Validate a replacement read from a storage adapter. Full snapshot validation
    /// uses the same application owner as session restoration.
    ///
    /// # Errors
    /// Refuses invalid incarnation, positions, completeness, or semantic history.
    pub fn new(
        id: SessionId,
        incarnation: String,
        position: u64,
        downloaded: u64,
        observed_head: u64,
        snapshot: Option<SessionSnapshot>,
        status: CommittedStatus,
    ) -> Result<Self, StorageError> {
        let incarnation = Id::new(&incarnation)
            .map_err(|_| StorageError::Corrupt("committed read incarnation is invalid".into()))?;
        if !status.validates(position, downloaded, observed_head, snapshot.is_some()) {
            return Err(StorageError::Corrupt(
                "committed read metadata disagrees with state".into(),
            ));
        }
        if let Some(snapshot) = &snapshot {
            if snapshot.id != id {
                return Err(StorageError::IdentityMismatch);
            }
            super::validation::validate(snapshot)?;
        }
        Ok(Self {
            id,
            incarnation,
            position,
            downloaded,
            observed_head,
            snapshot: snapshot.map(Arc::new),
            status,
            unreadable: Vec::new(),
        })
    }
    pub(crate) fn from_transcript(
        id: SessionId,
        incarnation: Id,
        downloaded: u64,
        observed_head: u64,
        applied: u64,
        snapshot: Option<Arc<SessionSnapshot>>,
        status: CommittedStatus,
    ) -> Result<Self, StorageError> {
        if snapshot.as_ref().is_some_and(|snapshot| snapshot.id != id) {
            return Err(StorageError::IdentityMismatch);
        }
        if !status.validates(applied, downloaded, observed_head, snapshot.is_some()) {
            return Err(StorageError::Corrupt(
                "committed read status disagrees with positions".into(),
            ));
        }
        Ok(Self {
            id,
            incarnation,
            position: applied,
            downloaded,
            observed_head,
            snapshot,
            status,
            unreadable: Vec::new(),
        })
    }
    /// Exact session identity requested from storage.
    pub fn id(&self) -> &SessionId {
        &self.id
    }
    /// Storage incarnation that scopes positions.
    pub fn incarnation(&self) -> &str {
        self.incarnation.as_str()
    }
    /// Last complete committed position.
    pub fn position(&self) -> u64 {
        self.position
    }
    /// Last downloaded physical frame, including unfinished attempts.
    pub fn downloaded(&self) -> u64 {
        self.downloaded
    }
    /// Greatest physical or adapter revision head observed by this read.
    /// Latest observed physical source position; this may include an incomplete
    /// fact. The semantic terminal position is [`Self::position`].
    pub fn observed_head(&self) -> u64 {
        self.observed_head
    }
    /// Full validated semantic history.
    pub fn snapshot(&self) -> Option<&SessionSnapshot> {
        self.snapshot.as_deref()
    }
    /// Orthogonal read completeness and freshness.
    pub fn status(&self) -> CommittedStatus {
        self.status
    }
    /// Save groups this read could not fold, in stream order.
    ///
    /// Each part names the session's first unread record, how many invocations
    /// were already folded, and why. A later save does not rewrite those bytes.
    pub fn unreadable(&self) -> &[super::UnreadablePart] {
        &self.unreadable
    }
    /// Attach parts this read could not fold. The snapshot stays the history
    /// that did fold, and a later save still appends to that snapshot.
    pub fn with_unreadable(mut self, parts: Vec<super::UnreadablePart>) -> Self {
        self.unreadable = parts;
        self
    }
    /// Coarse display completeness and freshness.
    pub fn state(&self) -> CommittedViewState {
        self.status.view_state()
    }
}
impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "session storage: {self:?}")
    }
}
impl Error for StorageError {}

/// Retained application evidence, not the authoritative live execution aggregate.
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// One SDK decision retained for an atomic semantic-record save.
///
/// A record writer receives these in decision order. Existing snapshot adapters
/// may use the accompanying snapshot alone. No variant authorizes replay to run
/// a provider or tool effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionChange {
    /// The first empty state of a newly created conversation.
    Opened {
        /// Conversation identity.
        id: SessionId,
        /// Provider identity selected for this conversation.
        provider: ProviderIdentity,
        /// Initial recoverable provider context, usually absent.
        context: ProviderContext,
    },
    /// Exact input and pending receipt accepted before provider work.
    InputAccepted(Box<InvocationRecord>),
    /// Actual queue membership, selection, restoration or order decision.
    QueueDecision(QueueHistoryRecord),
    /// Local lifecycle edge for the named invocation.
    SchedulingTransition {
        /// Invocation identity.
        execution_id: ExecutionId,
        /// The validated new edge.
        event: InvocationSchedulingEvent,
    },
    /// Provider observation owned by a dispatched invocation.
    ProviderObservation(ExecutionEvent),
    /// Audit and storage acknowledgement of an earlier accepted input.
    ReceiptUpdated {
        /// Invocation identity.
        execution_id: ExecutionId,
        /// Previously retained acknowledgement.
        before: SubmissionAcknowledgement,
        /// New acknowledgement.
        after: SubmissionAcknowledgement,
    },
    /// Undispatched local stop with its cause and caller.
    StopDecision {
        /// Invocation identity.
        execution_id: ExecutionId,
        /// Stop evidence.
        event: InvocationCancellationEvent,
    },
    /// Provider settlement and the first dispatched local stop, if any.
    ProviderReport {
        /// Invocation identity.
        execution_id: ExecutionId,
        /// Provider-owned result evidence.
        report: ExecutionReport,
        /// Stop evidence required for a local-cancellation report.
        local_stop: Option<InvocationCancellationEvent>,
    },
    /// A local result superseding the prior saved result, if any.
    LocalSettlement {
        /// Invocation identity.
        execution_id: ExecutionId,
        /// Previously saved result.
        before: Option<Result<ExecutionOutcome, AgentError>>,
        /// Newly saved result.
        after: Result<ExecutionOutcome, AgentError>,
        /// Earlier successful local outcome retained across a later failure.
        local_outcome: Option<ExecutionOutcome>,
    },
    /// The provider context needed to resume an existing provider session.
    ProviderContext {
        /// Previously saved context.
        before: ProviderContext,
        /// Newly saved context.
        after: ProviderContext,
    },
}
impl SessionSnapshot {
    pub(crate) fn retained_bytes(&self) -> usize {
        super::retained::snapshot(self)
    }

    /// Pending input identities in committed dispatch order, reconstructed by
    /// the same queue authority used during restoration and semantic folding.
    /// This is a read of saved evidence and grants no dispatch authority.
    ///
    /// # Errors
    /// Returns corruption when queue facts contradict the saved invocations.
    pub fn pending_order(&self) -> Result<Vec<ExecutionId>, StorageError> {
        Ok(super::queue_validation::replay(self)?
            .pending()
            .into_iter()
            .map(|(id, _)| id)
            .collect())
    }

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

    /// Structural upper bound for retained queue evidence at an invocation count.
    /// Membership may retain admission, selection and removal evidence; reorder
    /// evidence has its separately owned limit. Uses saturating arithmetic.
    pub const fn maximum_entries(invocations: usize) -> usize {
        invocations
            .saturating_mul(3)
            .saturating_add(Self::MAX_REORDERS)
    }
}

/// Saved input and observations. Never replay an input solely because its result
/// is missing: pending, cancelled, or injected input may have no execution result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvocationRecord {
    /// Target observation count at steering admission; not a provider consumption acknowledgement.
    pub target_event_offset: Option<usize>,
    /// Original delivery operation; retries must preserve this intent.
    pub submission: SubmissionMode,
    /// Exact submitted input and execution identity.
    pub request: ExecutionRequest,
    /// Caller attribution verified by the host before submission.
    pub actor: ActionContext,
    /// Independent audit and durable-write acknowledgement for admission or
    /// confirmed native injection. This fact is retained even when delivery or
    /// execution has no terminal result.
    pub acknowledgement: SubmissionAcknowledgement,
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

/// Durable acknowledgement state for a submitted invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmissionAcknowledgement {
    /// The submission is saved, but its admission or injection acknowledgement
    /// has not completed yet.
    Pending,
    /// Audit and storage both acknowledged the applicable boundary effect.
    Acknowledged,
    /// The boundary effect remains retained with one or both acknowledgement failures.
    Failed {
        /// Mandatory audit failure, if the audit sink did not acknowledge it.
        audit: Option<AgentError>,
        /// Durable snapshot failure, if storage did not acknowledge it.
        storage: Option<StorageError>,
    },
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
    /// Read a committed session without acquiring its exclusive writer lease.
    /// A gateway uses this for replacement views. Missing session returns
    /// `None`; a valid empty stream returns a session with no snapshot.
    ///
    /// # Errors
    /// Returns `CommittedReadUnavailable` for adapters without a reader, or
    /// the backend's typed storage failure. This read never runs a provider.
    fn read_committed(&self, _id: SessionId) -> StorageFuture<'_, Option<CommittedSession>> {
        Box::pin(async { Err(StorageError::CommittedReadUnavailable) })
    }
    /// Drains this backend after all session owners have stopped. Snapshot
    /// backends have no shared runtime to close.
    fn shutdown(&self) -> StorageFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    /// Acquires a writer lease for `id`, the local conversation key.
    ///
    /// Returns [`StorageError::Busy`] when already leased, or a backend error.
    /// Acquisition must exclude competing owners before returning. If an opening
    /// future is dropped, any acquired resources must eventually be released.
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>>;

    /// Acquires a writer lease for `id` as [`Self::open`] does, but only for a
    /// session this backend already holds something for; `Ok(None)` when it
    /// holds nothing, in which case nothing is created.
    ///
    /// For a caller that means to read or erase what a session saved and has
    /// no reason to begin one: opening a session that never existed would
    /// leave behind a stream or exclusion resource for an identity nothing
    /// will use again.
    ///
    /// The default cannot tell whether a session exists without acquiring it,
    /// so it acquires it exactly as [`Self::open`] does and returns `Some`,
    /// creating whatever that creates. [`RecordStorage`] and
    /// [`InMemoryStorage`] override it and create no session stream
    /// (`opening_an_existing_session_creates_nothing_for_one_that_never_was`).
    ///
    /// # Errors
    /// The same as [`Self::open`]: [`StorageError::Busy`] while another owner
    /// holds the session, or a backend error.
    ///
    /// [`RecordStorage`]: crate::infrastructure::session_storage::RecordStorage
    /// [`InMemoryStorage`]: crate::infrastructure::session_storage::InMemoryStorage
    fn open_existing(
        &self,
        id: SessionId,
    ) -> StorageFuture<'_, Option<Box<dyn SessionStorageLease>>> {
        Box::pin(async move { self.open(id).await.map(Some) })
    }

    /// Remove history this build cannot open, without folding it.
    ///
    /// Legacy JSONL and a stream whose replay refuses are not a prefix a
    /// reader can open. Deleting that chat still finishes: this drops the
    /// JSONL file and resets the stream without reading its bytes. The
    /// default reports nothing to remove. [`RecordStorage`] removes both.
    /// Adapters that can refuse a record must override this, or a delete
    /// treats the history as already gone.
    ///
    /// # Errors
    /// [`StorageError::Busy`] when another owner holds the session. A backend
    /// error when the file or the reset cannot be acknowledged.
    ///
    /// [`RecordStorage`]: crate::infrastructure::session_storage::RecordStorage
    fn discard_unreadable(&self, id: SessionId) -> StorageFuture<'_, ()> {
        let _ = id;
        Box::pin(async { Ok(()) })
    }
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
/// scope and durability; the supplied record adapter's SQLite runtime excludes
/// other local processes, while memory storage excludes owners sharing one instance.
pub trait SessionStorageLease: Send + Sync {
    /// Read published state and the adapter's actual save binding, or typed
    /// unfinished evidence. Unfinished state must not initialize a provider.
    /// Cancellation retains the adapter's original physical I/O ownership.
    fn load(&self) -> StorageFuture<'_, SessionLoad>;

    /// Persist explicit indivisible semantic units and their final candidate.
    /// Every unit checkpoint and the complete candidate validate before pending
    /// reconciliation or append. Record adapters keep all units unpublished until
    /// one durable completion terminal; snapshot adapters persist the candidate.
    ///
    /// Retain the original binding and exact ordered boundaries after failure,
    /// cancellation or a lost reply. An exact retry can resume a confirmed prefix;
    /// a valid extension retains that prefix. Success acknowledges this exact
    /// plan and supplies the next backend binding. Empty plans are refused.
    /// An individually oversized unit returns `TooLarge` before any append.
    ///
    /// A retry is compared with the units already durable: one that changes or
    /// omits any of them is refused. Units that never became durable leave no
    /// evidence, so only the plan's original owner may retry it.
    ///
    /// # Errors
    /// Refuses wrong incarnation/base/generation, a prefix that changes or omits
    /// durable units, invalid unit/candidate and unavailable persistence. A failure is not proof
    /// of absence and cannot release a reserved command identity for redispatch.
    fn save_changes(
        &self,
        binding: SessionSaveGeneration,
        snapshot: SessionSnapshot,
        units: Vec<SessionSaveUnit>,
    ) -> StorageFuture<'_, SessionSaveReceipt>;

    /// Erases this session's saved history, so that it has no snapshot.
    ///
    /// Erasure happens under the exclusive lease this handle holds, which is
    /// why it is an operation of the lease and not of the backend: removing
    /// history that another owner is still writing would let that owner write
    /// it back, and removing it from outside any lease would let an opener
    /// acquire the session halfway through. The exclusion resource itself is
    /// kept, so a second opener is still refused with [`StorageError::Busy`]
    /// until this lease is dropped; removing it while held would let that
    /// opener acquire a fresh one beside this lease, which is two writers.
    ///
    /// Afterwards [`Self::load`] returns published empty evidence with a new binding. The identity is not retired:
    /// a later save through this lease or a later one starts a new history
    /// under it, using the adapter's supported save method. A caller erasing a session permanently must therefore
    /// stop using that identity itself. Erasing a session that has no saved
    /// history succeeds. Nothing outside this storage is touched: a provider's
    /// own record of the context named by [`SessionSnapshot::provider_context`]
    /// is the provider's to keep or erase.
    ///
    /// Like [`Self::save_changes`], an erase that has started retains the lease until
    /// it finishes, even if its caller stops waiting.
    ///
    /// # Errors
    /// Returns a backend error when removal cannot be acknowledged. A reset
    /// may already have committed while retired physical rows still await
    /// cleanup. The same live lease fences later loads and saves until a retry
    /// reconciles the reset and completes physical cleanup. After process
    /// restart, the deletion caller's durable tombstone remains the fence: it
    /// must retry erase before treating an empty replacement snapshot as an
    /// erasure acknowledgement.
    fn erase(&self) -> StorageFuture<'_, ()>;
}

impl SessionSnapshot {
    /// Loads the snapshot saved for the session `id` through `lease`, checked
    /// the way [`Agent`](crate::application::agent_execution::agents::Agent)
    /// checks one before restoring it: every relationship inside it is
    /// validated, and it must have been saved under `id`.
    ///
    /// For a caller that reads saved history without restoring it — to learn
    /// which provider context a session names before erasing it, say — and so
    /// must not trust a custom adapter to have returned the session it was
    /// asked for. It does not compare the snapshot's provider identity, which
    /// only a caller holding the configured provider can; restoration still
    /// does. Nothing is written, and `lease` is only borrowed.
    ///
    /// # Errors
    /// A backend error from [`SessionStorageLease::load`];
    /// [`StorageError::Corrupt`] for a snapshot whose relationships do not
    /// hold; [`StorageError::IdentityMismatch`] for a snapshot saved under
    /// another session (`a_snapshot_for_another_session_is_refused_when_read_to_erase`).
    pub async fn load_saved(
        lease: &dyn SessionStorageLease,
        id: &SessionId,
    ) -> Result<Option<Self>, StorageError> {
        let (saved, _) = lease
            .load()
            .await
            .map_err(StorageError::bounded)?
            .into_published(id)?;
        Ok(saved)
    }

    /// Whether this snapshot, read back from storage, may stand for the
    /// session `id`: its relationships hold and it was saved under `id`. The
    /// one check every reader of saved history applies.
    pub(crate) fn check_saved(&self, id: &SessionId) -> Result<(), StorageError> {
        super::validation::validate(self)?;
        if self.id != *id {
            return Err(StorageError::IdentityMismatch);
        }
        Ok(())
    }

    /// Release rejected adapter evidence without recursive error-tree destruction.
    pub(crate) fn discard_rejected_errors(mut self) {
        for invocation in &mut self.invocations {
            if let Some(Err(error)) = invocation.result.take() {
                error.discard_iteratively();
            }
            if let SubmissionAcknowledgement::Failed { audit, .. } = &mut invocation.acknowledgement
            {
                if let Some(error) = audit.take() {
                    error.discard_iteratively();
                }
            }
        }
    }
}

impl StorageError {
    /// `schemaVersion` written on every session-record batch and transcript checkpoint.
    ///
    /// This build reads this version and writes it on every batch and checkpoint.
    /// A record with no marker is [`Self::AnotherVersion`], as is any other
    /// unsigned integer. During alpha there is no migration and no reader for
    /// an unmarked record. That record drops its save group. The chat shows one
    /// placeholder there and still folds every other record. The stored bytes
    /// stay where they are. Semantic batches and transcript checkpoints share
    /// this number: a change to either shape bumps both.
    pub const SCHEMA_VERSION: u64 = 1;

    /// Aggregate retained diagnostic text budget; structural slots are accounted separately.
    pub(crate) const DIAGNOSTIC_BYTES: usize = 4096;
    fn measure(
        mut pending: Vec<(&StorageError, usize)>,
        mut usage: StorageErrorUsage,
    ) -> Result<StorageErrorUsage, StorageError> {
        while let Some((error, depth)) = pending.pop() {
            usage.nodes = usage.nodes.saturating_add(1);
            usage.depth = usage.depth.max(depth);
            if usage.nodes > DiagnosticTreeLimits::NODES || depth > DiagnosticTreeLimits::DEPTH {
                return Err(Self::DiagnosticLimit);
            }
            match error {
                Self::Io(text) | Self::Corrupt(text) => {
                    usage.allocations = usage.allocations.saturating_add(text.capacity());
                    usage.diagnostics = usage.diagnostics.saturating_add(text.capacity());
                    usage.text = usage.text.saturating_add(text.len());
                }
                Self::ShutdownFailures(failure) => {
                    usage.allocations = usage
                        .allocations
                        .saturating_add(mem::size_of_val(failure.as_ref()));
                    pending.push((failure.read(), depth + 1));
                    pending.push((failure.runtime(), depth + 1));
                }
                _ => {}
            }
        }
        Ok(usage)
    }
    pub(crate) fn retained_usage(&self) -> Result<StorageErrorUsage, StorageError> {
        let usage = Self::measure(vec![(self, 1)], StorageErrorUsage::default())?;
        if usage.allocations > DiagnosticTreeLimits::BYTES {
            return Err(Self::DiagnosticLimit);
        }
        Ok(usage)
    }
    /// Actual owned allocations, excluding the caller-accounted inline enum.
    pub(crate) fn allocation_bytes(&self) -> usize {
        self.retained_usage()
            .map_or(usize::MAX, |usage| usage.allocations)
    }
    pub(crate) fn validate_retained_size(&self) -> Result<(), StorageError> {
        if matches!(self, Self::Io(text) | Self::Corrupt(text) if text.capacity() > Self::DIAGNOSTIC_BYTES)
        {
            return Err(Self::Corrupt(
                "stored acknowledgement diagnostic exceeds limit".into(),
            ));
        }
        let usage = self.retained_usage()?;
        (usage.diagnostics <= Self::DIAGNOSTIC_BYTES)
            .then_some(())
            .ok_or(Self::DiagnosticLimit)
    }
    fn compact_diagnostics(&mut self) {
        let mut pending = vec![self];
        while let Some(error) = pending.pop() {
            match error {
                Self::Io(text) | Self::Corrupt(text) => {
                    *text = mem::take(text).into_boxed_str().into_string()
                }
                Self::ShutdownFailures(failure) => {
                    pending.push(&mut failure.read);
                    pending.push(&mut failure.runtime);
                }
                _ => {}
            }
        }
    }
    /// Compact external diagnostics before an SDK error wrapper or clone retains them.
    pub(crate) fn bounded(self) -> Self {
        self.bounded_diagnostic(Self::DIAGNOSTIC_BYTES)
    }
    pub(crate) fn bounded_diagnostic(self, limit: usize) -> Self {
        fn text(mut value: String, limit: usize) -> String {
            const SUFFIX: &str = " [diagnostic truncated]";
            if value.len() > limit {
                let mut end = limit.saturating_sub(SUFFIX.len());
                while !value.is_char_boundary(end) {
                    end -= 1;
                }
                value.truncate(end);
                value.push_str(SUFFIX);
            }
            value.into_boxed_str().into_string()
        }
        match self {
            Self::Io(value) => Self::Io(text(value, limit)),
            Self::Corrupt(value) => Self::Corrupt(text(value, limit)),
            // Private construction already validated aggregate text and tree shape.
            Self::ShutdownFailures(failure) => Self::ShutdownFailures(failure),
            other => other,
        }
    }
}

#[cfg(test)]
mod committed_tests {
    use super::*;
    use crate::application::agent_execution::executions::ExecutionRequest;
    use crate::application::agent_execution::sessions::{
        CommittedCompleteness, CommittedFreshness,
    };
    use crate::domain::agent_execution::prompts::{PromptText, UserMessage};
    #[test]
    fn typed_shutdown_tree_limits_and_iterative_rejection() {
        let aggregate = |read, runtime| {
            StorageShutdownFailure::new(read, runtime)
                .map(|failure| StorageError::ShutdownFailures(Box::new(failure)))
        };
        let mut exact_depth = StorageError::ReadWorkerPanicked;
        for _ in 1..DiagnosticTreeLimits::DEPTH {
            exact_depth = aggregate(exact_depth, StorageError::Unresolved).unwrap();
        }
        assert_eq!(
            exact_depth.retained_usage().unwrap().depth,
            DiagnosticTreeLimits::DEPTH
        );
        assert_eq!(
            aggregate(exact_depth, StorageError::Closed),
            Err(StorageError::DiagnosticLimit)
        );
        fn tree(level: usize) -> StorageError {
            if level == 0 {
                StorageError::Closed
            } else {
                StorageError::ShutdownFailures(Box::new(
                    StorageShutdownFailure::new(tree(level - 1), tree(level - 1)).unwrap(),
                ))
            }
        }
        let exact_nodes = tree(6);
        let exact_agent_nodes = AgentError::ExecutionObservation {
            error: Box::new(AgentError::Storage(exact_nodes.clone())),
            execution_result: None,
        };
        assert!(exact_agent_nodes.validate_retained_size().is_ok());
        let over_agent_nodes = AgentError::ExecutionObservation {
            error: Box::new(exact_agent_nodes),
            execution_result: None,
        };
        assert!(over_agent_nodes.validate_retained_size().is_err());
        assert_eq!(
            exact_nodes.retained_usage().unwrap().nodes,
            DiagnosticTreeLimits::NODES - 1
        );
        assert_eq!(
            aggregate(exact_nodes, StorageError::Closed),
            Err(StorageError::DiagnosticLimit)
        );
        let exact_text = aggregate(
            StorageError::Io("x".repeat(StorageError::DIAGNOSTIC_BYTES)),
            StorageError::Unresolved,
        )
        .unwrap();
        assert!(exact_text.validate_retained_size().is_ok());
        assert_eq!(
            aggregate(
                StorageError::Io("x".repeat(StorageError::DIAGNOSTIC_BYTES + 1)),
                StorageError::Unresolved
            ),
            Err(StorageError::DiagnosticLimit)
        );
        let mut hostile = StorageError::Closed;
        for _ in 0..100_000 {
            hostile = StorageError::ShutdownFailures(Box::new(StorageShutdownFailure {
                read: hostile,
                runtime: StorageError::Unresolved,
            }));
        }
        assert_eq!(
            aggregate(hostile, StorageError::Closed),
            Err(StorageError::DiagnosticLimit)
        );
    }
    #[test]
    fn shutdown_failure_allocation_accounting_agrees_across_wrappers() {
        let mut read = String::with_capacity(1024 * 1024);
        read.push_str(&"r".repeat(2048));
        let mut runtime = String::with_capacity(1024 * 1024);
        runtime.push_str(&"c".repeat(2048));
        let failure = StorageError::ShutdownFailures(Box::new(
            StorageShutdownFailure::new(StorageError::Io(read), StorageError::Corrupt(runtime))
                .unwrap(),
        ));
        let allocated = mem::size_of::<StorageShutdownFailure>() + 4096;
        assert_eq!(failure.allocation_bytes(), allocated);
        assert!(failure.validate_retained_size().is_ok());
        let wrapped = AgentError::Storage(failure.clone());
        assert_eq!(
            wrapped.retained_size().unwrap(),
            mem::size_of::<AgentError>() + allocated
        );
        let mut snapshot = SessionSnapshot {
            id: SessionId::new("session").unwrap(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            provider_context: ProviderContext::Absent,
            invocations: vec![InvocationRecord {
                request: ExecutionRequest {
                    execution_id: ExecutionId::new("execution").unwrap(),
                    user_message: UserMessage::text_only(PromptText::new("message").unwrap()),
                    estimated_input_tokens: 1,
                    reserved_output_tokens: 1,
                },
                submission: SubmissionMode::Immediate,
                target_event_offset: None,
                actor: ActionContext::new("user", "surface", "request").unwrap(),
                acknowledgement: SubmissionAcknowledgement::Pending,
                events: Vec::new(),
                scheduling: Vec::new(),
                cancellation: None,
                local_cancellation: None,
                provider_report: None,
                result: None,
                local_outcome: None,
            }],
            queue_history: Vec::new(),
        };
        let before = super::super::retained::snapshot(&snapshot);
        snapshot.invocations[0].acknowledgement = SubmissionAcknowledgement::Failed {
            audit: None,
            storage: Some(failure),
        };
        assert_eq!(
            super::super::retained::snapshot(&snapshot) - before,
            allocated
        );
        let mut diagnostic = String::with_capacity(8192);
        diagnostic.push('x');
        let capacity = diagnostic.capacity();
        assert_eq!(StorageError::Io(diagnostic).allocation_bytes(), capacity);
    }
    fn empty() -> CommittedStatus {
        CommittedStatus::new(
            CommittedCompleteness::CompleteEmpty,
            CommittedFreshness::Current,
        )
    }
    #[test]
    fn committed_constructor_validates_positions_head_status_and_incarnation_bytes() {
        let id = SessionId::new("session").unwrap();
        for incarnation in ["x".repeat(128), "é".repeat(64)] {
            let mut external = String::with_capacity(8 * 1024 * 1024);
            external.push_str(&incarnation);
            let view = CommittedSession::new(id.clone(), external, 0, 0, 0, None, empty()).unwrap();
            assert_eq!(view.incarnation(), incarnation);
            assert_eq!(view.clone().incarnation(), incarnation);
        }
        for incarnation in ["x".repeat(129), "é".repeat(65), " ".repeat(128)] {
            assert!(matches!(
                CommittedSession::new(id.clone(), incarnation, 0, 0, 0, None, empty()),
                Err(StorageError::Corrupt(_))
            ));
        }
        for (a, d, head) in [(2, 1, 1), (1, 1, 0), (1, 1, 2)] {
            assert!(matches!(
                CommittedSession::new(id.clone(), "incarnation".into(), a, d, head, None, empty()),
                Err(StorageError::Corrupt(_))
            ));
        }
        let stale = CommittedStatus::new(
            CommittedCompleteness::CompleteEmpty,
            CommittedFreshness::Stale,
        );
        assert!(
            CommittedSession::new(id.clone(), "incarnation".into(), 1, 1, 2, None, stale).is_ok()
        );
        let contradictory =
            CommittedStatus::new(CommittedCompleteness::Complete, CommittedFreshness::Current);
        assert!(matches!(
            CommittedSession::new(id, "incarnation".into(), 0, 0, 0, None, contradictory),
            Err(StorageError::Corrupt(_))
        ));
    }
}
