use super::{
    app_sources,
    artifacts::ArtifactRefusal,
    attachment::AttachmentLease,
    notes::{NoteAdmission, NoteRefusal, NoteSelection},
    records::{continuation::Continuation, AppliedUnits},
    steering_position::SteeringPosition,
    ArtifactRecord, CurrentLease, InvocationCancellationEvent, InvocationRecord,
    InvocationSchedulingEvent, LeaseRecord, MessageCommitClock, MessageCommitSleep, NoteCandidates,
    NoteRecord, NoteRequest, ProviderContext, QueueHistoryRecord, SessionChange, SessionLoadState,
    SessionSaveGeneration, SessionSaveUnit, SessionSnapshot, SessionStorage, SessionStorageLease,
    StorageError, StorageFuture, SubmissionAcknowledgement,
};
use crate::application::agent_execution::agents::OwnedCleanupScope;
use crate::application::agent_execution::{
    agents::AgentError,
    caller_wake::{contain_caller_wake, CallerWaiter},
    executions::{
        limits::ObservationUsage, ExecutionEvent, ExecutionRequest, ExecutionUpdate,
        SubmissionMode, Submitter,
    },
    permissions::ActionContext,
    providers::{
        AgentProvider, ExecutionEventStream, ExecutionReport, ExecutionReportSource,
        FailedOpenCauseSource, FailedOpenCleanup, ModelChange, ModelChangeRecord,
        ModelChangeTiming, ModelConfiguration, ProviderOpenControl, ProviderOpenRequest,
        ProviderSession, SessionConfigurationKind, SessionConfigurationRecord,
    },
    tools::ToolReviewInput,
};
use crate::domain::agent_execution::{
    executions::{
        ExecutionId, ExecutionOutcome, Initiation, InvocationKind, InvocationStage, QueueMutation,
        QueueOrderChange, SchedulingCause,
    },
    permissions::PermissionRequest,
    prompts::NoteContext,
    sessions::SessionId,
};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    future::poll_fn,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    task::Poll,
    time::Duration,
};
use tokio::sync::Mutex;
use uuid::Uuid;

pub(crate) struct AttachmentOpenError {
    pub(crate) cause: AgentError,
    pub(crate) source: AttachmentOpenFailureSource,
}

/// A validated scheduling decision was applied to local evidence; its
/// persistence acknowledgement is a separate fact.
pub(crate) struct SchedulingCommit {
    pub saved: Result<(), StorageError>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttachmentOpenFailureSource {
    Independent,
    FailedOpenCleanup,
}
impl AttachmentOpenError {
    fn independent(cause: AgentError) -> Self {
        Self {
            cause,
            source: AttachmentOpenFailureSource::Independent,
        }
    }
    fn failed_open(cause: AgentError, source: FailedOpenCauseSource) -> Self {
        Self {
            cause,
            source: match source {
                FailedOpenCauseSource::Independent => AttachmentOpenFailureSource::Independent,
                FailedOpenCauseSource::CleanupReport => {
                    AttachmentOpenFailureSource::FailedOpenCleanup
                }
            },
        }
    }
}

/// What [`SessionManager::record_lease`] decided and whether it is durable.
#[derive(Debug)]
#[must_use = "a lease record that was not saved must be reported"]
pub struct LeaseCommit<T> {
    /// The caller's value from deciding.
    pub decided: T,
    /// Whether the records are durable. An error leaves them retained, and
    /// the next save writes them.
    pub saved: Result<(), LeaseRecordError>,
}

/// Why lease records were not recorded, or not yet saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaseRecordError {
    /// The manager has not prepared the conversation evidence.
    NotLoaded,
    /// The lease rules refuse a record; nothing was retained.
    Refused(StorageError),
    /// A record of a kind this build cannot read was offered; such records
    /// are only ever read back, never written. Nothing was retained.
    Unreadable,
    /// The records were retained but writing them failed.
    Storage(StorageError),
}
impl fmt::Display for LeaseRecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotLoaded => formatter.write_str("the conversation is not loaded"),
            Self::Refused(error) => write!(formatter, "lease record refused: {error}"),
            Self::Unreadable => formatter.write_str("an unreadable lease record cannot be written"),
            Self::Storage(error) => write!(formatter, "lease record not saved: {error}"),
        }
    }
}
impl std::error::Error for LeaseRecordError {}

/// Why [`SessionManager::record_artifact`] did not record an artifact, or did
/// not yet save it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtifactRecordError {
    /// The manager has not prepared the conversation evidence.
    NotLoaded,
    /// The conversation already records
    /// [`ArtifactRecord::MAX_PER_CONVERSATION`] artifacts. Nothing was
    /// retained.
    Full,
    /// The record's lease is not the conversation's latest, is not live, or
    /// was not issued by the record's actor; or the latest lease cannot be
    /// read, so which it is cannot be told. Nothing was retained.
    NotThisLease,
    /// The record names a turn the conversation did not accept. Nothing was
    /// retained.
    UnknownTurn,
    /// The record was retained but writing it failed; the next save writes it.
    Storage(StorageError),
}
impl fmt::Display for ArtifactRecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotLoaded => formatter.write_str("the conversation is not loaded"),
            Self::Full => formatter.write_str("the conversation records its most artifacts"),
            Self::NotThisLease => {
                formatter.write_str("the artifact's lease is not the conversation's live lease")
            }
            Self::UnknownTurn => {
                formatter.write_str("the artifact names a turn the conversation did not accept")
            }
            Self::Storage(error) => write!(formatter, "artifact record not saved: {error}"),
        }
    }
}
impl std::error::Error for ArtifactRecordError {}

/// Why [`SessionManager::record_note`] did not record a note, or did not yet
/// save it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoteRecordError {
    /// The manager has not prepared the conversation evidence.
    NotLoaded,
    /// The principal's request already left another note: another text,
    /// delivery, surface or authority under the same request. Nothing was
    /// retained.
    Conflict,
    /// The conversation already keeps [`NoteRecord::MAX_PER_CONVERSATION`]
    /// notes. Nothing was retained.
    Full,
    /// The author already left [`NoteRecord::MAX_PER_AUTHOR_IN_WINDOW`] notes
    /// in the last [`NoteRecord::RATE_WINDOW_MS`]. Nothing was retained.
    RateLimited,
    /// The note was retained but writing it failed; the next save, or the
    /// same request again, writes it.
    Storage(StorageError),
}
impl fmt::Display for NoteRecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotLoaded => formatter.write_str("the conversation is not loaded"),
            Self::Conflict => {
                formatter.write_str("the request already left another note in the conversation")
            }
            Self::Full => formatter.write_str("the conversation keeps its most notes"),
            Self::RateLimited => formatter.write_str("the author left its most notes for now"),
            Self::Storage(error) => write!(formatter, "note not saved: {error}"),
        }
    }
}
impl std::error::Error for NoteRecordError {}

/// What a turn's admission is told about the conversation's pending notes:
/// the host's verdict on each author it could check, and whether the turn
/// may take any. A turn that may not still retires what the verdict or the
/// clock ends.
#[derive(Clone, Debug, Default)]
pub(crate) struct NoteIntake {
    pub(crate) candidates: NoteCandidates,
    pub(crate) takes_notes: bool,
}

/// Owns the local conversation key, its exclusive storage lease, and saved evidence.
/// Provider-private model/tool state is restored by the selected provider.
/// Opening acquires storage access; attaching this manager to an Agent loads and
/// verifies saved evidence. Provider close leaves the lease held so the same
/// Agent can resume safely. Dropping an attached manager keeps storage access
/// until supervised writes and provider cleanup finish. Uncertain cleanup is
/// retried with bounded backoff; the Tokio runtime must remain alive until it ends.
/// An unattached manager has no provider resources and releases access on drop.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use nessa_sdk::{
///     application::agent_execution::sessions::SessionManager,
///     domain::agent_execution::sessions::SessionId,
///     infrastructure::session_storage::{InMemoryStorage, RuntimeMessageCommitClock},
/// };
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let storage = Arc::new(InMemoryStorage::new());
/// let manager = SessionManager::open(
///     Some(SessionId::new("conversation")?),
///     storage,
///     Arc::new(RuntimeMessageCommitClock::new()),
/// ).await?;
/// assert!(manager.snapshot().await.is_none()); // No Agent attached yet.
/// drop(manager); // Releases the writer lease without deleting saved data.
/// # Ok(())
/// # }
/// ```
pub struct SessionManager {
    id: SessionId,
    message_commit_clock: Arc<dyn MessageCommitClock>,
    // One exclusive lease; supervised admission writes retain this same handle
    // until they publish their result, even when the caller drops its future.
    storage_lease: Arc<dyn SessionStorageLease>,
    evidence: Arc<Mutex<Evidence>>,
    // Live dispatch authority is independent of asynchronous snapshot writes.
    // Register and poll the provider in one task poll; sharing the evidence mutex
    // could deadlock when event handling wins select while dispatch waits for it.
    // Ordinary output authority ends with the invocation observation scope.
    // Retained IDs authorize only validated trailing permission cancellations.
    // Imported history never reconstructs this live authority.
    dispatched: RwLock<HashMap<ExecutionId, ObservationAuthority>>,
    attachment: Arc<AttachmentLease>,
    // Set by prepare when the fingerprint differs and this provider would
    // resume the saved native session. attach opens a new one and saves the
    // new fingerprint with that session's id, in one write. In memory only:
    // a crash before that save leaves the durable identity unchanged, so the
    // next open takes the same path (docs/design/restoration-fingerprint.md, S6).
    open_fresh_context: AtomicBool,
}
#[derive(Clone, Copy)]
enum ObservationAuthority {
    Output,
    CancellationOnly,
}
#[derive(Clone)]
pub(crate) struct AttachedProvider {
    pub(crate) session: ProviderSession,
    pub(crate) events: Arc<Mutex<Box<dyn ExecutionEventStream>>>,
}
// Retire output authority even if provider polling, hooks, or storage panic.
// The provider reply alone does not end observation: buffered events still drain.
pub(crate) struct InvocationObservations<'a> {
    manager: &'a SessionManager,
    id: &'a ExecutionId,
}
impl Drop for InvocationObservations<'_> {
    fn drop(&mut self) {
        self.manager.retire_observations(self.id);
    }
}
#[derive(Default)]
struct Evidence {
    save_generation: Option<SessionSaveGeneration>,
    event_usage: HashMap<ExecutionId, ObservationUsage>,
    // The conversation as observed: the saved records with `pending` applied
    // after them. It changes only by applying a unit ([`Self::record`]), the
    // same way storage applies it, so it can never disagree with what is saved.
    observed: Continuation,
    // How to undo each pending unit, so the saved conversation can be read
    // without keeping a copy of it.
    applied: AppliedUnits,
    // Decisions retained after observation but before a confirmed save. A
    // retry submits the same ordered facts, including earlier failed writes.
    pending: Vec<SessionSaveUnit>,
    message_commit: Option<PendingMessageCommit>,
}
#[derive(Clone)]
struct PendingMessageCommit {
    generation: SessionSaveGeneration,
    deadline: Duration,
    bytes: usize,
    retained_bytes: usize,
    count: usize,
}
impl Evidence {
    fn generation(&self) -> Result<SessionSaveGeneration, StorageError> {
        self.save_generation
            .clone()
            .ok_or_else(|| StorageError::Corrupt("session has not loaded its save binding".into()))
    }
    fn observed(&self) -> Option<&SessionSnapshot> {
        self.observed.snapshot.as_ref()
    }
    /// Apply one decision boundary to the observed conversation and retain it
    /// for the next save. A unit that does not apply changes nothing.
    fn record(&mut self, changes: Vec<SessionChange>) -> Result<(), StorageError> {
        let unit = SessionSaveUnit::new(changes.clone())?;
        self.observed.apply_kept(changes, &mut self.applied)?;
        self.pending.push(unit);
        Ok(())
    }
    /// Forget every pending unit after the first `keep`, as if never observed.
    fn forget_after(&mut self, keep: usize) {
        self.observed.revert_after(&mut self.applied, keep);
        self.pending.truncate(keep);
    }
    /// The pending units are durable: the observed conversation is saved.
    fn saved(&mut self, next: SessionSaveGeneration) {
        self.applied = AppliedUnits::default();
        self.pending.clear();
        self.message_commit = None;
        self.save_generation = Some(next);
    }
    /// The conversation as last saved: the observed one without its pending
    /// units. Its cost is a copy of it plus the pending units, never a
    /// comparison or a second retained copy.
    fn committed(&mut self) -> Option<SessionSnapshot> {
        self.observed
            .before_applied(&mut self.applied, |state| state.snapshot.clone())
            .expect("pending units apply again to the state they applied to")
    }
}
const MESSAGE_COMMIT_DELAY: Duration = Duration::from_millis(100);
const MESSAGE_COMMIT_BYTES: usize = 16 * 1024;
const MESSAGE_COMMIT_COUNT: usize = 64;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Preparation {
    Provider,
    LeaseRecord,
}

