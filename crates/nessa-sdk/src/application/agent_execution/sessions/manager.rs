use super::{
    attachment::AttachmentLease, InvocationCancellationEvent, InvocationRecord,
    InvocationSchedulingEvent, ProviderContext, QueueHistoryRecord, SessionSnapshot,
    SessionStorage, SessionStorageLease, StorageError, StorageFuture, SubmissionAcknowledgement,
};
use crate::application::agent_execution::{
    agents::AgentError,
    executions::{
        limits::{reserve_observation_slot, ObservationUsage},
        ExecutionEvent, ExecutionRequest, ExecutionUpdate, SubmissionMode,
    },
    permissions::ActionContext,
    providers::{
        AgentProvider, ExecutionEventStream, ExecutionReport, ExecutionReportSource,
        FailedOpenCleanup, ProviderOpenControl, ProviderOpenRequest, ProviderSession,
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
};
use tokio::sync::Mutex;
use uuid::Uuid;

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
///     infrastructure::session_storage::InMemoryStorage,
/// };
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let storage = Arc::new(InMemoryStorage::new());
/// let manager = SessionManager::open(Some(SessionId::new("conversation")?), storage).await?;
/// assert!(manager.snapshot().await.is_none()); // No Agent attached yet.
/// drop(manager); // Releases the writer lease without deleting saved data.
/// # Ok(())
/// # }
/// ```
pub struct SessionManager {
    id: SessionId,
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
    event_usage: HashMap<ExecutionId, ObservationUsage>,
    // Derived live text validation only. Saves invalidate it before any await;
    // restoration and consequential boundaries still validate complete evidence.
    message_histories: HashMap<ExecutionId, (usize, InvocationHistory)>,
    observed: Option<SessionSnapshot>,
    committed: Option<SessionSnapshot>,
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
    ///
    /// # Errors
    /// Returns [`StorageError::Busy`] if another owner holds this session, or a
    /// backend error if access fails. No provider is opened and no model is called.
    /// Dropping the manager releases access, including before attaching an Agent.
    pub async fn open(
        id: Option<SessionId>,
        storage: Arc<dyn SessionStorage>,
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
    pub async fn snapshot(&self) -> Option<SessionSnapshot> {
        self.evidence.lock().await.committed.clone()
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
        if let Some(snapshot) = &saved {
            if let Err(error) = super::validation::validate(snapshot) {
                saved.expect("validated snapshot").discard_rejected_errors();
                return Err(AgentError::Storage(error));
            }
        }
        let compacted = saved.as_ref().cloned();
        drop(saved);
        let identity = provider.identity();
        if compacted
            .as_ref()
            .is_some_and(|snapshot| snapshot.id != self.id || snapshot.provider != identity)
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
        let mut changed = created;
        if !super::queue_validation::replay(&snapshot)
            .map_err(AgentError::Storage)?
            .is_empty()
        {
            Self::append_queue_mutation(&mut snapshot, QueueMutation::Restored, None)
                .map_err(AgentError::Storage)?;
            changed = true;
        }
        if changed {
            catch_storage_operation(|| self.storage_lease.save(snapshot.clone()))
                .await
                .map_err(StorageError::bounded)
                .map_err(AgentError::Storage)?;
        }
        let context = snapshot.provider_context.clone();
        *self.evidence.lock().await = Evidence {
            event_usage: HashMap::new(),
            message_histories: HashMap::new(),
            observed: Some(snapshot.clone()),
            committed: Some(snapshot),
        };
        Ok(context)
    }

    pub(crate) async fn attach(
        &self,
        provider: &dyn AgentProvider,
        control: ProviderOpenControl,
    ) -> Result<AttachedProvider, AgentError> {
        let snapshot = self
            .evidence
            .lock()
            .await
            .observed
            .clone()
            .expect("prepared session evidence");
        let restore = snapshot.provider_context.recorded().cloned();
        let opening = catch_unwind(AssertUnwindSafe(|| {
            provider.open(ProviderOpenRequest::new(restore.clone(), control))
        }));
        let mut opening = match opening {
            Ok(opening) => opening,
            Err(payload) => {
                std::mem::forget(payload);
                self.attachment
                    .arm_unknown_open(self.storage_lease.clone())
                    .await;
                return Err(AgentError::CleanupUncertain);
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
                    let (error, cleanup) = error.into_parts();
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
            return Err(cause);
        }
        let opened = match result.expect("completed provider open") {
            Ok(opened) => opened,
            Err(error) => {
                let (cause, cleanup) = error.into_parts();
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
                return Err(cause);
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
            return Err(AgentError::StorageInitialization {
                error: StorageError::IdentityMismatch,
                cleanup_result: Box::new(cleanup_result.into_result()),
            });
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
            return Err(AgentError::StorageInitialization {
                error: StorageError::IdentityMismatch,
                cleanup_result: Box::new(cleanup_result.into_result()),
            });
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
            next.provider_context = ProviderContext::Recorded(session.id().clone());
            let next = next.clone();
            let result = catch_storage_operation(|| self.storage_lease.save(next.clone()))
                .await
                .map_err(StorageError::bounded);
            if result.is_ok() {
                evidence.committed = Some(next);
            }
            result
        };
        if let Err(error) = save_result {
            let cleanup_result = self.attachment.cleanup().await;
            return Err(AgentError::StorageInitialization {
                error,
                cleanup_result: Box::new(cleanup_result.into_result()),
            });
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
            let mut next = snapshot.clone();
            let target_event_offset = scheduling
                .first()
                .and_then(|edge| edge.target.as_ref())
                .and_then(|target| {
                    snapshot
                        .invocations
                        .iter()
                        .find(|record| &record.request.execution_id == target)
                })
                .map(|record| record.events.len());
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
            evidence.message_histories.clear();
            let previous = evidence.observed.replace(next);
            if let Err(error) = storage_lease
                .save(
                    evidence
                        .observed
                        .as_ref()
                        .expect("reserved admission")
                        .clone(),
                )
                .await
                .map_err(StorageError::bounded)
            {
                // save may have replaced the file before failing to acknowledge it.
                // Only a successful read proving this identity absent permits retry.
                let absent = match storage_lease.load().await.map_err(StorageError::bounded) {
                    Ok(None) => true,
                    Ok(Some(saved)) => {
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
                }
                return Err(AgentError::Storage(error));
            }
            evidence.committed = evidence.observed.clone();
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
        record.cancellation = Some(cancellation);
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
        let _ = super::queue_validation::replay(snapshot)?;
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
        record.acknowledgement = acknowledgement;
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
        record.scheduling.push(event);
        if let Some(mutation) = mutation {
            Self::append_queue_mutation(snapshot, mutation, actor)?;
        }
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
        record.local_outcome = result.as_ref().ok().copied();
        record.result = Some(result);
        record.scheduling.push(event);
        super::validation::invocation_history(record)?;
        Self::append_queue_mutation(&mut next, mutation, None)?;
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
            record.events.push(event);
            evidence.event_usage.insert(id, usage);
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
        record.events.push(event);
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
        evidence.event_usage.insert(execution_id, usage);
        // Text/thought fragments are live output until the next boundary save.
        // Tools, reviews, terminal events and settlement persist the accumulated
        // observations. A process failure may lose unfinished streaming text.
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
        let mut history = super::validation::invocation_history(record)?;
        super::validation::record_report(
            &mut history,
            Some(&settlement),
            cancellation.as_ref(),
            record.scheduling.last(),
        )?;
        if let Some(prior) = &record.provider_report {
            if prior != &settlement || record.local_cancellation != cancellation {
                return Err(StorageError::Corrupt(
                    "provider settlement cannot be replaced".into(),
                ));
            }
        } else {
            record.provider_report = Some(settlement);
            record.local_cancellation = cancellation;
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
        record.local_outcome = local_outcome;
        record.result = Some(result);
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
            result = Err(AgentError::StorageAfterExecution {
                error,
                execution_result: Box::new(result),
            }
            .bounded());
            evidence
                .observed
                .as_mut()
                .expect("initialized agent session")
                .invocations[index]
                .result = Some(result.clone());
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
        record.local_outcome = local_outcome;
        record.result = Some(result.clone());
        Ok(true)
    }
    async fn save_observed(&self, evidence: &mut Evidence) -> Result<(), StorageError> {
        evidence.message_histories.clear();
        let snapshot = evidence
            .observed
            .as_ref()
            .expect("initialized agent session")
            .clone();
        self.storage_lease
            .save(snapshot.clone())
            .await
            .map_err(StorageError::bounded)?;
        evidence.committed = Some(snapshot);
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
        super::validation::validate_local_result(record, result)?;
        let mut history = super::validation::invocation_history(record)?;
        history
            .record_local_result(result.as_ref().copied().map_err(|_| ()))
            .map_err(|error| StorageError::Corrupt(error.to_string()))?;
        Ok(history.local_outcome())
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
