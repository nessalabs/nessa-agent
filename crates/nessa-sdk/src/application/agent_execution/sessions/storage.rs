//! Typed session snapshot persistence and exclusive access contracts.
#![deny(missing_docs)]

use crate::application::agent_execution::providers::ModelChangeRecord;
use crate::application::agent_execution::providers::SessionConfigurationRecord;

use super::{
    ArtifactRecord, CommittedEventReader, CommittedStatus, CommittedViewState, CurrentLease,
    EventFuture, EventReadError, EventStart, LeaseRecord, NoteDelivery, NoteId, NoteRecord,
    NoteRetiredRecord, NoteState,
};
use crate::application::agent_execution::agents::{AgentError, DiagnosticTreeLimits};
use crate::application::agent_execution::executions::{
    ExecutionEvent, ExecutionRequest, SubmissionMode,
};
use crate::application::agent_execution::permissions::ActionContext;
use crate::application::agent_execution::providers::{ExecutionReport, ProviderIdentity};
pub use crate::domain::agent_execution::sessions::ProviderContext;
use crate::domain::agent_execution::{
    executions::{
        ExecutionId, ExecutionOutcome, Initiation, InvocationCancellation, InvocationKind,
        InvocationStage, QueueMutation, SchedulingCause, SchedulingInitiator, SchedulingTransition,
        SchedulingTransitionError,
    },
    sessions::SessionId,
};
use nessa_sync::replication::domain::Id;
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt, future::Future, mem, pin::Pin, sync::Arc};

/// Asynchronous storage result borrowing its adapter for `'a` and returning `T`.
pub type StorageFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, StorageError>> + Send + 'a>>;

mod save;
pub use save::{
    SessionLoad, SessionLoadState, SessionSaveBackend, SessionSaveGeneration, SessionSaveReceipt,
    SessionSaveUnit,
};

/// Failure to acquire access, read, validate, or persist a session.
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
    /// The records database failed its own checks.
    ///
    /// A schema or accounting check, a poisoned lock, or a stream the store
    /// can no longer name. This is not a failure to decode one conversation's
    /// saved bytes. A later attempt can succeed once the database is usable.
    Store(String),
    /// The record's `schemaVersion` is missing or is not [`Self::SCHEMA_VERSION`].
    ///
    /// The reader does not decode the rest of that record. `found` is the
    /// integer when the field is present. `None` means the field was absent.
    AnotherVersion {
        /// The `schemaVersion` integer in the record, when the field is present.
        found: Option<u64>,
    },
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

/// One saved record the fold could not read. The transcript shows it as a line
/// and keeps every record it could read.
///
/// `id` is the decimal physical position of that record. `after_execution_id`
/// is the last turn the chat had accepted when the gap was recorded, absent
/// when it had accepted none. At most [`Self::MAX_PER_CONVERSATION`] are kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnreadablePart {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after_execution_id: Option<String>,
}
impl UnreadablePart {
    /// Most gap lines one chat keeps. A later record that cannot be read is
    /// still skipped, and the fold records that the list was cut.
    pub const MAX_PER_CONVERSATION: usize = 32;

    pub(crate) fn at(position: u64, after_execution_id: Option<String>) -> Self {
        Self {
            id: position.to_string(),
            after_execution_id,
        }
    }

    /// The record's physical position, as decimal text.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The last accepted turn when this gap was recorded.
    pub fn after_execution_id(&self) -> Option<&str> {
        self.after_execution_id.as_deref()
    }