impl SessionManager {
    /// Acquires exclusive storage access for one local session.
    ///
    /// # Parameters
    /// - `id`: Optional local conversation key, distinct from the provider context ID.
    ///   `None` creates a fresh UUID v4 key; `Some` opens the supplied named or saved
    ///   key. Retain [`Self::id`] to reopen a generated session later.
    /// - `storage`: Shared backend used to acquire the lease. The manager retains
    ///   only the returned lease, which must keep its backend resources alive.
    /// - `message_commit_clock`: Monotonic application timer for streaming
    ///   output and Agent scheduling-audit acknowledgement deadlines. Message-save
    ///   wakes ask this manager to recheck pending facts.
    ///
    /// # Errors
    /// Returns [`StorageError::Busy`] if another owner holds this session, or a
    /// backend error if access fails. No provider is opened and no model is called.
    /// Dropping the manager releases access, including before attaching an Agent.
    pub async fn open(
        id: Option<SessionId>,
        storage: Arc<dyn SessionStorage>,
        message_commit_clock: Arc<dyn MessageCommitClock>,
    ) -> Result<Self, StorageError> {
        let id = id.unwrap_or_else(|| {
            SessionId::new(Uuid::new_v4().to_string())
                .expect("UUID v4 is a valid portable session key")
        });
        let storage_lease = storage
            .open(id.clone())
            .await
            .map_err(StorageError::bounded)?;
        Ok(Self {
            id,
            message_commit_clock,
            storage_lease: Arc::from(storage_lease),
            evidence: Arc::new(Mutex::new(Evidence::default())),
            dispatched: RwLock::new(HashMap::new()),
            attachment: Arc::new(AttachmentLease::empty()),
            open_fresh_context: AtomicBool::new(false),
        })
    }
    /// Returns the local conversation key used to acquire storage access.
    pub fn id(&self) -> &SessionId {
        &self.id
    }
    /// Last snapshot acknowledged by storage. None until evidence preparation.
    /// Failed writes never appear here as committed evidence.
    ///
    /// Waiting for the evidence lock registers the polling task's `Waker`
    /// with it; a panic from that waker when a save releases the lock is
    /// logged and does not fail the save. See "Caller wakers" in
    /// docs/agent_execution/lifecycle.md.
    pub async fn snapshot(&self) -> Option<SessionSnapshot> {
        let waiter = CallerWaiter::CommittedSnapshot(self.id.clone());
        contain_caller_wake(waiter, async { self.evidence.lock().await.committed() }).await
    }
    #[cfg(test)]
    pub(crate) async fn pending_message_deadline(
        &self,
    ) -> Option<(SessionSaveGeneration, Duration)> {
        self.evidence
            .lock()
            .await
            .message_commit
            .as_ref()
            .map(|pending| (pending.generation.clone(), pending.deadline))
    }
    pub(crate) fn try_pending_message_deadline(
        &self,
    ) -> Option<Option<(SessionSaveGeneration, Duration)>> {
        self.evidence.try_lock().ok().map(|evidence| {
            evidence
                .message_commit
                .as_ref()
                .map(|pending| (pending.generation.clone(), pending.deadline))
        })
    }
    pub(crate) async fn wait_for_message_deadline(&self, deadline: Duration) {
        self.message_commit_clock.sleep_until(deadline).await;
    }
    /// Share the injected clock with Agent audit observation owners.
    pub(crate) fn message_commit_clock(&self) -> Arc<dyn MessageCommitClock> {
        self.message_commit_clock.clone()
    }

    /// Elapsed time on the injected message-commit clock.
    pub(crate) fn message_commit_now(&self) -> Duration {
        self.message_commit_clock.now()
    }
    /// Wake when the injected message-commit clock reaches `deadline`.
    pub(crate) fn message_commit_sleep_until(&self, deadline: Duration) -> MessageCommitSleep {
        self.message_commit_clock.sleep_until(deadline)
    }
    pub(crate) async fn flush_due_messages(
        &self,
        generation: SessionSaveGeneration,
    ) -> Result<(), StorageError> {
        let mut evidence = self.evidence.lock().await;
        if evidence.message_commit.as_ref().is_some_and(|pending| {
            pending.generation == generation && self.message_commit_clock.now() >= pending.deadline
        }) {
            self.save_observed(&mut evidence).await?;
        }
        Ok(())
    }
    /// Saves a streaming message that is waiting for its commit deadline now,
    /// so a wait that cannot poll that deadline does not hold it back.
    pub(crate) async fn flush_pending_messages(&self) -> Result<(), StorageError> {
        let mut evidence = self.evidence.lock().await;
        if evidence.message_commit.is_some() {
            self.save_observed(&mut evidence).await?;
        }
        Ok(())
    }
    /// Joins admission writes after the Agent has excluded new invocations.
    pub(crate) async fn await_admission_writes(&self) {
        drop(self.evidence.lock().await);
    }
    pub(crate) async fn record_configuration(
        &self,
        initial_before: ModelConfiguration,
        after: ModelConfiguration,
        actor: ActionContext,
        kind: SessionConfigurationKind,
    ) -> Result<(), StorageError> {
        let mut evidence = self.evidence.lock().await;
        let snapshot = evidence.observed().ok_or_else(|| {
            StorageError::Corrupt("configuration change before preparation".into())
        })?;
        let record = SessionConfigurationRecord::new(
            Some(
                snapshot
                    .current_configuration
                    .as_ref()
                    .map_or(initial_before, |record| record.after().clone()),
            ),
            after,
            actor,
            kind,
        );
        evidence.record(vec![SessionChange::ConfigurationChanged(record)])?;
        catch_storage_operation(|| Box::pin(self.save_observed(&mut evidence))).await
    }

    /// Read the latest committed attributed settings without opening a provider.
    ///
    /// # Errors
    /// Returns storage validation failures or Unresolved for an unfinished save.
    pub async fn saved_configuration(
        &self,
    ) -> Result<Option<SessionConfigurationRecord>, AgentError> {
        let saved = catch_storage_operation(|| self.storage_lease.load())
            .await
            .map_err(AgentError::Storage)?;
        if saved.state() == SessionLoadState::Unfinished {
            return Err(AgentError::Storage(StorageError::Unresolved));
        }
        let (snapshot, _) = saved.into_checked(&self.id).map_err(AgentError::Storage)?;
        Ok(snapshot.and_then(|snapshot| snapshot.current_configuration))
    }

    /// Read the last committed attributed model transition without opening a provider.
    ///
    /// # Errors
    /// Returns storage validation failures or Unresolved for an unfinished save.
    pub async fn saved_model_change(&self) -> Result<Option<ModelChangeRecord>, AgentError> {
        let saved = catch_storage_operation(|| self.storage_lease.load())
            .await
            .map_err(AgentError::Storage)?;
        if saved.state() == SessionLoadState::Unfinished {
            return Err(AgentError::Storage(StorageError::Unresolved));
        }
        let (snapshot, _) = saved.into_checked(&self.id).map_err(AgentError::Storage)?;
        Ok(snapshot.and_then(|snapshot| snapshot.model_change))
    }

    /// Change an unattached history's model selection before preparing its next Agent.
    /// This is explicit selection authority, not a relaxation of normal restore identity.
    /// The exact provider context, inputs and observations are retained unchanged.
    ///
    /// # Errors
    /// Refuses a manager already prepared, unfinished writes, another prior identity,
    /// queued or active history, and storage failures. An empty history needs no
    /// rebinding and is left empty for the next Agent to initialize.
    pub async fn change_model(
        &mut self,
        change: ModelChange,
        actor: ActionContext,
    ) -> Result<(), AgentError> {
        if self.evidence.lock().await.observed().is_some() || self.attachment.needs_cleanup() {
            return Err(AgentError::Busy);
        }
        let saved = catch_storage_operation(|| self.storage_lease.load())
            .await
            .map_err(AgentError::Storage)?;
        if saved.state() == SessionLoadState::Unfinished {
            return Err(AgentError::Storage(StorageError::Unresolved));
        }
        let (saved, generation) = saved.into_checked(&self.id).map_err(AgentError::Storage)?;
        let Some(snapshot) = saved else {
            return Ok(());
        };
        if &snapshot.provider != change.before() {
            return Err(AgentError::Storage(StorageError::IdentityMismatch));
        }
        let pending = super::queue_validation::replay(&snapshot).map_err(AgentError::Storage)?;
        if !pending.is_empty()
            || snapshot.invocations.iter().any(|record| {
                record.scheduling.last().is_some_and(|edge| {
                    matches!(
                        edge.stage,
                        InvocationStage::Queued
                            | InvocationStage::Running
                            | InvocationStage::Injected
                    )
                }) || (record.scheduling.is_empty() && record.result.is_none())
            })
        {
            return Err(AgentError::Busy);
        }
        let configuration = change.requested_configuration().clone();
        let record = ModelChangeRecord::new(change, actor, configuration, None);
        let unit = SessionSaveUnit::new(vec![SessionChange::ModelChanged(record)])
            .map_err(AgentError::Storage)?;
        let receipt = catch_storage_operation(|| {
            self.storage_lease
                .save_changes(generation.clone(), vec![unit])
        })
        .await
        .map_err(AgentError::Storage)?;
        receipt
            .next_for(&generation, 1)
            .map_err(AgentError::Storage)?;
        Ok(())
    }

    /// Record one provider-acknowledged model choice while retaining this history and context.
    pub(crate) async fn record_model_change(
        &self,
        change: ModelChange,
        configuration: ModelConfiguration,
        timing: ModelChangeTiming,
        actor: ActionContext,
    ) -> Result<(), StorageError> {
        let mut evidence = self.evidence.lock().await;
        let snapshot = evidence.observed().ok_or_else(|| {
            StorageError::Corrupt("model change before session preparation".into())
        })?;
        if &snapshot.provider != change.before() {
            return Err(StorageError::IdentityMismatch);
        }
        let record = ModelChangeRecord::new(change, actor, configuration, Some(timing));
        evidence.record(vec![SessionChange::ModelChanged(record)])?;
        catch_storage_operation(|| Box::pin(self.save_observed(&mut evidence))).await
    }

