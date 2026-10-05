use super::{
    app_sources, attachment::AttachmentLease, steering_position::SteeringPosition,
    InvocationCancellationEvent, InvocationRecord, InvocationSchedulingEvent, MessageCommitClock,
    ProviderContext, QueueHistoryRecord, SessionChange, SessionLoadState, SessionSaveGeneration,
    SessionSaveUnit, SessionSnapshot, SessionStorage, SessionStorageLease, StorageError,
    StorageFuture, SubmissionAcknowledgement,
};
use crate::application::agent_execution::{
    agents::AgentError,
    caller_wake::{contain_caller_wake, CallerWaiter},
    executions::{
        limits::{reserve_observation_slot, ObservationUsage},
        ExecutionEvent, ExecutionRequest, ExecutionUpdate, SubmissionMode,
    },
    permissions::ActionContext,
    providers::{
        AgentProvider, ExecutionEventStream, ExecutionReport, ExecutionReportSource,
        FailedOpenCauseSource, FailedOpenCleanup, ProviderOpenControl, ProviderOpenRequest,
        ProviderSession,
    },
    tools::ToolReviewInput,
};
use crate::domain::agent_execution::{
    executions::{
        ExecutionId, ExecutionOutcome, InvocationHistory, InvocationKind, InvocationObservation,
        QueueMutation, QueueOrderChange, SchedulingCause,
    },
    permissions::PermissionRequest,
    sessions::SessionId,
};
use std::{
    collections::HashMap,
    future::poll_fn,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{Arc, RwLock},
    task::Poll,
    time::Duration,
};
use tokio::sync::Mutex;
use uuid::Uuid;