    pub(crate) fn valid_list(parts: &[Self]) -> bool {
        if parts.len() > Self::MAX_PER_CONVERSATION {
            return false;
        }
        let mut previous = 0u64;
        parts.iter().all(|part| {
            if part.id.is_empty()
                || part.id.len() > 20
                || !part.id.bytes().all(|byte| byte.is_ascii_digit())
                || part.id.starts_with('0')
                || part
                    .after_execution_id
                    .as_ref()
                    .is_some_and(|id| id.is_empty() || id.len() > 256)
            {
                return false;
            }
            // `01` parses as 1, so the leading-zero check above stays. A value
            // past `u64::MAX` is not a physical position. Oldest first: a
            // restored suffix appends later positions, and a list that is
            // already out of order would put a newer gap ahead of an older one.
            let Ok(position) = part.id.parse::<u64>() else {
                return false;
            };
            if position <= previous {
                return false;
            }
            previous = position;
            true
        })
    }
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
    unreadable: Vec<UnreadablePart>,
    unreadable_truncated: bool,
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
            unreadable_truncated: false,
        })
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "one argument per field of the committed read"
    )]
    pub(crate) fn from_transcript(
        id: SessionId,
        incarnation: Id,
        downloaded: u64,
        observed_head: u64,
        applied: u64,
        snapshot: Option<Arc<SessionSnapshot>>,
        status: CommittedStatus,
        unreadable: Vec<UnreadablePart>,
        unreadable_truncated: bool,
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
            unreadable,
            unreadable_truncated,
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
    /// Coarse display completeness and freshness.
    pub fn state(&self) -> CommittedViewState {
        self.status.view_state()
    }
    /// Records in this read that could not be applied, oldest first.
    pub fn unreadable(&self) -> &[UnreadablePart] {
        &self.unreadable
    }
    /// A record past the 32 kept gaps was also skipped.
    pub fn unreadable_truncated(&self) -> bool {
        self.unreadable_truncated
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
    /// Complete native settings acknowledged by the last explicit model selection.
    pub model_change: Option<ModelChangeRecord>,
    /// Latest attributed configuration control; restoration uses this current value.
    pub current_configuration: Option<SessionConfigurationRecord>,
    /// Submitted invocations in admission order, including unresolved work.
    pub invocations: Vec<InvocationRecord>,
    /// Append-only global queue membership/order facts. Local selection precedes
    /// provider dispatch; restoration clears pending membership without replay.
    pub queue_history: Vec<QueueHistoryRecord>,
    /// The conversation's latest lease and its records, or `None` before any
    /// lease was recorded. Earlier leases stay in the stream; only the latest
    /// is folded here.
    pub lease: Option<CurrentLease>,
    /// Artifacts the conversation holds, oldest first: append-only, at most
    /// [`ArtifactRecord::MAX_PER_CONVERSATION`].
    pub artifacts: Vec<ArtifactRecord>,
    /// Notes left in the conversation, oldest first: append-only, at most
    /// [`NoteRecord::MAX_PER_CONVERSATION`].
    pub notes: Vec<NoteRecord>,
}

/// One SDK decision retained for an atomic semantic-record save.
///
/// A record writer receives these in decision order. An adapter that stores
/// whole snapshots applies them with [`SessionSaveUnit::fold`]. No variant
/// authorizes replay to run a provider or tool effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionChange {
    /// Provider-verified same-provider model selection, preserving history and context.
    ModelChanged(ModelChangeRecord),
    /// A verified approval or effort change, retaining complete before/after settings.
    ConfigurationChanged(SessionConfigurationRecord),
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
    /// One fact about the conversation's lease, validated by the lease rules
    /// before it is committed and again when it is read back.
    Lease(LeaseRecord),
    /// A file published under the conversation's live lease, now held by the
    /// conversation. Checked against the latest lease before it is committed
    /// and again when it is read back.
    Artifact(ArtifactRecord),
    /// A note left in the conversation, which starts and steers no turn.
    /// Checked against the notes before it is committed and again when it is
    /// read back.
    Note(NoteRecord),
    /// A pending `next_turn` note retired before any turn took it. Checked
    /// against the notes before it is committed and again when it is read
    /// back.
    NoteRetired(NoteRetiredRecord),
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

    /// The note `principal` left under request `request`, if it left one: a
    /// read of saved evidence, which is how a sender finds its note again.
    pub fn note(&self, principal: &str, request: &str) -> Option<&NoteRecord> {
        self.notes
            .iter()
            .find(|note| note.answers(principal, request))
    }

    /// Where `note` stands, read from this snapshot: the turn whose saved
    /// acceptance names it, its retirement, or neither.
    pub fn note_state(&self, note: &NoteRecord) -> NoteState {
        if note.delivery == NoteDelivery::RecordOnly {
            return NoteState::Recorded;
        }
        if let Some(retirement) = note.retired {
            return NoteState::Retired(retirement);
        }
        self.invocations
            .iter()
            .find(|record| record.notes.contains(&note.id))
            .map_or(NoteState::Pending, |record| {
                NoteState::Assigned(record.request.execution_id.clone())
            })
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
    /// Whether the conversation's owner or a share grant started this turn, and
    /// whether its tools were checked. Retries must preserve it.
    pub initiation: Initiation,
    /// The `next_turn` notes this turn took, in saved order, assigned by
    /// saving this record. The conversation chooses them at admission; a
    /// retry finds them here and chooses nothing again.
    pub notes: Vec<NoteId>,
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
    /// Open an independent semantic observer without acquiring a writer lease.
    /// Snapshot adapters without retained decisions return Unsupported. Dropping
    /// the reader does not stop provider work. The host authorizes access first.
    ///
    /// # Errors
    /// Returns unsupported, missing source, invalid cursor or storage failure.
    fn events(
        &self,
        _id: SessionId,
        _start: EventStart,
    ) -> EventFuture<'_, Box<dyn CommittedEventReader>> {
        Box::pin(async { Err(EventReadError::Unsupported) })
    }

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

    /// Remove history this build cannot open, without reading it.
    ///
    /// Delete calls this when opening the history returns
    /// [`StorageError::AnotherVersion`] or [`StorageError::Corrupt`]: this
    /// conversation's saved bytes do not decode. [`StorageError::Store`] is
    /// the records database and is not discarded. The default refuses, so an
    /// adapter that can hold that history must
    /// override it or the delete stays incomplete. The record adapter drops
    /// a legacy journal if one is there and resets the stream without replay.
    /// After that reset is acknowledged it publishes to the session's
    /// committed-change watches, then releases the writer reservation. Lease
    /// records in the refused stream are not read. A reservation already held
    /// for the session refuses the reset and publishes nothing.
    ///
    /// # Errors
    /// The default returns [`StorageError::Corrupt`]. [`StorageError::Busy`]
    /// when another owner holds the session. A backend error when the reset
    /// cannot be acknowledged.
    fn discard_unreadable(&self, id: SessionId) -> StorageFuture<'_, ()> {
        let _ = id;
        Box::pin(async {
            Err(StorageError::Corrupt(
                "this storage does not discard unreadable history".into(),
            ))
        })
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

    /// Persist explicit indivisible semantic units. The units are the only
    /// account of what changed: the saved state is the prior state with them
    /// applied, in order, and a caller's own state is built the same way, so
    /// nothing else is passed or compared. Every unit applies before pending
    /// reconciliation or append, at the cost of the units, not the history.
    /// Record adapters keep all units unpublished until one durable completion
    /// terminal.
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
    /// durable units, a unit that does not apply, and unavailable persistence. A failure is not proof
    /// of absence and cannot release a reserved command identity for redispatch.
    fn save_changes(
        &self,
        binding: SessionSaveGeneration,
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
    /// Aggregate retained diagnostic text budget; structural slots are accounted separately.
    pub(crate) const DIAGNOSTIC_BYTES: usize = 4096;
    /// `schemaVersion` written on every session-record batch and transcript checkpoint.
    ///
    /// This build reads only [`Self::SCHEMA_VERSION`]. A record stamped 1 or 2
    /// (before turns saved who started them), a finished object with no
    /// marker, or any other unsigned integer is [`Self::AnotherVersion`] and is
    /// not decoded. A marker that is not an
    /// unsigned integer is [`Self::Corrupt`]. A missing marker together with a
    /// body the walk refuses is also [`Self::Corrupt`]: the broken body is
    /// reported, not another version. Old files are deleted by hand. Nothing
    /// is migrated.
    pub const SCHEMA_VERSION: u64 = 3;
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
                Self::Io(text) | Self::Corrupt(text) | Self::Store(text) => {
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
        if matches!(self, Self::Io(text) | Self::Corrupt(text) | Self::Store(text) if text.capacity() > Self::DIAGNOSTIC_BYTES)
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
                Self::Io(text) | Self::Corrupt(text) | Self::Store(text) => {
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
            Self::Store(value) => Self::Store(text(value, limit)),
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
    use crate::domain::agent_execution::executions::GrantInitiator;
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
            model_change: None,
            current_configuration: None,
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
                initiation: Initiation::owner(),
                notes: Vec::new(),
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
            lease: None,
            artifacts: Vec::new(),
            notes: Vec::new(),
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
        // A share grant's identity text is retained with the turn; an owner
        // holds none.
        let before = super::super::retained::snapshot(&snapshot);
        snapshot.invocations[0].initiation =
            Initiation::grant(GrantInitiator::new("share-grant", 3).unwrap());
        assert_eq!(
            super::super::retained::snapshot(&snapshot) - before,
            "share-grant".len()
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