    /// Prepare saved evidence for recording a refused lease before an Agent exists.
    ///
    /// Uses the same checked load, continuation validation and initialization
    /// save as Agent preparation. Existing provider identity, context and queue
    /// are preserved: no provider is restored, opened or dispatched. `provider`
    /// supplies the initial identity only for a conversation with no history.
    /// A later `Agent::prepare` still checks its selected provider normally.
    ///
    /// The manager retains its exclusive storage lease. Preparation flushes
    /// retained decisions through their existing save owner before reloading
    /// storage; a failed flush leaves those decisions retained and returns an
    /// error. Mutable access excludes concurrent preparation. Dropping this unattached manager
    /// releases storage access; no provider resources have been acquired.
    ///
    /// # Errors
    /// Returns an Agent storage error for unreadable or conflicting evidence,
    /// unresolved initialization, or a failed initialization save.
    pub async fn prepare_for_lease_record(
        &mut self,
        provider: &dyn AgentProvider,
    ) -> Result<(), AgentError> {
        self.prepare_evidence(provider, Preparation::LeaseRecord)
            .await
            .map(|_| ())
    }

    pub(crate) async fn prepare(
        &mut self,
        provider: &dyn AgentProvider,
    ) -> Result<ProviderContext, AgentError> {
        self.prepare_evidence(provider, Preparation::Provider).await
    }

    async fn prepare_evidence(
        &mut self,
        provider: &dyn AgentProvider,
        preparation: Preparation,
    ) -> Result<ProviderContext, AgentError> {
        catch_storage_operation(|| Box::pin(self.flush_observed()))
            .await
            .map_err(AgentError::Storage)?;
        let saved = catch_storage_operation(|| self.storage_lease.load())
            .await
            .map_err(StorageError::bounded)
            .map_err(AgentError::Storage)?;
        // Initialization derives its plan only from the prior publication and
        // the provider identity, so a plan this method left unfinished — its
        // Unit durable, its completion refused or never written — is derived
        // again here and retried exactly. The writer refuses the retry when
        // its durable units differ from this plan, and that refusal is reported
        // as unresolved. It cannot compare units that never became durable, so
        // a longer plan whose durable prefix is exactly this one would be
        // completed as this plan (design row O12).
        let unfinished = saved.state() == SessionLoadState::Unfinished;
        let (saved, mut save_generation) =
            saved.into_checked(&self.id).map_err(AgentError::Storage)?;
        let compacted = saved;
        let identity = provider.identity();
        // Provider preparation follows the restoration-fingerprint table. A
        // lease-only cleanup record neither changes provider identity nor
        // cancels an already selected fresh-context attachment.
        if preparation == Preparation::Provider {
            self.open_fresh_context.store(false, Ordering::Relaxed);
        }
        let moved = match (&compacted, preparation) {
            (Some(snapshot), Preparation::Provider) if snapshot.provider != identity => {
                if snapshot.provider.name() != identity.name()
                    || snapshot.provider.model_id() != identity.model_id()
                {
                    return Err(AgentError::Storage(StorageError::IdentityMismatch));
                }
                let restores = snapshot
                    .provider_context
                    .recorded()
                    .is_some_and(|saved| provider.restores_saved_context(saved));
                if restores {
                    self.open_fresh_context.store(true, Ordering::Relaxed);
                    None
                } else {
                    Some(SessionChange::ProviderIdentityChanged {
                        before: snapshot.provider.clone(),
                        after: identity.clone(),
                    })
                }
            }
            _ => None,
        };
        let created = compacted.is_none();
        let restore_queue = preparation == Preparation::Provider
            && match &compacted {
                Some(snapshot) => !super::queue_validation::replay(snapshot)
                    .map_err(AgentError::Storage)?
                    .is_empty(),
                None => false,
            };
        let mut observed = Continuation::restore(compacted).map_err(AgentError::Storage)?;
        let mut changes = if created {
            vec![SessionChange::Opened {
                id: self.id.clone(),
                provider: identity,
                context: ProviderContext::Absent,
            }]
        } else {
            Vec::new()
        };
        changes.extend(moved);
        if restore_queue {
            changes.push(SessionChange::QueueDecision(QueueHistoryRecord {
                mutation: QueueMutation::Restored,
                actor: None,
                scheduling_length: None,
            }));
        }
        if unfinished && changes.is_empty() {
            return Err(AgentError::Storage(StorageError::Unresolved));
        }
        if !changes.is_empty() {
            observed.apply_unit(&changes).map_err(AgentError::Storage)?;
            let unit = SessionSaveUnit::new(changes).map_err(AgentError::Storage)?;
            let receipt = catch_storage_operation(|| {
                self.storage_lease
                    .save_changes(save_generation.clone(), vec![unit])
            })
            .await
            .map_err(StorageError::bounded)
            .map_err(|error| match error {
                // The writer refuses a plan that differs from the unfinished
                // one before appending anything. It has no separate variant for
                // that refusal, so every corruption refusal of this retry is
                // reported as unresolved, including a physical conflict or a
                // store error.
                StorageError::Corrupt(_) if unfinished => StorageError::Unresolved,
                error => error,
            })
            .map_err(AgentError::Storage)?;
            save_generation = receipt
                .next_for(&save_generation, 1)
                .map_err(AgentError::Storage)?;
        }
        // S6 returns no recorded context so the lifecycle opens as an initial
        // attachment. The evidence keeps the saved context until attach
        // replaces it: clearing it to Absent would fail once a turn has run.
        let context = if self.open_fresh_context.load(Ordering::Relaxed) {
            ProviderContext::Absent
        } else {
            observed
                .snapshot
                .as_ref()
                .expect("prepared session")
                .provider_context
                .clone()
        };
        *self.evidence.lock().await = Evidence {
            save_generation: Some(save_generation),
            event_usage: HashMap::new(),
            observed,
            applied: AppliedUnits::default(),
            pending: Vec::new(),
            message_commit: None,
        };
        Ok(context)
    }