pub(crate) struct AttachmentOpenError {
    pub(crate) cause: AgentError,
    pub(crate) source: AttachmentOpenFailureSource,
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
    // Derived live text validation only. Saves invalidate it before any await;
    // restoration and consequential boundaries still validate complete evidence.
    message_histories: HashMap<ExecutionId, (usize, InvocationHistory)>,
    observed: Option<SessionSnapshot>,
    committed: Option<SessionSnapshot>,
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
    fn append_unit(&mut self, changes: Vec<SessionChange>) {
        self.pending
            .push(SessionSaveUnit::new(changes).expect("SDK decision boundary is nonempty"));
    }
    fn push_change(&mut self, change: SessionChange) {
        self.append_unit(vec![change]);
    }
}
const MESSAGE_COMMIT_DELAY: Duration = Duration::from_millis(100);
const MESSAGE_COMMIT_BYTES: usize = 16 * 1024;
const MESSAGE_COMMIT_COUNT: usize = 64;
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
    ///   output deadlines. Its wake only asks this manager to recheck pending facts.
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
        })
    }
    /// Returns the local conversation key used to acquire storage access.
    pub fn id(&self) -> &SessionId {
        &self.id
    }
    /// Last snapshot acknowledged by storage. None until Agent initialization.
    /// Failed writes never appear here as committed evidence.
    ///
    /// Waiting for the evidence lock registers the polling task's `Waker`
    /// with it; a panic from that waker when a save releases the lock is
    /// logged and does not fail the save. See "Caller wakers" in
    /// docs/agent_execution/lifecycle.md.
    pub async fn snapshot(&self) -> Option<SessionSnapshot> {
        let waiter = CallerWaiter::CommittedSnapshot(self.id.clone());
        contain_caller_wake(waiter, async {
            self.evidence.lock().await.committed.clone()
        })
        .await
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
    /// Joins admission writes after the Agent has excluded new invocations.
    pub(crate) async fn await_admission_writes(&self) {
        drop(self.evidence.lock().await);
    }
    pub(crate) async fn prepare(
        &mut self,
        provider: &dyn AgentProvider,
    ) -> Result<ProviderContext, AgentError> {
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
        let compacted = saved.as_ref().cloned();
        drop(saved);
        let identity = provider.identity();
        if compacted
            .as_ref()
            .is_some_and(|snapshot| snapshot.provider != identity)
        {
            return Err(AgentError::Storage(StorageError::IdentityMismatch));
        }
        let created = compacted.is_none();
        let mut snapshot = compacted.unwrap_or_else(|| SessionSnapshot {
            queue_history: Vec::new(),
            id: self.id.clone(),
            provider: identity,
            provider_context: ProviderContext::Absent,
            invocations: Vec::new(),
        });
        let mut changes = if created {
            vec![SessionChange::Opened {
                id: snapshot.id.clone(),
                provider: snapshot.provider.clone(),
                context: snapshot.provider_context.clone(),
            }]
        } else {
            Vec::new()
        };
        if !super::queue_validation::replay(&snapshot)
            .map_err(AgentError::Storage)?
            .is_empty()
        {
            Self::append_queue_mutation(&mut snapshot, QueueMutation::Restored, None)
                .map_err(AgentError::Storage)?;
            changes.push(SessionChange::QueueDecision(
                snapshot
                    .queue_history
                    .last()
                    .expect("restoration decision")
                    .clone(),
            ));
        }
        if unfinished && changes.is_empty() {
            return Err(AgentError::Storage(StorageError::Unresolved));
        }
        if !changes.is_empty() {
            let unit = SessionSaveUnit::new(changes).map_err(AgentError::Storage)?;
            let receipt = catch_storage_operation(|| {
                self.storage_lease.save_changes(
                    save_generation.clone(),
                    snapshot.clone(),
                    vec![unit],
                )
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
        let context = snapshot.provider_context.clone();
        *self.evidence.lock().await = Evidence {
            save_generation: Some(save_generation),
            event_usage: HashMap::new(),
            message_histories: HashMap::new(),
            observed: Some(snapshot.clone()),
            committed: Some(snapshot),
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
        let snapshot = self
            .evidence
            .lock()
            .await
            .observed
            .clone()
            .expect("prepared session evidence");
        let restore = snapshot.provider_context.recorded().cloned();
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
        if opened.session.capabilities() != provider.capabilities() {
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
            let next = evidence
                .observed
                .as_mut()
                .expect("prepared session evidence");
            let before = next.provider_context.clone();
            next.provider_context = ProviderContext::Recorded(session.id().clone());
            let next = next.clone();
            if before != next.provider_context {
                evidence.push_change(SessionChange::ProviderContext {
                    before,
                    after: next.provider_context.clone(),
                });
            }
            let result = if evidence.pending.is_empty() {
                Ok(())
            } else {
                let generation = evidence.generation();
                match generation {
                    Ok(generation) => {
                        let result = catch_storage_operation(|| {
                            self.storage_lease.save_changes(
                                generation.clone(),
                                next.clone(),
                                evidence.pending.clone(),
                            )
                        })
                        .await
                        .map_err(StorageError::bounded);
                        match result {
                            Ok(receipt) => {
                                match receipt.next_for(&generation, evidence.pending.len()) {
                                    Ok(next) => {
                                        evidence.save_generation = Some(next);
                                        Ok(())
                                    }
                                    Err(error) => Err(error),
                                }
                            }
                            Err(error) => Err(error),
                        }
                    }
                    Err(error) => Err(error),
                }
            };
            if result.is_ok() {
                evidence.committed = Some(next);
                evidence.pending.clear();
                evidence.message_commit = None;
            }
            result
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
        actor: ActionContext,
    ) -> Result<usize, AgentError> {
        self.begin_record(request, actor, SubmissionMode::Immediate, Vec::new())
            .await
    }
    /// Saves input and its first scheduling event atomically before admission.
    /// A failed write is reconciled by reading storage. Uncertain persisted input
    /// remains observed so a later save cannot erase it or retry it blindly.
    pub(crate) async fn begin_with_scheduling(
        &self,
        request: ExecutionRequest,
        actor: ActionContext,
        event: InvocationSchedulingEvent,
        submission: SubmissionMode,
    ) -> Result<usize, AgentError> {
        self.begin_record(request, actor, submission, vec![event])
            .await
    }
    async fn begin_record(
        &self,
        request: ExecutionRequest,
        actor: ActionContext,
        submission: SubmissionMode,
        scheduling: Vec<InvocationSchedulingEvent>,
    ) -> Result<usize, AgentError> {
        request.validate_message_size()?;
        InvocationSchedulingEvent::validate_history(&scheduling)
            .map_err(|error| AgentError::Storage(StorageError::Corrupt(error.to_string())))?;
        let storage_lease = self.storage_lease.clone();
        let session_id = self.id.clone();
        // Reserve this write before the caller can release its invocation guard.
        // Close can then join all supervised writes through the evidence mutex.
        let mut evidence = self.evidence.clone().lock_owned().await;
        tokio::spawn(async move {
            let snapshot = evidence
                .observed
                .as_ref()
                .expect("initialized agent session");
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
            let mut next = snapshot.clone();
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
            next.invocations.push(InvocationRecord {
                target_event_offset,
                submission,
                request,
                actor,
                acknowledgement: SubmissionAcknowledgement::Pending,
                events: Vec::new(),
                scheduling,
                provider_report: None,
                local_cancellation: None,
                local_outcome: None,
                cancellation: None,
                result: None,
            });
            // Reserve identity before polling untrusted storage. Even a task panic
            // after committing the write must not erase admission or permit replay.
            let index = next.invocations.len() - 1;
            let generation = evidence.generation().map_err(AgentError::Storage)?;
            let pending_before = evidence.pending.len();
            evidence.push_change(SessionChange::InputAccepted(Box::new(
                next.invocations[index].clone(),
            )));
            evidence.message_histories.clear();
            let previous = evidence.observed.replace(next);
            let save_result = storage_lease
                .save_changes(
                    generation.clone(),
                    evidence
                        .observed
                        .as_ref()
                        .expect("reserved admission")
                        .clone(),
                    evidence.pending.clone(),
                )
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
                            let next = evidence.observed.as_ref().expect("reserved admission");
                            let absent = saved.id == next.id
                                && saved.provider == next.provider
                                && saved.provider_context == next.provider_context
                                && super::validation::validate(&saved).is_ok()
                                && !saved.invocations.iter().any(|record| {
                                    record.request.execution_id
                                        == next
                                            .invocations
                                            .last()
                                            .expect("new input")
                                            .request
                                            .execution_id
                                });
                            saved.discard_rejected_errors();
                            absent
                        }
                        Err(_) => false,
                    };
                    if absent {
                        evidence.observed = previous;
                        evidence.pending.truncate(pending_before);
                    }
                    return Err(AgentError::Storage(error));
                }
            };
            let next_generation = receipt
                .next_for(&generation, evidence.pending.len())
                .map_err(AgentError::Storage)?;
            evidence.committed = evidence.observed.clone();
            evidence.pending.clear();
            evidence.message_commit = None;
            evidence.save_generation = Some(next_generation);
            Ok(index)
        })
        .await
        .map_err(|_| AgentError::SubmissionUnresolved)?
    }
    /// Latest locally observed submission, including evidence awaiting persistence.
    pub(crate) async fn submission(&self, id: &ExecutionId) -> Option<InvocationRecord> {
        self.evidence
            .lock()
            .await
            .observed
            .as_ref()?
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
            .observed
            .as_ref()
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
            .observed
            .as_ref()
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
            .observed
            .as_mut()
            .and_then(|snapshot| snapshot.invocations.get_mut(index))
            .ok_or_else(|| {
                StorageError::Corrupt("cancellation has no submitted invocation".into())
            })?;
        let mut history = super::validation::invocation_history(record)?;
        history
            .record_cancellation(cancellation.cancellation()?)
            .map_err(|error| StorageError::Corrupt(error.to_string()))?;
        let execution_id = record.request.execution_id.clone();
        record.cancellation = Some(cancellation.clone());
        evidence.push_change(SessionChange::StopDecision {
            execution_id,
            event: cancellation,
        });
        self.save_observed(&mut evidence).await
    }
    /// Check the bounded session-level reorder budget before changing live order.
    pub(crate) async fn check_queue_reorder_capacity(&self) -> Result<(), StorageError> {
        let evidence = self.evidence.lock().await;
        if evidence.observed.as_ref().is_some_and(|snapshot| {
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
    fn append_queue_mutation(
        snapshot: &mut SessionSnapshot,
        mutation: QueueMutation,
        actor: Option<ActionContext>,
    ) -> Result<(), StorageError> {
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
        snapshot.queue_history.push(QueueHistoryRecord {
            mutation,
            actor,
            scheduling_length,
        });
        if let Err(error) = super::queue_validation::replay(snapshot) {
            snapshot.queue_history.pop();
            return Err(error);
        }
        Ok(())
    }
    /// Record actual queue membership after the scheduler changed it. Failure
    /// retains observed evidence and must not roll back the live queue silently.
    pub(crate) async fn record_queue_mutation(
        &self,
        mutation: QueueMutation,
        actor: Option<ActionContext>,
    ) -> Result<(), StorageError> {
        let mut evidence = self.evidence.lock().await;
        let snapshot = evidence
            .observed
            .as_mut()
            .ok_or_else(|| StorageError::Corrupt("queue has no session".into()))?;
        Self::append_queue_mutation(snapshot, mutation, actor)?;
        let decision = snapshot
            .queue_history
            .last()
            .expect("queue decision")
            .clone();
        evidence.push_change(SessionChange::QueueDecision(decision));
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
        let snapshot = evidence
            .observed
            .as_mut()
            .ok_or_else(|| StorageError::Corrupt("queue has no session".into()))?;
        // Either both effects happen or neither does, so a refused order change
        // leaves no retained mutation the live queue never made. Only this
        // transition rolls back: every other queue record describes an effect
        // the scheduler has already performed.
        let retained = snapshot.queue_history.len();
        let result =
            Self::append_queue_mutation(snapshot, QueueMutation::Reordered(change), Some(actor))
                .and_then(|()| apply());
        if result.is_err() {
            snapshot.queue_history.truncate(retained);
        } else {
            let decision = snapshot
                .queue_history
                .last()
                .expect("queue reorder")
                .clone();
            evidence.push_change(SessionChange::QueueDecision(decision));
        }
        result
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
            .observed
            .as_mut()
            .ok_or_else(|| StorageError::Corrupt("acknowledgement has no session".into()))?
            .invocations
            .get_mut(index)
            .ok_or_else(|| {
                StorageError::Corrupt("acknowledgement has no submitted invocation".into())
            })?;
        let before = record.acknowledgement.clone();
        if before != acknowledgement {
            let execution_id = record.request.execution_id.clone();
            record.acknowledgement = acknowledgement.clone();
            evidence.push_change(SessionChange::ReceiptUpdated {
                execution_id,
                before,
                after: acknowledgement,
            });
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
    /// Persist a lifecycle edge and its corresponding actual queue removal together.
    pub(crate) async fn record_scheduling_with_queue(
        &self,
        index: usize,
        event: InvocationSchedulingEvent,
        mutation: Option<QueueMutation>,
    ) -> Result<(), StorageError> {
        let mut evidence = self.evidence.lock().await;
        let snapshot = evidence
            .observed
            .as_mut()
            .ok_or_else(|| StorageError::Corrupt("scheduling event has no session".into()))?;
        let record = snapshot.invocations.get_mut(index).ok_or_else(|| {
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
        record.scheduling.push(event.clone());
        let decision = if let Some(mutation) = mutation {
            if let Err(error) = Self::append_queue_mutation(snapshot, mutation, actor) {
                snapshot.invocations[index].scheduling.pop();
                return Err(error);
            }
            Some(
                snapshot
                    .queue_history
                    .last()
                    .expect("queue removal")
                    .clone(),
            )
        } else {
            None
        };
        let mut changes = vec![SessionChange::SchedulingTransition {
            execution_id,
            event,
        }];
        if let Some(decision) = decision {
            changes.push(SessionChange::QueueDecision(decision));
        }
        evidence.append_unit(changes);
        self.save_observed(&mut evidence).await
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
        let mut next = evidence
            .observed
            .as_ref()
            .ok_or_else(|| StorageError::Corrupt("queue settlement has no session".into()))?
            .clone();
        let record = next.invocations.get_mut(index).ok_or_else(|| {
            StorageError::Corrupt("queue settlement has no submitted invocation".into())
        })?;
        let execution_id = record.request.execution_id.clone();
        let before = record.result.clone();
        record.local_outcome = result.as_ref().ok().copied();
        record.result = Some(result.clone());
        record.scheduling.push(event.clone());
        let local_outcome = record.local_outcome;
        super::validation::invocation_history(record)?;
        Self::append_queue_mutation(&mut next, mutation, None)?;
        evidence.append_unit(vec![
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
            SessionChange::QueueDecision(
                next.queue_history
                    .last()
                    .expect("failed queue removal")
                    .clone(),
            ),
        ]);
        evidence.observed = Some(next);
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
                .observed
                .as_ref()
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
        if !save {
            let id = event.execution_id().clone();
            if !evidence.message_histories.contains_key(&id) {
                let snapshot = evidence
                    .observed
                    .as_ref()
                    .expect("initialized agent session");
                let index = snapshot
                    .invocations
                    .iter()
                    .position(|record| record.request.execution_id == id)
                    .ok_or_else(|| {
                        StorageError::Corrupt(
                            "provider observation has no submitted invocation".into(),
                        )
                    })?;
                let history = super::validation::invocation_history(&snapshot.invocations[index])?;
                evidence
                    .message_histories
                    .insert(id.clone(), (index, history));
            }
            let (index, history) = evidence
                .message_histories
                .get_mut(&id)
                .expect("initialized text history");
            history
                .observe(&id, InvocationObservation::Output)
                .map_err(|error| StorageError::Corrupt(error.to_string()))?;
            let index = *index;
            let record = &mut evidence
                .observed
                .as_mut()
                .expect("initialized agent session")
                .invocations[index];
            reserve_observation_slot(&mut record.events);
            record.events.push(event.clone());
            evidence.push_change(SessionChange::ProviderObservation(event));
            evidence.event_usage.insert(id, usage);
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
            return Ok(());
        }
        evidence.message_histories.clear();
        let snapshot = evidence
            .observed
            .as_mut()
            .expect("initialized agent session");
        // Output from an abandoned caller wait still belongs to its original input.
        let record = snapshot
            .invocations
            .iter_mut()
            .find(|record| &record.request.execution_id == event.execution_id())
            .ok_or_else(|| {
                StorageError::Corrupt("provider observation has no submitted invocation".into())
            })?;
        let execution_id = event.execution_id().clone();
        reserve_observation_slot(&mut record.events);
        record.events.push(event.clone());
        if let Err(error) = super::validation::validate(snapshot) {
            snapshot
                .invocations
                .iter_mut()
                .find(|record| record.request.execution_id == execution_id)
                .expect("matched invocation")
                .events
                .pop();
            return Err(error);
        }
        evidence.push_change(SessionChange::ProviderObservation(event));
        evidence.event_usage.insert(execution_id, usage);
        // Text/thought fragments are live output until a bounded output flush
        // or the next boundary save. Tools, reviews, terminal events and
        // settlement persist the accumulated observations. A process failure
        // may lose only the unfinished streaming suffix.
        if save {
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
        let record = &mut evidence
            .observed
            .as_mut()
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
        let new_report = record.provider_report.is_none();
        if let Some(prior) = &record.provider_report {
            if prior != &settlement || record.local_cancellation != cancellation {
                return Err(StorageError::Corrupt(
                    "provider settlement cannot be replaced".into(),
                ));
            }
        } else {
            record.provider_report = Some(settlement.clone());
            record.local_cancellation = cancellation.clone();
        }
        if new_report {
            evidence.push_change(SessionChange::ProviderReport {
                execution_id,
                report: settlement,
                local_stop: cancellation,
            });
        }
        self.save_observed(&mut evidence).await
    }
    /// Cancellation is an explicit fact, never inferred from nested diagnostics.
    pub(crate) async fn invocation_cancelled(&self, index: usize) -> bool {
        let evidence = self.evidence.lock().await;
        let record = &evidence
            .observed
            .as_ref()
            .expect("initialized agent session")
            .invocations[index];
        record.provider_report.as_ref().is_some_and(|settlement| {
            settlement.source() == ExecutionReportSource::LocalCancellation
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
        let record = &mut evidence
            .observed
            .as_mut()
            .expect("initialized agent session")
            .invocations[index];
        let before = record.result.clone();
        let prior_outcome = record.local_outcome;
        let execution_id = record.request.execution_id.clone();
        record.local_outcome = local_outcome;
        record.result = Some(result.clone());
        if before.as_ref() != Some(&result) || prior_outcome != local_outcome {
            evidence.push_change(SessionChange::LocalSettlement {
                execution_id,
                before,
                after: result,
                local_outcome,
            });
        }
        self.save_observed(&mut evidence).await
    }
    /// Reads the last scheduling edge by its stable admitted record index.
    pub(crate) async fn scheduling_event(&self, index: usize) -> Option<InvocationSchedulingEvent> {
        self.evidence
            .lock()
            .await
            .observed
            .as_ref()?
            .invocations
            .get(index)?
            .scheduling
            .last()
            .cloned()
    }

    /// Narrow observed scheduling lookup for supervisor recovery.
    pub(crate) async fn scheduling_metadata(
        &self,
        id: &ExecutionId,
    ) -> Option<(usize, InvocationSchedulingEvent)> {
        let evidence = self.evidence.lock().await;
        let (index, record) = evidence
            .observed
            .as_ref()?
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
            .observed
            .as_ref()
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
        let Some(index) = evidence.observed.as_ref().and_then(|snapshot| {
            snapshot
                .invocations
                .iter()
                .position(|record| &record.request.execution_id == id)
        }) else {
            return Err(error);
        };
        let prior = evidence
            .observed
            .as_ref()
            .expect("matched invocation")
            .invocations[index]
            .result
            .clone()
            .or_else(|| {
                evidence
                    .observed
                    .as_ref()
                    .expect("matched invocation")
                    .invocations[index]
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
            let record = &mut evidence
                .observed
                .as_mut()
                .expect("initialized agent session")
                .invocations[index];
            let execution_id = record.request.execution_id.clone();
            let local_outcome = record.local_outcome;
            record.result = Some(result.clone());
            evidence.push_change(SessionChange::LocalSettlement {
                execution_id,
                before: Some(before),
                after: result.clone(),
                local_outcome,
            });
        }
        (result, storage_failure)
    }
    fn update_result(
        evidence: &mut Evidence,
        index: usize,
        result: &Result<ExecutionOutcome, AgentError>,
    ) -> Result<bool, StorageError> {
        let local_outcome = Self::validate_result(evidence, index, result)?;
        let record = &mut evidence
            .observed
            .as_mut()
            .expect("initialized agent session")
            .invocations[index];
        if record.result.as_ref() == Some(result) {
            return Ok(false);
        }
        let before = record.result.clone();
        let execution_id = record.request.execution_id.clone();
        record.local_outcome = local_outcome;
        record.result = Some(result.clone());
        evidence.push_change(SessionChange::LocalSettlement {
            execution_id,
            before,
            after: result.clone(),
            local_outcome,
        });
        Ok(true)
    }
    async fn save_observed(&self, evidence: &mut Evidence) -> Result<(), StorageError> {
        if evidence.pending.is_empty() {
            return Ok(());
        }
        evidence.message_histories.clear();
        let snapshot = evidence
            .observed
            .as_ref()
            .expect("initialized agent session")
            .clone();
        let generation = evidence.generation()?;
        let receipt = self
            .storage_lease
            .save_changes(
                generation.clone(),
                snapshot.clone(),
                evidence.pending.clone(),
            )
            .await
            .map_err(StorageError::bounded)?;
        let next_generation = receipt.next_for(&generation, evidence.pending.len())?;
        evidence.committed = Some(snapshot);
        evidence.pending.clear();
        evidence.message_commit = None;
        evidence.save_generation = Some(next_generation);
        Ok(())
    }
    fn validate_result(
        evidence: &Evidence,
        index: usize,
        result: &Result<ExecutionOutcome, AgentError>,
    ) -> Result<Option<ExecutionOutcome>, StorageError> {
        let record = &evidence
            .observed
            .as_ref()
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