    pub(crate) async fn attach(
        &self,
        provider: &dyn AgentProvider,
        control: ProviderOpenControl,
    ) -> Result<AttachedProvider, AttachmentOpenError> {
        // Held until this returns: every path that keeps what the open
        // launched arms the lease first, so an observer never sees neither.
        let _open = self.attachment.opening();
        // Saved context is restored only where it was saved and the durable
        // fingerprint still matches. A context-only mismatch (S6) and a
        // provider that starts elsewhere open new context, recorded below.
        let fresh = self.open_fresh_context.load(Ordering::Relaxed);
        let restore = if fresh {
            None
        } else {
            self.evidence
                .lock()
                .await
                .observed()
                .expect("prepared session evidence")
                .provider_context
                .recorded()
                .cloned()
                .filter(|saved| provider.restores_saved_context(saved))
        };
        let opening = catch_unwind(AssertUnwindSafe(|| {
            provider.open(ProviderOpenRequest::new(
                self.id.clone(),
                restore.clone(),
                control,
            ))
        }));
        let mut opening = match opening {
            Ok(opening) => opening,
            Err(payload) => {
                std::mem::forget(payload);
                self.attachment
                    .arm_unknown_open(self.storage_lease.clone())
                    .await;
                return Err(AttachmentOpenError::independent(
                    AgentError::CleanupUncertain,
                ));
            }
        };
        let result = poll_fn(|context| {
            match catch_unwind(AssertUnwindSafe(|| opening.as_mut().poll(context))) {
                Ok(Poll::Pending) => Poll::Pending,
                Ok(Poll::Ready(result)) => Poll::Ready(Some(result)),
                Err(payload) => {
                    std::mem::forget(payload);
                    Poll::Ready(None)
                }
            }
        })
        .await;
        let drop_panicked = match catch_unwind(AssertUnwindSafe(|| drop(opening))) {
            Ok(()) => false,
            Err(payload) => {
                std::mem::forget(payload);
                true
            }
        };
        if result.is_none() || drop_panicked {
            let drop_failure =
                AgentError::Protocol("provider opening future panicked while being dropped".into());
            let mut cause = AgentError::CleanupUncertain;
            match result {
                Some(Ok(opened)) => {
                    cause = drop_failure;
                    let events = Arc::new(Mutex::new(opened.events));
                    self.attachment
                        .arm(self.storage_lease.clone(), opened.session, events)
                        .await;
                }
                Some(Err(error)) => {
                    let (error, cleanup, _) = error.into_parts();
                    cause = AgentError::MultipleOperationFailures {
                        first_error: Box::new(error),
                        subsequent_error: Box::new(drop_failure),
                    };
                    match cleanup {
                        FailedOpenCleanup::Retained { report, owner } => {
                            self.attachment
                                .arm_failed_open(self.storage_lease.clone(), owner, report)
                                .await
                        }
                        FailedOpenCleanup::NotStarted => {
                            self.attachment
                                .arm_unknown_open(self.storage_lease.clone())
                                .await
                        }
                        FailedOpenCleanup::Completed(report) => {
                            self.attachment.reconcile_cleanup(report);
                        }
                    }
                }
                None => {
                    self.attachment
                        .arm_unknown_open(self.storage_lease.clone())
                        .await
                }
            }
            return Err(AttachmentOpenError::independent(cause));
        }
        let opened = match result.expect("completed provider open") {
            Ok(opened) => opened,
            Err(error) => {
                let (cause, cleanup, source) = error.into_parts();
                match cleanup {
                    FailedOpenCleanup::NotStarted => {}
                    FailedOpenCleanup::Completed(report) => {
                        self.attachment.reconcile_cleanup(report);
                    }
                    FailedOpenCleanup::Retained { report, owner } => {
                        self.attachment
                            .arm_failed_open(self.storage_lease.clone(), owner, report)
                            .await;
                    }
                }
                return Err(AttachmentOpenError::failed_open(cause, source));
            }
        };
        if restore.as_ref().is_some_and(|id| id != opened.session.id()) {
            self.attachment
                .arm(
                    self.storage_lease.clone(),
                    opened.session,
                    Arc::new(Mutex::new(opened.events)),
                )
                .await;
            let cleanup_result = self.attachment.cleanup().await;
            return Err(AttachmentOpenError::independent(
                AgentError::StorageInitialization {
                    error: StorageError::IdentityMismatch,
                    cleanup_result: Box::new(cleanup_result.into_result()),
                },
            ));
        }
        if &opened.session.capabilities() != provider.capabilities() {
            self.attachment
                .arm(
                    self.storage_lease.clone(),
                    opened.session,
                    Arc::new(Mutex::new(opened.events)),
                )
                .await;
            let cleanup_result = self.attachment.cleanup().await;
            return Err(AttachmentOpenError::independent(
                AgentError::StorageInitialization {
                    error: StorageError::IdentityMismatch,
                    cleanup_result: Box::new(cleanup_result.into_result()),
                },
            ));
        }
        let session = opened.session;
        let events = Arc::new(Mutex::new(opened.events));
        self.attachment
            .arm(self.storage_lease.clone(), session.clone(), events.clone())
            .await;
        // Provider open is deliberately outside the evidence lock. Publish its
        // context into the current snapshot so queue mutations acknowledged
        // while open was pending cannot be overwritten by the pre-open view.
        let save_result = {
            let mut evidence = self.evidence.lock().await;
            let mut changes = Vec::new();
            // The new fingerprint and the new session id are one save. Writing
            // the fingerprint first would make a crash resume the old session
            // under the new fingerprint.
            if self.open_fresh_context.load(Ordering::Relaxed) {
                let current = provider.identity();
                let saved = evidence.observed().expect("prepared session evidence");
                if saved.provider != current {
                    changes.push(SessionChange::ProviderIdentityChanged {
                        before: saved.provider.clone(),
                        after: current,
                    });
                }
            }
            let before = evidence
                .observed()
                .expect("prepared session evidence")
                .provider_context
                .clone();
            let after = ProviderContext::Recorded(session.id().clone());
            if before != after {
                changes.push(SessionChange::ProviderContext { before, after });
            }
            let recorded = if changes.is_empty() {
                Ok(())
            } else {
                evidence.record(changes)
            };
            match recorded {
                Err(error) => Err(error),
                Ok(()) if evidence.pending.is_empty() => Ok(()),
                Ok(()) => match evidence.generation() {
                    Ok(generation) => {
                        let result = catch_storage_operation(|| {
                            self.storage_lease
                                .save_changes(generation.clone(), evidence.pending.clone())
                        })
                        .await
                        .map_err(StorageError::bounded)
                        .and_then(|receipt| receipt.next_for(&generation, evidence.pending.len()));
                        result.map(|next| evidence.saved(next))
                    }
                    Err(error) => Err(error),
                },
            }
        };
        if let Err(error) = save_result {
            let cleanup_result = self.attachment.cleanup().await;
            return Err(AttachmentOpenError::independent(
                AgentError::StorageInitialization {
                    error,
                    cleanup_result: Box::new(cleanup_result.into_result()),
                },
            ));
        }
        // Cleared only after the save. A failed save retries this path
        // instead of resuming the saved session under the new fingerprint.
        self.open_fresh_context.store(false, Ordering::Relaxed);
        Ok(AttachedProvider { session, events })
    }
    pub(crate) fn attachment(&self) -> Arc<AttachmentLease> {
        self.attachment.clone()
    }
    pub(crate) fn protective_storage_lease(&self) -> Arc<dyn SessionStorageLease> {
        self.storage_lease.clone()
    }
    pub(crate) async fn begin(
        &self,
        request: ExecutionRequest,
        submitter: Submitter,
    ) -> Result<usize, AgentError> {
        self.begin_record(
            request,
            submitter,
            SubmissionMode::Immediate,
            Vec::new(),
            NoteIntake::default(),
            None,
        )
        .await
    }
    /// Saves input and its first scheduling event atomically before admission.
    /// A failed write is reconciled by reading storage. Uncertain persisted input
    /// remains observed so a later save cannot erase it or retry it blindly.
    pub(crate) async fn begin_with_scheduling(
        &self,
        request: ExecutionRequest,
        submitter: Submitter,
        event: InvocationSchedulingEvent,
        submission: SubmissionMode,
        notes: NoteIntake,
        resumed_from: Option<ExecutionId>,
    ) -> Result<usize, AgentError> {
        self.begin_record(
            request,
            submitter,
            submission,
            vec![event],
            notes,
            resumed_from,
        )
        .await
    }
    async fn begin_record(
        &self,
        request: ExecutionRequest,
        submitter: Submitter,
        submission: SubmissionMode,
        scheduling: Vec<InvocationSchedulingEvent>,
        notes: NoteIntake,
        resumed_from: Option<ExecutionId>,
    ) -> Result<usize, AgentError> {
        let Submitter {
            context: actor,
            initiation,
            ..
        } = submitter;
        // Notes reach the model only from the conversation's own records,
        // joined at dispatch: a caller cannot hand a turn notes of its own.
        if !request.user_message.notes().is_empty() {
            return Err(AgentError::InvalidInput(
                "a submitted message carries no notes".into(),
            ));
        }
        request.validate_message_size()?;
        InvocationSchedulingEvent::validate_history(&scheduling)
            .map_err(|error| AgentError::Storage(StorageError::Corrupt(error.to_string())))?;
        let storage_lease = self.storage_lease.clone();
        let session_id = self.id.clone();
        // Reserve this write before the caller can release its invocation guard.
        // Close can then join all supervised writes through the evidence mutex.
        let mut evidence = self.evidence.clone().lock_owned().await;
        // Read under the lock, as a note's acceptance is: a turn that waited
        // for it retires and takes notes as of its own admission.
        let now_ms = self.message_commit_clock.unix_milliseconds();
        tokio::spawn(async move {
            let snapshot = evidence.observed().expect("initialized agent session");
            if snapshot
                .invocations
                .iter()
                .any(|record| record.request.execution_id == request.execution_id)
            {
                return Err(AgentError::InvalidInput(
                    "execution ID already belongs to a saved invocation".into(),
                ));
            }
            if snapshot.invocations.len() >= SessionSnapshot::MAX_INVOCATIONS {
                return Err(AgentError::InvalidInput(
                    "session retained invocation limit reached".into(),
                ));
            }
            // Asked of the turns saved so far, under the lock that admits
            // the next: an app it names was drawn before it.
            app_sources::validate_against(&request.user_message, |execution| {
                snapshot
                    .invocations
                    .iter()
                    .find(|record| &record.request.execution_id == execution)
            })
            .map_err(AgentError::UnknownApp)?;
            // A steered message's offset is its target's saved event count.
            // A target not saved has none: a defensive refusal of an
            // invariant, since no `Agent` entry reaches it (a native steer
            // targets the running turn, saved at its own admission). The SDK
            // has no internal-error variant, so it is `InvalidInput`, saving
            // nothing rather than half a position
            // (`admission_saves_a_steering_target_with_its_offset_or_refuses_it`).
            let target_event_offset = scheduling
                .first()
                .and_then(|edge| edge.target.as_ref())
                .map(|target| {
                    SteeringPosition::at_admission(target, &snapshot.invocations)
                        .map(SteeringPosition::offset)
                        .ok_or_else(|| {
                            AgentError::InvalidInput(
                                "steering target is not a saved invocation".into(),
                            )
                        })
                })
                .transpose()?;
            let NoteSelection { retired, taken } =
                super::notes::select(snapshot, &notes.candidates, notes.takes_notes, now_ms);
            let record = InvocationRecord {
                decline: None,
                cancellation_delivery: None,
                resumed_from,
                target_event_offset,
                submission,
                request,
                actor,
                initiation,
                notes: taken,
                acknowledgement: SubmissionAcknowledgement::Pending,
                events: Vec::new(),
                scheduling,
                provider_report: None,
                local_cancellation: None,
                local_outcome: None,
                cancellation: None,
                result: None,
            };
            let execution_id = record.request.execution_id.clone();
            // Reserve identity before polling untrusted storage. Even a task panic
            // after committing the write must not erase admission or permit replay.
            let index = snapshot.invocations.len();
            let generation = evidence.generation().map_err(AgentError::Storage)?;
            let pending_before = evidence.pending.len();
            let recorded = retired
                .into_iter()
                .try_for_each(|record| evidence.record(vec![SessionChange::NoteRetired(record)]))
                .and_then(|()| {
                    evidence.record(vec![SessionChange::InputAccepted(Box::new(record))])
                });
            if let Err(error) = recorded {
                evidence.forget_after(pending_before);
                return Err(AgentError::Storage(error));
            }
            let save_result = storage_lease
                .save_changes(generation.clone(), evidence.pending.clone())
                .await
                .map_err(StorageError::bounded);
            let receipt = match save_result {
                Ok(receipt) => receipt,
                Err(error) => {
                    // save may have replaced the file before failing to acknowledge it.
                    // Only a successful read proving this identity absent permits retry.
                    let absent = match storage_lease
                        .load()
                        .await
                        .and_then(|loaded| loaded.into_published(&session_id))
                        .map_err(StorageError::bounded)
                    {
                        Ok((None, _)) => true,
                        Ok((Some(saved), _)) => {
                            let next = evidence.observed().expect("reserved admission");
                            let absent = saved.id == next.id
                                && saved.provider == next.provider
                                && saved.provider_context == next.provider_context
                                && super::validation::validate(&saved).is_ok()
                                && !saved
                                    .invocations
                                    .iter()
                                    .any(|record| record.request.execution_id == execution_id);
                            saved.discard_rejected_errors();
                            absent
                        }
                        Err(_) => false,
                    };
                    if absent {
                        evidence.forget_after(pending_before);
                    }
                    return Err(AgentError::Storage(error));
                }
            };
            let next_generation = receipt
                .next_for(&generation, evidence.pending.len())
                .map_err(AgentError::Storage)?;
            evidence.saved(next_generation);
            Ok(index)
        })
        .await
        .map_err(|_| AgentError::SubmissionUnresolved)?
    }
    /// Inspect one observed history under its evidence lock, including saves
    /// awaiting acknowledgement, without copying the invocation records.
    pub(crate) async fn read_invocations<T>(
        &self,
        inspect: impl FnOnce(&[InvocationRecord]) -> T,
    ) -> T {
        let evidence = self.evidence.lock().await;
        inspect(
            evidence
                .observed()
                .map_or(&[], |snapshot| &snapshot.invocations),
        )
    }
    /// Each observed invocation that `pick` keeps, read in saved order under
    /// the evidence lock with its index and all saved execution identities.
    pub(crate) async fn pick_invocations<T>(
        &self,
        mut pick: impl FnMut(usize, &InvocationRecord, &HashSet<&ExecutionId>) -> Option<T>,
    ) -> Vec<T> {
        let evidence = self.evidence.lock().await;
        let Some(snapshot) = evidence.observed() else {
            return Vec::new();
        };
        let saved: HashSet<&ExecutionId> = snapshot
            .invocations
            .iter()
            .map(|record| &record.request.execution_id)
            .collect();
        snapshot
            .invocations
            .iter()
            .enumerate()
            .filter_map(|(index, record)| pick(index, record, &saved))
            .collect()
    }
    pub(crate) async fn submission(&self, id: &ExecutionId) -> Option<InvocationRecord> {
        self.evidence
            .lock()
            .await
            .observed()?
            .invocations
            .iter()
            .find(|record| &record.request.execution_id == id)
            .cloned()
    }
    /// Whether current observed evidence names a provider context, including a
    /// publication write whose acknowledgement failed.
    pub(crate) async fn has_observed_provider_context(&self) -> bool {
        self.evidence
            .lock()
            .await
            .observed()
            .is_some_and(|snapshot| snapshot.provider_context.recorded().is_some())
    }
    /// Check the complete reviewed subject before acknowledging adapter evidence.
    /// Permission observations are saved before publication, so absent retained
    /// evidence cannot authorize an otherwise well-formed provider receipt.
    pub(crate) async fn validate_permission_evidence(
        &self,
        request: &PermissionRequest,
        input: &ToolReviewInput,
    ) -> Result<(), AgentError> {
        let evidence = self.evidence.lock().await;
        let review = evidence
            .observed()
            .and_then(|snapshot| {
                snapshot
                    .invocations
                    .iter()
                    .find(|record| &record.request.execution_id == request.execution_id())
            })
            .and_then(|record| {
                record.events.iter().find_map(|event| match event.update() {
                    ExecutionUpdate::PermissionRequested {
                        id,
                        tool_id,
                        options,
                        input,
                        ..
                    } if id == request.id() => Some((tool_id, options, input)),
                    _ => None,
                })
            });
        match review {
            Some((tool, options, original))
                if tool == request.tool_id()
                    && options == request.options()
                    && original == input =>
            {
                Ok(())
            }
            Some(_) => Err(AgentError::Protocol(
                "permission receipt contradicts the retained review".into(),
            )),
            None => Err(AgentError::Protocol(
                "permission receipt has no retained review".into(),
            )),
        }
    }

    /// Retain the local cancellation and its caller before settling the input.
    /// Failed writes keep observed evidence for the next persistence attempt.
    pub(crate) async fn record_cancellation(
        &self,
        index: usize,
        cancellation: InvocationCancellationEvent,
    ) -> Result<(), StorageError> {
        let mut evidence = self.evidence.lock().await;
        let record = evidence
            .observed()
            .and_then(|snapshot| snapshot.invocations.get(index))
            .ok_or_else(|| {
                StorageError::Corrupt("cancellation has no submitted invocation".into())
            })?;
        let mut history = super::validation::invocation_history(record)?;
        history
            .record_cancellation(cancellation.cancellation()?)
            .map_err(|error| StorageError::Corrupt(error.to_string()))?;
        let execution_id = record.request.execution_id.clone();
        evidence.record(vec![SessionChange::StopDecision {
            execution_id,
            event: cancellation,
        }])?;
        self.save_observed(&mut evidence).await
    }
    /// Check the bounded session-level reorder budget before changing live order.
    pub(crate) async fn check_queue_reorder_capacity(&self) -> Result<(), StorageError> {
        let evidence = self.evidence.lock().await;
        if evidence.observed().is_some_and(|snapshot| {
            snapshot
                .queue_history
                .iter()
                .filter(|entry| matches!(entry.mutation, QueueMutation::Reordered(_)))
                .count()
                >= QueueHistoryRecord::MAX_REORDERS
        }) {
            return Err(StorageError::Io("queue reorder history is full".into()));
        }
        Ok(())
    }
    /// `decision` as made just after a new scheduling edge of `edge` in the
    /// same unit: the edge counts toward its invocation's scheduling length.
    fn after_edge(mut decision: QueueHistoryRecord, edge: &ExecutionId) -> QueueHistoryRecord {
        if decision.mutation.id() == Some(edge) {
            decision.scheduling_length = decision.scheduling_length.map(|length| length + 1);
        }
        decision
    }
    /// The queue decision `mutation` makes, as observed. Applying it checks
    /// it against the queue.
    fn queue_decision(
        evidence: &Evidence,
        mutation: QueueMutation,
        actor: Option<ActionContext>,
    ) -> Result<QueueHistoryRecord, StorageError> {
        let snapshot = evidence
            .observed()
            .ok_or_else(|| StorageError::Corrupt("queue has no session".into()))?;
        let scheduling_length = mutation
            .id()
            .map(|id| {
                snapshot
                    .invocations
                    .iter()
                    .find(|record| &record.request.execution_id == id)
                    .map(|record| record.scheduling.len())
                    .ok_or_else(|| StorageError::Corrupt("queue mutation has no invocation".into()))
            })
            .transpose()?;
        Ok(QueueHistoryRecord {
            mutation,
            actor,
            scheduling_length,
        })
    }
    /// Record actual queue membership after the scheduler changed it. Failure
    /// retains observed evidence and must not roll back the live queue silently.
    pub(crate) async fn record_queue_mutation(
        &self,
        mutation: QueueMutation,
        actor: Option<ActionContext>,
    ) -> Result<(), StorageError> {
        let mut evidence = self.evidence.lock().await;
        let decision = Self::queue_decision(&evidence, mutation, actor)?;
        evidence.record(vec![SessionChange::QueueDecision(decision)])?;
        self.save_observed(&mut evidence).await
    }
    /// Persist dequeue before releasing the scheduler, independently of later dispatch.
    pub(crate) async fn record_queue_selection(&self, id: ExecutionId) -> Result<(), StorageError> {
        self.record_queue_mutation(QueueMutation::Selected { id }, None)
            .await
    }
    /// Persist actual admission, including native-steering fallback membership.
    pub(crate) async fn record_queue_admission(
        &self,
        id: ExecutionId,
        kind: InvocationKind,
        actor: ActionContext,
    ) -> Result<(), StorageError> {
        self.record_queue_mutation(QueueMutation::Admitted { id, kind }, Some(actor))
            .await
    }
    /// Retain one order decision and apply it to the live queue in a single
    /// transition. `apply` runs while this call holds the evidence mutex, so the
    /// retained history and the order the scheduler will dispatch cannot diverge.
    ///
    /// The only suspension point is acquiring that mutex, before either effect:
    /// a caller that bounds this call with a timeout or close therefore either
    /// retains and applies the change or leaves both unchanged. Writing the
    /// retained evidence to storage is the caller's separate, interruptible
    /// [`Self::flush_observed`].
    pub(crate) async fn retain_queue_reorder(
        &self,
        change: QueueOrderChange,
        actor: ActionContext,
        apply: impl FnOnce() -> Result<(), StorageError>,
    ) -> Result<(), StorageError> {
        let mut evidence = self.evidence.lock().await;
        // Either both effects happen or neither does, so a refused order change
        // leaves no retained mutation the live queue never made. Only this
        // transition rolls back: every other queue record describes an effect
        // the scheduler has already performed.
        let retained = evidence.pending.len();
        let decision =
            Self::queue_decision(&evidence, QueueMutation::Reordered(change), Some(actor))?;
        evidence.record(vec![SessionChange::QueueDecision(decision)])?;
        let result = apply();
        if result.is_err() {
            evidence.forget_after(retained);
        }
        result
    }
    /// Decide and record lease facts under the same lock every other fact of
    /// this conversation is recorded under, so two callers cannot both decide
    /// from the same lease (rows L5 and L6).
    ///
    /// `decide` is given the conversation's current lease as observed, which
    /// includes facts retained but not yet durable, and returns the records to
    /// add and a value for the caller. The records are folded through the
    /// lease rules ([`CurrentLease::apply`]) before anything is retained; a
    /// record they refuse retains nothing and is
    /// [`LeaseRecordError::Refused`]. Retained records are written at once, and
    /// a write that fails leaves them retained for the next save, as every
    /// other fact is: [`LeaseCommit::saved`] says which.
    ///
    /// Only a prepared manager records: before [`Self::prepare_for_lease_record`] or
    /// [`Agent::prepare`] has loaded the conversation this is
    /// [`LeaseRecordError::NotLoaded`].
    ///
    /// [`Agent::prepare`]: crate::application::agent_execution::agents::Agent::prepare
    pub async fn record_lease<T>(
        &self,
        decide: impl FnOnce(Option<&CurrentLease>) -> (Vec<LeaseRecord>, T),
    ) -> Result<LeaseCommit<T>, LeaseRecordError> {
        let mut evidence = self.evidence.lock().await;
        let snapshot = evidence.observed().ok_or(LeaseRecordError::NotLoaded)?;
        let (records, decided) = decide(snapshot.lease.as_ref());
        if records.is_empty() {
            return Ok(LeaseCommit {
                decided,
                saved: Ok(()),
            });
        }
        let mut lease = snapshot.lease.clone();
        for record in &records {
            if matches!(record, LeaseRecord::Unreadable { .. }) {
                return Err(LeaseRecordError::Unreadable);
            }
            lease = Some(
                CurrentLease::apply(lease.as_ref(), record).map_err(LeaseRecordError::Refused)?,
            );
        }
        evidence
            .record(records.into_iter().map(SessionChange::Lease).collect())
            .map_err(LeaseRecordError::Storage)?;
        let saved = self.save_observed(&mut evidence).await;
        Ok(LeaseCommit {
            decided,
            saved: saved.map_err(LeaseRecordError::Storage),
        })
    }
    /// How many more artifacts the conversation can record:
    /// [`ArtifactRecord::MAX_PER_CONVERSATION`] less those it records,
    /// including any retained but not yet saved. `None` before
    /// [`Agent::prepare`] has loaded the conversation.
    ///
    /// Read under the lock [`Self::record_artifact`] takes, but not held after
    /// it returns: room seen here can be taken by another record before this
    /// caller's, which [`Self::record_artifact`] then refuses as
    /// [`ArtifactRecordError::Full`].
    ///
    /// [`Agent::prepare`]: crate::application::agent_execution::agents::Agent::prepare
    pub async fn artifact_room(&self) -> Option<usize> {
        let evidence = self.evidence.lock().await;
        evidence.observed().map(|snapshot| {
            ArtifactRecord::MAX_PER_CONVERSATION.saturating_sub(snapshot.artifacts.len())
        })
    }
    /// Whether the conversation, as observed, accepted turn `turn`: the
    /// turns a record may name ([`ArtifactRecordError::UnknownTurn`]
    /// otherwise). `false` before it is loaded. Once accepted, a turn stays
    /// accepted.
    pub async fn accepts_turn(&self, turn: &ExecutionId) -> bool {
        let evidence = self.evidence.lock().await;
        evidence.observed().is_some_and(|snapshot| {
            snapshot
                .invocations
                .iter()
                .any(|invocation| &invocation.request.execution_id == turn)
        })
    }
    /// Whether the conversation has room to record `record`: room for one
    /// more, or the same record already recorded
    /// ([`ArtifactRecord::is_same_record`]), which [`Self::record_artifact`]
    /// answers without a new one. `None` as for [`Self::artifact_room`], and
    /// like it, only what was seen.
    pub async fn has_artifact_room(&self, record: &ArtifactRecord) -> Option<bool> {
        let evidence = self.evidence.lock().await;
        evidence.observed().map(|snapshot| {
            snapshot.artifacts.len() < ArtifactRecord::MAX_PER_CONVERSATION
                || snapshot
                    .artifacts
                    .iter()
                    .any(|recorded| recorded.is_same_record(record))
        })
    }
    /// Record that the conversation holds an artifact, under the same lock as
    /// [`Self::record_lease`], so a lease cannot end between the check and the
    /// record (row L5). The same record already recorded
    /// ([`ArtifactRecord::is_same_record`]: one person, one turn, whatever
    /// request) is answered `Ok` and recorded once; the same file asked for
    /// by another person, or in another turn, is recorded again, within
    /// [`ArtifactRecord::MAX_PER_CONVERSATION`].
    ///
    /// The record is checked against the conversation's current lease as
    /// observed, including facts retained but not yet durable: `record.lease`
    /// must be the latest lease, still live, and `record.actor` the one who
    /// asked for it; `record.turn`, when given, a turn the conversation
    /// accepted. The same rule reads the record back. A record refused
    /// retains nothing. A retained record is written at once, and a write
    /// that fails leaves it retained for the next save, as a lease record is:
    /// [`ArtifactRecordError::Storage`].
    ///
    /// # Errors
    /// [`ArtifactRecordError::NotLoaded`] before [`Self::prepare_for_lease_record`] or [`Agent::prepare`] has loaded
    /// the conversation; [`ArtifactRecordError::Full`],
    /// [`ArtifactRecordError::NotThisLease`] or
    /// [`ArtifactRecordError::UnknownTurn`] when refused;
    /// [`ArtifactRecordError::Storage`] when retained but not saved.
    ///
    /// [`Agent::prepare`]: crate::application::agent_execution::agents::Agent::prepare
    pub async fn record_artifact(&self, record: ArtifactRecord) -> Result<(), ArtifactRecordError> {
        let mut evidence = self.evidence.lock().await;
        let snapshot = evidence.observed().ok_or(ArtifactRecordError::NotLoaded)?;
        let accepted = |turn: &ExecutionId| {
            snapshot
                .invocations
                .iter()
                .find(|invocation| &invocation.request.execution_id == turn)
                .map(|invocation| invocation.actor.clone())
        };
        // The same record again, by the same person in the same turn under
        // any request, is already recorded: nothing is added, so one turn
        // publishing one file over and over never fills the conversation,
        // and a full one still answers it.
        let already = snapshot
            .artifacts
            .iter()
            .any(|recorded| recorded.is_same_record(&record));
        match record.admit(snapshot, accepted) {
            Ok(()) if already => return Ok(()),
            Err(ArtifactRefusal::Full) if already => return Ok(()),
            Ok(()) => {}
            Err(refusal) => {
                return Err(match refusal {
                    ArtifactRefusal::Full => ArtifactRecordError::Full,
                    ArtifactRefusal::NotThisLease | ArtifactRefusal::LeaseUnreadable => {
                        ArtifactRecordError::NotThisLease
                    }
                    ArtifactRefusal::UnknownTurn => ArtifactRecordError::UnknownTurn,
                })
            }
        }
        evidence
            .record(vec![SessionChange::Artifact(record)])
            .map_err(ArtifactRecordError::Storage)?;
        self.save_observed(&mut evidence)
            .await
            .map_err(ArtifactRecordError::Storage)
    }
    /// Leave a note in the conversation, under the same lock every other fact
    /// of it is recorded under. A note starts no turn and steers none.
    ///
    /// The same request again (the same principal and request, with the same
    /// text, delivery, surface and authority) is answered with the note it
    /// left, unchanged, once that note is saved: a retry after a lost answer
    /// finds the first acceptance, never a second one. Another note under the
    /// same principal's request is [`NoteRecordError::Conflict`]. A new note
    /// takes a fresh id and the injected clock's wall time, and must fit the
    /// conversation's bound and its author's rate ([`NoteRecord`]).
    ///
    /// A retained note is written at once, and a write that fails leaves it
    /// retained for the next save: [`NoteRecordError::Storage`].
    ///
    /// # Errors
    /// [`NoteRecordError::NotLoaded`] before [`Self::prepare_for_lease_record`] or [`Agent::prepare`] has loaded
    /// the conversation; [`NoteRecordError::Conflict`],
    /// [`NoteRecordError::Full`] or [`NoteRecordError::RateLimited`] when
    /// refused; [`NoteRecordError::Storage`] when retained but not saved.
    ///
    /// [`Agent::prepare`]: crate::application::agent_execution::agents::Agent::prepare
    pub async fn record_note(&self, request: NoteRequest) -> Result<NoteRecord, NoteRecordError> {
        let mut evidence = self.evidence.lock().await;
        // Read under the lock: a note that waited for it is accepted when it
        // is admitted, never dated before, which would let it fall out of the
        // rate window at once.
        let now = self.message_commit_clock.unix_milliseconds();
        let snapshot = evidence.observed().ok_or(NoteRecordError::NotLoaded)?;
        let record = match super::notes::admit(snapshot, request, now) {
            Ok(NoteAdmission::Recorded(record)) => record,
            Ok(NoteAdmission::New(record)) => {
                evidence
                    .record(vec![SessionChange::Note(record.clone())])
                    .map_err(NoteRecordError::Storage)?;
                record
            }
            Err(NoteRefusal::Conflict) => return Err(NoteRecordError::Conflict),
            Err(NoteRefusal::Full) => return Err(NoteRecordError::Full),
            Err(NoteRefusal::RateLimited) => return Err(NoteRecordError::RateLimited),
        };
        // A note already retained whose write failed is written now, so the
        // answer always names a saved note.
        self.save_observed(&mut evidence)
            .await
            .map_err(NoteRecordError::Storage)?;
        Ok(record)
    }
    /// Write the lease records, and any other evidence, that an earlier
    /// [`Self::record_lease`] retained but could not save
    /// ([`LeaseCommit::saved`]). Nothing retained is nothing to write. A
    /// caller that will not close the Agent through an attachment uses this so
    /// a retained record is not left unsaved.
    pub async fn save_retained_lease_records(&self) -> Result<(), StorageError> {
        self.flush_observed().await
    }
    /// Write observed evidence retained by an earlier transition. An unchanged
    /// retry flushes evidence an earlier failed write left observed.
    pub(crate) async fn flush_observed(&self) -> Result<(), StorageError> {
        let mut evidence = self.evidence.lock().await;
        self.save_observed(&mut evidence).await
    }
    /// Retain the acknowledgement fact separately from delivery and settlement.
    /// The caller flushes it under panic supervision so a failed adapter cannot
    /// erase the live receipt's exact audit/storage outcome.
    pub(crate) async fn retain_submission_acknowledgement(
        &self,
        index: usize,
        mut acknowledgement: SubmissionAcknowledgement,
    ) -> Result<(), StorageError> {
        if let SubmissionAcknowledgement::Failed { audit, storage } = &mut acknowledgement {
            *audit = audit.take().map(AgentError::bounded);
            *storage = storage.take().map(StorageError::bounded);
        }
        super::validation::validate_submission_acknowledgement(&acknowledgement)?;
        let mut evidence = self.evidence.lock().await;
        let record = evidence
            .observed()
            .ok_or_else(|| StorageError::Corrupt("acknowledgement has no session".into()))?
            .invocations
            .get(index)
            .ok_or_else(|| {
                StorageError::Corrupt("acknowledgement has no submitted invocation".into())
            })?;
        let before = record.acknowledgement.clone();
        if before != acknowledgement {
            let execution_id = record.request.execution_id.clone();
            evidence.record(vec![SessionChange::ReceiptUpdated {
                execution_id,
                before,
                after: acknowledgement,
            }])?;
        }
        Ok(())
    }
    /// Appends scheduling evidence to an existing input and saves it.
    /// Failed writes retain observed evidence for the next persistence attempt,
    /// while the committed snapshot remains unchanged.
    pub(crate) async fn record_scheduling(
        &self,
        index: usize,
        event: InvocationSchedulingEvent,
    ) -> Result<(), StorageError> {
        self.record_scheduling_with_queue(index, event, None).await
    }
    /// Return the locally applied decision even if its save is not acknowledged.
    /// Validation failure returns no commit and grants no decision authority.
    /// Flush only when a decline receipt itself still awaits storage ACK.
    /// Other pending facts keep their existing close/lease persistence owner.
    /// Actual SDK close producer only: record its non-decline ACK under the
    /// minted lifetime and exact current lease, then join the real writer ACK.
    pub(crate) async fn acknowledge_sdk_cleanup(
        &self,
        scope: &OwnedCleanupScope,
    ) -> Result<(), StorageError> {
        if scope.session() != &self.id {
            return Err(StorageError::Corrupt(
                "SDK close scope names another session".into(),
            ));
        }
        let mut evidence = self.evidence.lock().await;
        let current = evidence
            .observed()
            .and_then(|snapshot| snapshot.lease.as_ref());
        let Some(current) = current else {
            return Ok(());
        };
        let Some(held) = current.held() else {
            return Err(StorageError::Corrupt(
                "SDK close has no readable issued lease".into(),
            ));
        };
        if !current.sdk_cleanup_acknowledged(scope) {
            let record = LeaseRecord::SdkCleanupAcknowledged {
                lease: held.id().clone(),
                lifetime: scope.lifetime().clone(),
            };
            evidence.record(vec![SessionChange::Lease(record)])?;
        }
        self.save_observed(&mut evidence).await
    }

    pub(crate) async fn flush_cancellation_receipts(&self) -> Result<(), StorageError> {
        let mut evidence = self.evidence.lock().await;
        let pending = evidence
            .pending
            .iter()
            .flat_map(SessionSaveUnit::changes)
            .any(|change| {
                matches!(
                    change,
                    SessionChange::CancellationPrepared { .. }
                        | SessionChange::CancellationAcknowledged { .. }
                )
            });
        if pending {
            self.save_observed(&mut evidence).await
        } else {
            Ok(())
        }
    }

    pub(crate) async fn flush_decline_receipts(&self) -> Result<(), StorageError> {
        let mut evidence = self.evidence.lock().await;
        let pending = evidence
            .pending
            .iter()
            .flat_map(SessionSaveUnit::changes)
            .any(|change| {
                matches!(
                    change,
                    SessionChange::DeclinePrepared { .. }
                        | SessionChange::DeclineAcknowledged { .. }
                )
            });
        if pending {
            self.save_observed(&mut evidence).await
        } else {
            Ok(())
        }
    }

    pub(crate) async fn prepare_cancellation(
        &self,
        index: usize,
        event: InvocationSchedulingEvent,
    ) -> Result<super::CancellationDecision, StorageError> {
        let mut evidence = self.evidence.lock().await;
        let record = evidence
            .observed()
            .and_then(|snapshot| snapshot.invocations.get(index))
            .ok_or_else(|| StorageError::Corrupt("cancellation has no invocation".into()))?;
        if record.cancellation_delivery.is_none() {
            let execution_id = record.request.execution_id.clone();
            let decision = super::CancellationDecision::new(event, false);
            evidence.record(vec![SessionChange::CancellationPrepared {
                execution_id,
                decision,
            }])?;
        }
        // The preparation, unlike the terminal edge, must be acknowledged
        // before its independently durable audit can be attempted.
        self.save_observed(&mut evidence).await?;
        Ok(evidence.observed().unwrap().invocations[index]
            .cancellation_delivery
            .clone()
            .unwrap())
    }

    pub(crate) async fn acknowledge_cancellation(
        &self,
        index: usize,
        save: bool,
    ) -> Result<(), StorageError> {
        let mut evidence = self.evidence.lock().await;
        let record = evidence
            .observed()
            .and_then(|snapshot| snapshot.invocations.get(index))
            .ok_or_else(|| {
                StorageError::Corrupt("cancellation acknowledgement has no invocation".into())
            })?;
        let decision = record.cancellation_delivery.as_ref().ok_or_else(|| {
            StorageError::Corrupt("cancellation acknowledgement has no preparation".into())
        })?;
        if !decision.acknowledged() {
            let execution_id = record.request.execution_id.clone();
            evidence.record(vec![SessionChange::CancellationAcknowledged {
                execution_id,
            }])?;
        }
        if save {
            self.save_observed(&mut evidence).await
        } else {
            Ok(())
        }
    }
    pub(crate) async fn prepare_decline(
        &self,
        index: usize,
        event: InvocationSchedulingEvent,
    ) -> Result<super::DeclineDecision, StorageError> {
        let mut evidence = self.evidence.lock().await;
        let record = evidence
            .observed()
            .and_then(|snapshot| snapshot.invocations.get(index))
            .ok_or_else(|| StorageError::Corrupt("decline has no invocation".into()))?;
        if record.decline.is_none() {
            let execution_id = record.request.execution_id.clone();
            let decision = super::DeclineDecision::new(event, false);
            evidence.record(vec![SessionChange::DeclinePrepared {
                execution_id,
                decision,
            }])?;
        }
        // The preparation, unlike the terminal edge, must be acknowledged
        // before its independently durable audit can be attempted.
        self.save_observed(&mut evidence).await?;
        Ok(evidence.observed().unwrap().invocations[index]
            .decline
            .clone()
            .unwrap())
    }

    pub(crate) async fn apply_decline(
        &self,
        index: usize,
    ) -> Result<SchedulingCommit, StorageError> {
        let mut evidence = self.evidence.lock().await;
        let record = evidence
            .observed()
            .and_then(|snapshot| snapshot.invocations.get(index))
            .ok_or_else(|| StorageError::Corrupt("decline has no invocation".into()))?;
        let decision = record
            .decline
            .as_ref()
            .ok_or_else(|| StorageError::Corrupt("decline has no preparation".into()))?;
        if record.scheduling.last() != Some(decision.event()) {
            let change = SessionChange::SchedulingTransition {
                execution_id: record.request.execution_id.clone(),
                event: decision.event().clone(),
            };
            evidence.record(vec![change])?;
        }
        Ok(SchedulingCommit {
            saved: self.save_observed(&mut evidence).await,
        })
    }

    pub(crate) async fn acknowledge_decline(
        &self,
        index: usize,
        save: bool,
    ) -> Result<(), StorageError> {
        let mut evidence = self.evidence.lock().await;
        let record = evidence
            .observed()
            .and_then(|snapshot| snapshot.invocations.get(index))
            .ok_or_else(|| {
                StorageError::Corrupt("decline acknowledgement has no invocation".into())
            })?;
        let decision = record.decline.as_ref().ok_or_else(|| {
            StorageError::Corrupt("decline acknowledgement has no preparation".into())
        })?;
        if !decision.acknowledged() {
            let execution_id = record.request.execution_id.clone();
            evidence.record(vec![SessionChange::DeclineAcknowledged { execution_id }])?;
        }
        if save {
            self.save_observed(&mut evidence).await
        } else {
            Ok(())
        }
    }

    pub(crate) async fn record_scheduling_commit(
        &self,
        index: usize,
        event: InvocationSchedulingEvent,
    ) -> Result<SchedulingCommit, StorageError> {
        self.record_scheduling_with_queue_commit(index, event, None)
            .await
    }
    /// Persist a lifecycle edge and its corresponding actual queue removal together.
    pub(crate) async fn record_scheduling_with_queue(
        &self,
        index: usize,
        event: InvocationSchedulingEvent,
        mutation: Option<QueueMutation>,
    ) -> Result<(), StorageError> {
        self.record_scheduling_with_queue_commit(index, event, mutation)
            .await?
            .saved
    }
    async fn record_scheduling_with_queue_commit(
        &self,
        index: usize,
        event: InvocationSchedulingEvent,
        mutation: Option<QueueMutation>,
    ) -> Result<SchedulingCommit, StorageError> {
        let mut evidence = self.evidence.lock().await;
        let snapshot = evidence
            .observed()
            .ok_or_else(|| StorageError::Corrupt("scheduling event has no session".into()))?;
        let record = snapshot.invocations.get(index).ok_or_else(|| {
            StorageError::Corrupt("scheduling event has no submitted invocation".into())
        })?;
        let mut history = super::validation::invocation_history(record)?;
        history
            .schedule(
                event
                    .transition()
                    .map_err(|error| StorageError::Corrupt(error.to_string()))?,
            )
            .and_then(|()| history.validate_checkpoint())
            .map_err(|error| StorageError::Corrupt(error.to_string()))?;
        super::validation::validate_stop_actor(record.local_cancellation.as_ref(), Some(&event))?;
        let actor = event.actor.clone();
        let execution_id = record.request.execution_id.clone();
        let mut changes = vec![SessionChange::SchedulingTransition {
            execution_id: execution_id.clone(),
            event,
        }];
        if let Some(mutation) = mutation {
            let decision = Self::queue_decision(&evidence, mutation, actor)?;
            changes.push(SessionChange::QueueDecision(Self::after_edge(
                decision,
                &execution_id,
            )));
        }
        evidence.record(changes)?;
        Ok(SchedulingCommit {
            saved: self.save_observed(&mut evidence).await,
        })
    }
    /// Atomically retain a failed queued receipt, its terminal scheduling edge,
    /// and actual queue removal. The completed result is present before the
    /// domain validates the terminal scheduling checkpoint.
    pub(crate) async fn settle_failed_queue(
        &self,
        index: usize,
        event: InvocationSchedulingEvent,
        mutation: QueueMutation,
        result: Result<ExecutionOutcome, AgentError>,
    ) -> Result<(), StorageError> {
        let result = result.map_err(AgentError::bounded);
        let mut evidence = self.evidence.lock().await;
        let record = evidence
            .observed()
            .ok_or_else(|| StorageError::Corrupt("queue settlement has no session".into()))?
            .invocations
            .get(index)
            .ok_or_else(|| {
                StorageError::Corrupt("queue settlement has no submitted invocation".into())
            })?;
        let execution_id = record.request.execution_id.clone();
        let before = record.result.clone();
        let local_outcome = result.as_ref().ok().copied();
        let decision = Self::after_edge(
            Self::queue_decision(&evidence, mutation, None)?,
            &execution_id,
        );
        evidence.record(vec![
            SessionChange::LocalSettlement {
                execution_id: execution_id.clone(),
                before,
                after: result,
                local_outcome,
            },
            SessionChange::SchedulingTransition {
                execution_id,
                event,
            },
            SessionChange::QueueDecision(decision),
        ])?;
        self.save_observed(&mut evidence).await
    }
    /// Scope output authority to the owning observation loop, including unwind.
    /// This does not authorize events until begin_dispatch actually runs.
    pub(crate) fn observe_invocation<'a>(
        &'a self,
        id: &'a ExecutionId,
    ) -> InvocationObservations<'a> {
        InvocationObservations { manager: self, id }
    }

    /// Borrow one invocation's retained events while `body` runs.
    ///
    /// The evidence mutex is held for the call. `body` copies what it needs and
    /// returns before any hook callback. This method does not itself copy payloads.
    pub(crate) async fn with_invocation_events<R>(
        &self,
        index: usize,
        body: impl FnOnce(&[ExecutionEvent]) -> R + Send,
    ) -> Result<R, StorageError> {
        self.with_invocation(index, |record| body(&record.events))
            .await
    }

    /// Borrow one saved invocation record while `body` runs, under the same
    /// lock and rules as [`Self::with_invocation_events`].
    pub(crate) async fn with_invocation<R>(
        &self,
        index: usize,
        body: impl FnOnce(&InvocationRecord) -> R + Send,
    ) -> Result<R, StorageError> {
        let evidence = self.evidence.lock().await;
        let record = evidence
            .observed()
            .ok_or_else(|| StorageError::Corrupt("invocation observations have no session".into()))?
            .invocations
            .get(index)
            .ok_or_else(|| {
                StorageError::Corrupt("invocation observations have no submitted invocation".into())
            })?;
        Ok(body(record))
    }
    /// The notes the saved turn at `index` took, as the model is given them,
    /// in the order it took them. A taken note the conversation does not hold
    /// is a corrupt record: the turn fails rather than run without it.
    pub(crate) async fn taken_notes(&self, index: usize) -> Result<Vec<NoteContext>, StorageError> {
        let evidence = self.evidence.lock().await;
        let snapshot = evidence
            .observed()
            .ok_or_else(|| StorageError::Corrupt("a turn's notes have no session".into()))?;
        let record = snapshot
            .invocations
            .get(index)
            .ok_or_else(|| StorageError::Corrupt("a turn's notes have no saved turn".into()))?;
        record
            .notes
            .iter()
            .map(|id| {
                snapshot
                    .notes
                    .iter()
                    .find(|note| &note.id == id)
                    .ok_or_else(|| StorageError::Corrupt("a turn takes a note not saved".into()))?
                    .context()
            })
            .collect()
    }
    pub(crate) fn retire_observations(&self, id: &ExecutionId) {
        if let Some(authority) = self
            .dispatched
            .write()
            .expect("dispatch ownership")
            .get_mut(id)
        {
            *authority = ObservationAuthority::CancellationOnly;
        }
    }
    /// Register the execution immediately before polling its provider attempt.
    pub(crate) fn begin_dispatch(&self, id: &ExecutionId) {
        self.dispatched
            .write()
            .expect("dispatch ownership")
            .insert(id.clone(), ObservationAuthority::Output);
    }
    /// An explicit admission rejection cannot authorize later provider output.
    pub(crate) fn reject_dispatch(&self, id: &ExecutionId) {
        self.dispatched
            .write()
            .expect("dispatch ownership")
            .remove(id);
    }
    pub(crate) fn validate_observation_owner(
        &self,
        event: &ExecutionEvent,
    ) -> Result<(), AgentError> {
        match self
            .dispatched
            .read()
            .expect("dispatch ownership")
            .get(event.execution_id())
        {
            Some(ObservationAuthority::Output) => Ok(()),
            Some(ObservationAuthority::CancellationOnly)
                if matches!(event.update(), ExecutionUpdate::PermissionCancelled(_)) =>
            {
                Ok(())
            }
            Some(ObservationAuthority::CancellationOnly) => Err(AgentError::Protocol(
                "provider output follows its invocation observation boundary".into(),
            )),
            None => Err(AgentError::Protocol(
                "provider observation targets an undispatched invocation".into(),
            )),
        }
    }
    // Check before the Agent clones provider output. The cache is private derived
    // accounting, never persisted authority; imported records rebuild it lazily.
    pub(crate) async fn validate_event_retention(
        &self,
        event: &ExecutionEvent,
    ) -> Result<(), AgentError> {
        let mut evidence = self.evidence.lock().await;
        Self::usage_after(&mut evidence, event).map(|_| ())
    }
    fn usage_after(
        evidence: &mut Evidence,
        event: &ExecutionEvent,
    ) -> Result<ObservationUsage, AgentError> {
        let usage = if let Some(usage) = evidence.event_usage.get(event.execution_id()) {
            *usage
        } else {
            let record = evidence
                .observed()
                .and_then(|snapshot| {
                    snapshot
                        .invocations
                        .iter()
                        .find(|record| &record.request.execution_id == event.execution_id())
                })
                .ok_or_else(|| {
                    AgentError::Protocol("provider observation has no submitted invocation".into())
                })?;
            let usage = ObservationUsage::from_events(&record.events)?;
            evidence
                .event_usage
                .insert(event.execution_id().clone(), usage);
            usage
        };
        usage.with_event(event)
    }
    pub(crate) async fn event(&self, event: ExecutionEvent) -> Result<(), StorageError> {
        let save = !matches!(event.update(), ExecutionUpdate::Message(_));
        let message_bytes = match event.update() {
            ExecutionUpdate::Message(chunk) => chunk.payload_bytes(),
            _ => 0,
        };
        let message_retained_bytes = if save { 0 } else { event.retained_bytes() };
        event
            .validate_payload_size()
            .map_err(|error| StorageError::Corrupt(error.to_string()))?;
        self.validate_observation_owner(&event)
            .map_err(|error| StorageError::Corrupt(error.to_string()))?;
        let mut evidence = self.evidence.lock().await;
        let usage = Self::usage_after(&mut evidence, &event)
            .map_err(|error| StorageError::Corrupt(error.to_string()))?;
        // Output from an abandoned caller wait still belongs to its original
        // input; applying it checks the invocation's whole history.
        let id = event.execution_id().clone();
        evidence.record(vec![SessionChange::ProviderObservation(event)])?;
        evidence.event_usage.insert(id, usage);
        // Text/thought fragments are live output until a bounded output flush
        // or the next boundary save. Tools, reviews, terminal events and
        // settlement persist the accumulated observations. A process failure
        // may lose only the unfinished streaming suffix.
        if save {
            return self.save_observed(&mut evidence).await;
        }
        let generation = evidence.generation()?;
        let pending = evidence
            .message_commit
            .get_or_insert_with(|| PendingMessageCommit {
                generation,
                deadline: self
                    .message_commit_clock
                    .now()
                    .saturating_add(MESSAGE_COMMIT_DELAY),
                bytes: 0,
                retained_bytes: 0,
                count: 0,
            });
        pending.bytes = pending.bytes.saturating_add(message_bytes);
        pending.retained_bytes = pending
            .retained_bytes
            .saturating_add(message_retained_bytes);
        pending.count = pending.count.saturating_add(1);
        // The smaller cadence is the normal flush. The larger retained
        // safety bound remains a backstop for future policy adjustments.
        if pending.bytes >= MESSAGE_COMMIT_BYTES
            || pending.count >= MESSAGE_COMMIT_COUNT
            || pending.retained_bytes >= 8 * 1024 * 1024
            || pending.count >= 1024
        {
            self.save_observed(&mut evidence).await?;
        }
        Ok(())
    }
    /// Retain the adapter's independent facts before local hooks and receipt writes.
    /// Failed persistence leaves the report observed for the next save.
    pub(crate) async fn record_provider_report(
        &self,
        index: usize,
        settlement: ExecutionReport,
        cancellation: Option<InvocationCancellationEvent>,
    ) -> Result<(), StorageError> {
        let mut evidence = self.evidence.lock().await;
        let record = &evidence
            .observed()
            .expect("initialized agent session")
            .invocations[index];
        if let Some(result) = &record.result {
            super::validation::validate_report_against_local_result(&settlement, result)?;
        }
        let mut history = super::validation::invocation_history(record)?;
        super::validation::record_report(
            &mut history,
            Some(&settlement),
            cancellation.as_ref(),
            record.scheduling.last(),
        )?;
        let execution_id = record.request.execution_id.clone();
        if let Some(prior) = &record.provider_report {
            if prior != &settlement || record.local_cancellation != cancellation {
                return Err(StorageError::Corrupt(
                    "provider settlement cannot be replaced".into(),
                ));
            }
        } else {
            evidence.record(vec![SessionChange::ProviderReport {
                execution_id,
                report: settlement,
                local_stop: cancellation,
            }])?;
        }
        self.save_observed(&mut evidence).await
    }
    /// Cancellation is an explicit fact, never inferred from nested diagnostics.
    pub(crate) async fn invocation_cancelled(&self, index: usize) -> bool {
        let evidence = self.evidence.lock().await;
        let record = &evidence
            .observed()
            .expect("initialized agent session")
            .invocations[index];
        record.provider_report.as_ref().is_some_and(|settlement| {
            settlement.source() == ExecutionReportSource::LocalCancellation
                || settlement.source() == ExecutionReportSource::PolicyStop
                || settlement.provider_result() == Some(&Ok(ExecutionOutcome::Cancelled))
        }) || record.local_outcome == Some(ExecutionOutcome::Cancelled)
            || record.result == Some(Ok(ExecutionOutcome::Cancelled))
    }
    pub(crate) async fn finish(
        &self,
        index: usize,
        result: Result<ExecutionOutcome, AgentError>,
    ) -> Result<(), StorageError> {
        let result = result.map_err(AgentError::bounded);
        let mut evidence = self.evidence.lock().await;
        let local_outcome = Self::validate_result(&evidence, index, &result)?;
        let record = &evidence
            .observed()
            .expect("initialized agent session")
            .invocations[index];
        let before = record.result.clone();
        let prior_outcome = record.local_outcome;
        let execution_id = record.request.execution_id.clone();
        if before.as_ref() != Some(&result) || prior_outcome != local_outcome {
            evidence.record(vec![SessionChange::LocalSettlement {
                execution_id,
                before,
                after: result,
                local_outcome,
            }])?;
        }
        self.save_observed(&mut evidence).await
    }
    /// Reads the last scheduling edge by its stable admitted record index.
    pub(crate) async fn scheduling_event(&self, index: usize) -> Option<InvocationSchedulingEvent> {
        self.evidence
            .lock()
            .await
            .observed()?
            .invocations
            .get(index)?
            .scheduling
            .last()
            .cloned()
    }

    /// Who started the saved turn `id`, as its record says; `None` when no
    /// turn of this session is `id`.
    pub(crate) async fn initiation_of(&self, id: &ExecutionId) -> Option<Initiation> {
        let evidence = self.evidence.lock().await;
        evidence
            .observed()?
            .invocations
            .iter()
            .find(|record| &record.request.execution_id == id)
            .map(|record| record.initiation.clone())
    }

    /// Narrow observed scheduling lookup for supervisor recovery.
    pub(crate) async fn scheduling_metadata(
        &self,
        id: &ExecutionId,
    ) -> Option<(usize, InvocationSchedulingEvent)> {
        let evidence = self.evidence.lock().await;
        let (index, record) = evidence
            .observed()?
            .invocations
            .iter()
            .enumerate()
            .find(|(_, record)| &record.request.execution_id == id)?;
        Some((index, record.scheduling.last()?.clone()))
    }

    /// Ask the domain which final cause agrees with retained provider and local facts.
    /// A missing outcome and failure means the invocation is not ready to settle.
    pub(crate) async fn scheduling_settlement_cause(
        &self,
        index: usize,
    ) -> Result<SchedulingCause, StorageError> {
        let evidence = self.evidence.lock().await;
        let record = evidence
            .observed()
            .and_then(|snapshot| snapshot.invocations.get(index))
            .ok_or_else(|| {
                StorageError::Corrupt("scheduling settlement has no invocation".into())
            })?;
        super::validation::invocation_history(record)?
            .settlement_cause()
            .ok_or_else(|| {
                StorageError::Corrupt(
                    "scheduling settlement has no outcome or local failure".into(),
                )
            })
    }

    /// Preserve a supervisor failure without replacing an already-known outcome.
    /// A failure before admission has no record and is returned without a save.
    pub(crate) async fn retain_invocation_failure(
        &self,
        id: &ExecutionId,
        error: AgentError,
    ) -> Result<ExecutionOutcome, AgentError> {
        let error = error.bounded();
        let mut evidence = self.evidence.lock().await;
        let Some(index) = evidence.observed().and_then(|snapshot| {
            snapshot
                .invocations
                .iter()
                .position(|record| &record.request.execution_id == id)
        }) else {
            return Err(error);
        };
        let prior = evidence.observed().expect("matched invocation").invocations[index]
            .result
            .clone()
            .or_else(|| {
                evidence.observed().expect("matched invocation").invocations[index]
                    .provider_report
                    .as_ref()
                    .map(|settlement| settlement.clone().into_result())
            });
        let result = Err(match prior {
            Some(prior) => AgentError::ExecutionObservation {
                error: Box::new(error),
                execution_result: Some(Box::new(prior)),
            }
            .bounded(),
            None => error,
        });
        self.retain_result(&mut evidence, index, result).await.0
    }

    /// Retains late receipt failures so later saves cannot erase an observed error.
    /// If this final save also fails, its wrapper remains observed for a subsequent
    /// save. A process loss before successful persistence can only recover the last
    /// acknowledged evidence, never promise an exactly durable error response.
    pub(crate) async fn settle_submission(
        &self,
        index: usize,
        result: Result<ExecutionOutcome, AgentError>,
    ) -> Result<ExecutionOutcome, AgentError> {
        let result = result.map_err(AgentError::bounded);
        let mut evidence = self.evidence.lock().await;
        self.retain_result(&mut evidence, index, result).await.0
    }
    pub(crate) async fn settle_submission_with_storage_ack(
        &self,
        index: usize,
        result: Result<ExecutionOutcome, AgentError>,
    ) -> (Result<ExecutionOutcome, AgentError>, Option<StorageError>) {
        // The receipt may already contain an earlier storage failure. Return this
        // save's acknowledgement separately so callers never infer a new effect
        // by inspecting or comparing diagnostic error values.
        let result = result.map_err(AgentError::bounded);
        let mut evidence = self.evidence.lock().await;
        self.retain_result(&mut evidence, index, result).await
    }
    /// Retain a receipt result already known after a storage future panicked.
    ///
    /// This updates observed evidence only. A later ordinary flush owns any
    /// durable write, so retaining the failed write does not claim it succeeded
    /// or recursively create another persistence failure.
    pub(crate) async fn retain_submission_result(
        &self,
        index: usize,
        result: Result<ExecutionOutcome, AgentError>,
    ) -> Result<(), StorageError> {
        let result = result.map_err(AgentError::bounded);
        let mut evidence = self.evidence.lock().await;
        Self::update_result(&mut evidence, index, &result).map(|_| ())
    }
    async fn retain_result(
        &self,
        evidence: &mut Evidence,
        index: usize,
        mut result: Result<ExecutionOutcome, AgentError>,
    ) -> (Result<ExecutionOutcome, AgentError>, Option<StorageError>) {
        let changed = match Self::update_result(evidence, index, &result) {
            Ok(changed) => changed,
            Err(error) => return (Err(AgentError::Storage(error.clone())), Some(error)),
        };
        if !changed {
            return (result, None);
        }
        let mut storage_failure = None;
        if let Err(error) = self.save_observed(evidence).await {
            storage_failure = Some(error.clone());
            let before = result.clone();
            result = Err(AgentError::StorageAfterExecution {
                error,
                execution_result: Box::new(result),
            }
            .bounded());
            let record = &evidence
                .observed()
                .expect("initialized agent session")
                .invocations[index];
            let execution_id = record.request.execution_id.clone();
            let local_outcome = record.local_outcome;
            // The wrapper only describes this failed write. If it does not
            // apply, the result before it stays retained for the next save,
            // and the caller still gets this write's failure and the result.
            let _ = evidence.record(vec![SessionChange::LocalSettlement {
                execution_id,
                before: Some(before),
                after: result.clone(),
                local_outcome,
            }]);
        }
        (result, storage_failure)
    }
    fn update_result(
        evidence: &mut Evidence,
        index: usize,
        result: &Result<ExecutionOutcome, AgentError>,
    ) -> Result<bool, StorageError> {
        let local_outcome = Self::validate_result(evidence, index, result)?;
        let record = &evidence
            .observed()
            .expect("initialized agent session")
            .invocations[index];
        if record.result.as_ref() == Some(result) {
            return Ok(false);
        }
        let before = record.result.clone();
        let execution_id = record.request.execution_id.clone();
        evidence.record(vec![SessionChange::LocalSettlement {
            execution_id,
            before,
            after: result.clone(),
            local_outcome,
        }])?;
        Ok(true)
    }
    async fn save_observed(&self, evidence: &mut Evidence) -> Result<(), StorageError> {
        if evidence.pending.is_empty() {
            return Ok(());
        }
        let generation = evidence.generation()?;
        let receipt = self
            .storage_lease
            .save_changes(generation.clone(), evidence.pending.clone())
            .await
            .map_err(StorageError::bounded)?;
        let next_generation = receipt.next_for(&generation, evidence.pending.len())?;
        evidence.saved(next_generation);
        Ok(())
    }
    fn validate_result(
        evidence: &Evidence,
        index: usize,
        result: &Result<ExecutionOutcome, AgentError>,
    ) -> Result<Option<ExecutionOutcome>, StorageError> {
        let record = &evidence
            .observed()
            .expect("initialized agent session")
            .invocations[index];
        super::validation::local_result_transition(record, result)
    }
}

async fn catch_storage_operation<'a, T>(
    operation: impl FnOnce() -> StorageFuture<'a, T>,
) -> Result<T, StorageError>
where
    T: 'a,
{
    let operation = catch_unwind(AssertUnwindSafe(operation));
    let Ok(mut operation) = operation else {
        return Err(StorageError::Io(
            "session storage operation construction panicked".into(),
        ));
    };
    let result = poll_fn(|context| {
        match catch_unwind(AssertUnwindSafe(|| operation.as_mut().poll(context))) {
            Ok(poll) => poll.map(Some),
            Err(payload) => {
                std::mem::forget(payload);
                Poll::Ready(None)
            }
        }
    })
    .await;
    let dropped = catch_unwind(AssertUnwindSafe(|| drop(operation))).is_ok();
    match (result, dropped) {
        (Some(result), true) => result,
        _ => Err(StorageError::Io(
            "session storage operation panicked".into(),
        )),
    }
}

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/sessions/manager.rs"]
mod tests;
