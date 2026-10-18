//! Owns accepted work independently of a surface's wait future.
//! Pending owners retain persistence and recovery, then retire before receipt
//! publication. The queue runner also retires their original invocation slot;
//! separate invocation and attachment owners retain their own resources.
#![deny(missing_docs)]

use super::audit_observation::{AuditObservationFailure, AUDIT_OBSERVATION_TIMEOUT};
use super::lifecycle::{SessionLifecycle, WorkGeneration, WorkPermit};
use super::submissions::{self, Settlement, SubmissionReceipt};
use super::{Agent, AgentError, AttachmentPhase, AttachmentRequest};
use crate::application::agent_execution::{
    caller_wake::{contain_caller_wake, CallerWaiter},
    executions::{
        AfterSaveCheck, AfterSaveRefusal, ExecutionAuditRecord, ExecutionRequest,
        QueueAdmissionRecord, QueueOrderRecord, QueueSettlementRecord,
        SteeringAcknowledgementRecord, SubmissionMode, Submitter,
    },
    permissions::ActionContext,
    providers::{
        validate_configured_input, CloseOutcome, ProviderOperationFailure, ProviderSessionState,
        SessionCloseRequest, SteeringOutcome,
    },
    sessions::{
        InvocationSchedulingEvent, NoteCandidates, NoteIntake, StorageError,
        SubmissionAcknowledgement,
    },
};
use crate::domain::agent_execution::executions::{
    ExecutionId, ExecutionOutcome, InvocationKind, InvocationQueue, InvocationStage, QueueMutation,
    QueueOrderChange, QueueOrderError, QueueRemovalCause, SchedulingCause, SchedulingInitiator,
    SchedulingTransition,
};
use std::{
    collections::{HashMap, HashSet},
    future::{poll_fn, Future},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
    task::Poll,
};
use tokio::{
    sync::watch,
    time::{sleep, sleep_until, Instant},
};

/// Result of replacing the complete pending dispatch order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueReorder {
    /// The requested order was applied and its evidence saved.
    Applied,
    /// It was already the current order; outstanding evidence was saved.
    Unchanged,
    /// Pending membership changed; refresh and propose a new complete order.
    QueueChanged,
    /// Steering inputs cannot be moved behind ordinary queued input.
    PriorityConflict,
}

/// Receipt for admitted work. Dropping it stops waiting, not execution.
/// Accepted inputs are saved before this receipt is returned. The owning Tokio
/// runtime must remain alive; process/runtime shutdown does not replay unfinished
/// work automatically. Use Agent::close before shutting down the runtime.
pub struct QueuedInvocation {
    id: ExecutionId,
    result: watch::Receiver<Settlement>,
}

/// Why mandatory queue-admission evidence was not acknowledged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmissionEvidenceFailure {
    audit: Option<AgentError>,
    storage: Option<StorageError>,
}
impl AdmissionEvidenceFailure {
    pub(super) fn new(audit: Option<AgentError>, storage: Option<StorageError>) -> Option<Self> {
        (audit.is_some() || storage.is_some()).then_some(Self { audit, storage })
    }
    /// Mandatory audit failure, when the sink did not acknowledge ownership.
    pub fn audit(&self) -> Option<&AgentError> {
        self.audit.as_ref()
    }
    /// Durable snapshot failure, when queue or delivery evidence was not saved.
    pub fn storage(&self) -> Option<&StorageError> {
        self.storage.as_ref()
    }
    fn retain_storage(&mut self, storage: StorageError) {
        self.storage.get_or_insert(storage);
    }
}

/// Whether mandatory admission evidence was acknowledged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmissionEvidence {
    /// Audit and storage acknowledged queue admission.
    Acknowledged,
    /// The Agent still owns the receipt; inspect this separate evidence failure.
    Failed(AdmissionEvidenceFailure),
}

/// Evidence acknowledgement accompanying a confirmed native steering effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SteeringEvidence {
    /// Audit and durable scheduling evidence acknowledged the provider effect.
    Acknowledged,
    /// The provider effect remains confirmed despite a later evidence failure.
    Failed(AdmissionEvidenceFailure),
}

/// Owned queue admission with its stable receipt and evidence acknowledgement.
pub struct QueueAdmission {
    receipt: QueuedInvocation,
    evidence: AdmissionEvidence,
}
impl QueueAdmission {
    pub(super) fn new(receipt: QueuedInvocation, evidence: AdmissionEvidence) -> Self {
        Self { receipt, evidence }
    }
    /// Stable execution identity retained by the owned receipt.
    pub fn id(&self) -> &ExecutionId {
        self.receipt.id()
    }
    /// Independent audit/storage acknowledgement for this admission.
    pub fn evidence(&self) -> &AdmissionEvidence {
        &self.evidence
    }
    /// Consume this receipt and wait for invocation settlement.
    /// Inspect [`Self::evidence`] first when acknowledgement details matter.
    pub async fn wait(self) -> Result<ExecutionOutcome, AgentError> {
        self.receipt.wait().await
    }
    fn retain_storage_failure(&mut self, storage: StorageError) {
        match &mut self.evidence {
            AdmissionEvidence::Acknowledged => {
                self.evidence = AdmissionEvidence::Failed(AdmissionEvidenceFailure {
                    audit: None,
                    storage: Some(storage),
                });
            }
            AdmissionEvidence::Failed(failure) => failure.retain_storage(storage),
        }
    }
}
impl SteeringEvidence {
    fn retain_storage_failure(&mut self, storage: StorageError) {
        match self {
            Self::Acknowledged => {
                *self = Self::Failed(AdmissionEvidenceFailure {
                    audit: None,
                    storage: Some(storage),
                });
            }
            Self::Failed(failure) => failure.retain_storage(storage),
        }
    }
}
impl QueuedInvocation {
    pub(super) fn new(id: ExecutionId, result: watch::Receiver<Settlement>) -> Self {
        Self { id, result }
    }
    /// Identity supplied in the admitted request; use it to correlate saved evidence.
    pub fn id(&self) -> &ExecutionId {
        &self.id
    }

    /// Waits for settlement and invocation hooks. Errors include provider, hook,
    /// persistence, and explicit session-close failures. Closed can indicate local
    /// cancellation or provider disconnection; it does not confirm external cleanup.
    /// SubmissionUnresolved means the runner vanished without settlement.
    ///
    /// Publication retires this receipt's original accepted-work owner and its
    /// invocation slot, as coordinated by the queue runner and Pending publisher.
    /// Independent work can still make a direct invocation Busy; the receipt does
    /// not establish universal admission readiness or confirmed provider cleanup.
    ///
    /// A panic raised by the polling task's `Waker` when the result is published
    /// stays with this wait: it is logged, that one wake is lost, and the result
    /// stays retained for the next poll. The publisher, other waiters on the same
    /// receipt, and other queued work are unaffected. See "Caller wakers" in
    /// docs/agent_execution/lifecycle.md for what is not contained.
    pub async fn wait(self) -> Result<ExecutionOutcome, AgentError> {
        let Self { id, mut result } = self;
        contain_caller_wake(CallerWaiter::Receipt(id), async move {
            loop {
                if let Some(settled) = result.borrow_and_update().clone() {
                    return settled;
                }
                result
                    .changed()
                    .await
                    .map_err(|_| AgentError::SubmissionUnresolved)?;
            }
        })
        .await
    }
}

/// Acknowledged delivery path for one steering input.
pub enum SteeringDelivery {
    /// Provider acknowledged injection into `target`; its output and completion
    /// still belong to that original execution. This is not a completed tool effect.
    Injected {
        /// Active execution that accepted this additional input.
        target: ExecutionId,
        /// Audit/storage acknowledgement retained separately from provider acknowledgement.
        evidence: SteeringEvidence,
    },
    /// No turn consumed the input; it was admitted for the next invocation boundary.
    Queued(QueueAdmission),
}

/// Result of removing an input from the local pending queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueRemoval {
    /// Removed before dispatch; its receipt resolves Closed and evidence remains.
    Removed,
    /// Unknown, already removed, dispatched, or injected. No effect was undone.
    NotPending,
}

/// Whether a close ends the work it stops, or leaves it owed for the next start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CloseMode {
    /// A person or the host ended this agent's work.
    Final,
    /// The host stopped for its own shutdown; stopped work is resumed later.
    Suspend,
}

pub(super) struct Scheduler {
    queue: InvocationQueue,
    pending: HashMap<ExecutionId, Pending>,
    running: bool,
    receipts: HashMap<ExecutionId, SubmissionReceipt>,
}
struct Pending {
    input: ExecutionRequest,
    submitter: Submitter,
    index: usize,
    kind: InvocationKind,
    target: Option<ExecutionId>,
    reply: watch::Sender<Settlement>,
    _work: WorkPermit,
}
impl Pending {
    fn publish(self, result: Result<ExecutionOutcome, AgentError>) {
        let Self {
            reply, _work: work, ..
        } = self;
        // The original admitted work retires before its consumer can run.
        // Other invocation and attachment owners retain their own resources.
        drop(work);
        reply.send_replace(Some(result));
    }
}
impl Scheduler {
    // Nothing for a runner to select or settle. Cancellation drains the queue
    // before it settles each owner, so an owner whose settlement was cut short
    // stays in `pending` with no queue entry. A runner then takes the slot:
    // it releases that slot before settling a stopped owner. Otherwise, with
    // nothing else queued, it passes over it and exits, leaving it to the next
    // `cancel_pending`, which collects owners whether or not they are queued.
    // Both orderings are rows of the table in
    // docs/agent_execution/scheduling.md, each with its test.
    pub(super) fn has_no_work(&self) -> bool {
        self.queue.is_empty() && self.pending.is_empty()
    }
    pub(super) fn is_idle(&self) -> bool {
        !self.running && self.queue.is_empty()
    }
    /// Whether this Agent admitted `id` itself, so it owns its receipt.
    pub(super) fn has_receipt(&self, id: &ExecutionId) -> bool {
        self.receipts.contains_key(id)
    }
    pub(super) fn new() -> Self {
        Self {
            queue: InvocationQueue::new(64).expect("positive queue bound"),
            pending: HashMap::new(),
            running: false,
            receipts: HashMap::new(),
        }
    }
}

pub(super) struct ActiveInvocation<'a> {
    lifecycle: &'a SessionLifecycle,
    work_generation: WorkGeneration,
}
impl<'a> ActiveInvocation<'a> {
    pub(super) fn new(
        lifecycle: &'a SessionLifecycle,
        id: ExecutionId,
    ) -> Result<Self, AgentError> {
        Ok(Self {
            lifecycle,
            work_generation: lifecycle.start_execution(id)?,
        })
    }
}
impl Drop for ActiveInvocation<'_> {
    fn drop(&mut self) {
        self.lifecycle.finish_execution(self.work_generation);
    }
}

fn event(
    kind: InvocationKind,
    target: Option<ExecutionId>,
    before: Option<InvocationStage>,
    stage: InvocationStage,
    cause: SchedulingCause,
    actor: Option<ActionContext>,
) -> InvocationSchedulingEvent {
    let transition = SchedulingTransition::new(
        kind,
        target,
        before,
        stage,
        cause,
        if actor.is_some() {
            SchedulingInitiator::Caller
        } else {
            SchedulingInitiator::Automatic
        },
    )
    .expect("valid runtime scheduling transition");
    InvocationSchedulingEvent {
        kind: transition.kind(),
        target: transition.target().cloned(),
        before: transition.before(),
        stage: transition.stage(),
        cause: transition.cause(),
        actor,
    }
}

impl Agent {
    async fn persist_submission_acknowledgement(
        &self,
        index: usize,
        audit: Option<AgentError>,
        storage: Option<StorageError>,
        panic_diagnostic: &'static str,
    ) -> Option<AdmissionEvidenceFailure> {
        let mut failure = AdmissionEvidenceFailure::new(
            audit.map(AgentError::bounded),
            storage.map(StorageError::bounded),
        );
        let acknowledgement = match &failure {
            Some(failure) => SubmissionAcknowledgement::Failed {
                audit: failure.audit.clone(),
                storage: failure.storage.clone(),
            },
            None => SubmissionAcknowledgement::Acknowledged,
        };
        if let Err(error) = self
            .inner
            .manager
            .retain_submission_acknowledgement(index, acknowledgement)
            .await
        {
            failure = AdmissionEvidenceFailure::new(
                failure.as_ref().and_then(|failure| failure.audit.clone()),
                Some(error),
            );
            return failure;
        }
        let saved = self
            .catch_scheduling_panic(self.inner.manager.flush_observed())
            .await;
        let storage = match saved {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(error),
            Err(()) => Some(StorageError::Io(panic_diagnostic.into())),
        };
        if let Some(storage) = storage {
            let mut retained = failure.unwrap_or(AdmissionEvidenceFailure {
                audit: None,
                storage: None,
            });
            retained.retain_storage(storage);
            let acknowledgement = SubmissionAcknowledgement::Failed {
                audit: retained.audit.clone(),
                storage: retained.storage.clone(),
            };
            // This mutation has no adapter call and cannot erase the exact live
            // receipt fact. The subsequent stop/settlement write persists it.
            if let Err(error) = self
                .inner
                .manager
                .retain_submission_acknowledgement(index, acknowledgement)
                .await
            {
                retained.retain_storage(error);
            }
            failure = Some(retained);
        }
        failure
    }

    /// Saves `input` with its `submitter` (the verified caller and how the turn
    /// was started), then schedules a sequential invocation.
    ///
    /// Ordinary inputs default to FIFO across all Agent clones; explicit
    /// reorder_queued calls may replace order within a priority class. At
    /// most 64 inputs may wait; a full queue returns a Scheduling error. Retrying
    /// the same execution ID, input, submitter, and operation joins its original receipt,
    /// even after settlement. Changed intent returns SubmissionConflict. Restored
    /// unresolved delivery returns SubmissionUnresolved without resending it;
    /// one the host resumed ([`Self::resume_turn`]) answers as its successor.
    /// WorkStatus is supervised once polled, including while saving input.
    /// Validation/storage errors prevent admission. Each dispatched input runs the
    /// normal invocation hooks. Dropping the returned receipt does not cancel it.
    /// Queued work is retained but is never replayed by itself after restart:
    /// the host asks [`Self::owed_turns`] and decides what to resume.
    ///
    /// # Examples
    /// ```
    /// use nessa_sdk::{Agent, application::agent_execution::{
    ///     agents::AgentError, executions::{ExecutionRequest, Submitter}, permissions::ActionContext,
    /// }};
    /// # async fn submit(agent: &Agent, input: ExecutionRequest, actor: ActionContext) -> Result<(), AgentError> {
    /// let receipt = agent.enqueue(input, Submitter::owner(actor)).await?;
    /// let outcome = receipt.wait().await?;
    /// # let _ = outcome;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// The caller's `Waker` is woken when the operation's own task finishes;
    /// a panic from it is logged and does not affect the operation. See
    /// "Caller wakers" in docs/agent_execution/lifecycle.md.
    pub async fn enqueue(
        &self,
        input: ExecutionRequest,
        submitter: Submitter,
    ) -> Result<QueueAdmission, AgentError> {
        self.enqueue_kind(
            input,
            submitter,
            InvocationKind::Queued,
            NoteCandidates::default(),
        )
        .await
    }

    /// [`Self::enqueue`], with the host's verdict on the conversation's
    /// pending `next_turn` notes: which authors it checked and still allows,
    /// and which lost the right to leave them.
    ///
    /// Admitted while the agent is idle (nothing running, nothing queued),
    /// the turn takes up to eight pending notes `candidates` authorizes, in
    /// the order they were left, and the model is given them with its message,
    /// each labelled with its author. Admitted behind other work, it takes
    /// none: they wait for a turn that starts from them. Either way the
    /// admission retires, in the same save, each pending note past its 24-hour
    /// lifetime and each `candidates` names as revoked. A note in neither list
    /// waits. A retry of the same submission answers from its first admission
    /// and selects nothing again; `candidates` is not part of what a retry
    /// must repeat.
    ///
    /// The caller's `Waker` is woken when the operation's own task finishes;
    /// a panic from it is logged and does not affect the operation. See
    /// "Caller wakers" in docs/agent_execution/lifecycle.md.
    ///
    /// # Errors
    ///
    /// As [`Self::enqueue`].
    pub async fn enqueue_with_notes(
        &self,
        input: ExecutionRequest,
        submitter: Submitter,
        candidates: NoteCandidates,
    ) -> Result<QueueAdmission, AgentError> {
        self.enqueue_kind(input, submitter, InvocationKind::Queued, candidates)
            .await
    }

    /// Saves `input` from verified `submitter` for the next invocation boundary, ahead
    /// of ordinary queued inputs. Steering inputs preserve their own FIFO order.
    /// This explicit boundary operation does not interrupt or inject into a turn.
    /// WorkStatus and failure guarantees are the same as [`Self::enqueue`].
    ///
    /// The caller's `Waker` is woken when the operation's own task finishes;
    /// a panic from it is logged and does not affect the operation. See
    /// "Caller wakers" in docs/agent_execution/lifecycle.md.
    pub async fn enqueue_steering(
        &self,
        input: ExecutionRequest,
        submitter: Submitter,
    ) -> Result<QueueAdmission, AgentError> {
        self.enqueue_kind(
            input,
            submitter,
            InvocationKind::Steering,
            NoteCandidates::default(),
        )
        .await
    }

    /// Copy pending IDs in dispatch order under the scheduler's admission lock.
    /// Running/injected/removed inputs are excluded; the copy grants no authority.
    ///
    /// Waiting for the scheduler lock registers the polling task's `Waker`
    /// with it. A panic from that waker when another task releases the lock
    /// is logged and does not fail that task. See "Caller wakers" in
    /// docs/agent_execution/lifecycle.md.
    pub async fn queued_ids(&self) -> Vec<ExecutionId> {
        let waiter = CallerWaiter::QueuedIds(self.inner.manager.id().clone());
        contain_caller_wake(waiter, async {
            self.inner
                .scheduler
                .lock()
                .await
                .queue
                .pending()
                .into_iter()
                .map(|(id, _)| id)
                .collect()
        })
        .await
    }
    /// Atomically replace the complete pending order requested by verified `actor`.
    /// Steering retains priority; identities, requests, receipts and kinds remain
    /// unchanged. Membership races, including dispatch or withdrawal while the
    /// mandatory audit acknowledgement is pending, return QueueChanged;
    /// duplicate/oversized input returns InvalidInput. Applying and saving the
    /// validated order remain serialized with dispatch and withdrawal.
    /// Once polled, caller loss does not cancel this operation. Changing the live
    /// order and retaining its evidence is one transition: either both happen or
    /// neither does. Storage failure may leave that applied order observed but
    /// unwritten; it is saved before later dispatch, and an unchanged retry
    /// retries that persistence. At most 1024 actual order changes are retained
    /// per session; the budget is checked before audit and mutation. Audit
    /// acknowledgement is bounded to 30 seconds. Retention and its persistence
    /// share one further 30-second budget, so the admission lock this operation
    /// holds against dispatch and withdrawal is never blocked for longer than
    /// that. Close interrupts any of those waits: before retention it reports
    /// Closed and changes nothing, while interruption after retention may leave
    /// an audited live order that close then drains. This does not replay pending
    /// work after restart.
    ///
    /// # Examples
    /// ```
    /// use nessa_sdk::{Agent, application::agent_execution::{
    ///     agents::{AgentError, QueueReorder}, permissions::ActionContext,
    /// }, domain::agent_execution::executions::ExecutionId};
    /// # async fn reorder(agent: &Agent, desired: Vec<ExecutionId>, actor: ActionContext) -> Result<(), AgentError> {
    /// match agent.reorder_queued(desired, actor).await? {
    ///     QueueReorder::Applied | QueueReorder::Unchanged => {},
    ///     QueueReorder::QueueChanged | QueueReorder::PriorityConflict => {
    ///         let current = agent.queued_ids().await;
    ///         // Present current order and let the caller choose a fresh command.
    ///         # let _ = current;
    ///     }
    /// }
    /// # Ok(()) }
    /// ```
    ///
    /// The caller's `Waker` is woken when the operation's own task finishes;
    /// a panic from it is logged and does not affect the operation. See
    /// "Caller wakers" in docs/agent_execution/lifecycle.md.
    pub async fn reorder_queued(
        &self,
        order: Vec<ExecutionId>,
        actor: ActionContext,
    ) -> Result<QueueReorder, AgentError> {
        let agent = self.clone();
        let waiter = CallerWaiter::QueueReorder(self.inner.manager.id().clone());
        contain_caller_wake(
            waiter,
            tokio::spawn(async move {
                // Reorders serialize with each other while the scheduler remains
                // available to dispatch, cancellation, and close during audit I/O.
                let _reorder = agent.inner.reorder.lock().await;
                let change = match QueueOrderChange::new(
                    agent.inner.scheduler.lock().await.queue.pending(),
                    order.clone(),
                ) {
                    Ok(change) => change,
                    Err(QueueOrderError::QueueChanged) => return Ok(QueueReorder::QueueChanged),
                    Err(QueueOrderError::PriorityConflict) => {
                        return Ok(QueueReorder::PriorityConflict)
                    }
                    Err(error) => {
                        return Err(AgentError::InvalidInput(format!(
                            "invalid queue order: {error:?}"
                        )))
                    }
                };
                let unchanged = change.is_unchanged();
                if !unchanged {
                    agent
                        .inner
                        .manager
                        .check_queue_reorder_capacity()
                        .await
                        .map_err(AgentError::Storage)?;
                }
                let notice = agent.inner.lifecycle.close_notice();
                if !unchanged {
                    let mut close = notice.clone();
                    let audit = agent
                        .catch_scheduling_panic(async {
                            tokio::select! { biased;
                                _ = close.changed() => Err(AuditObservationFailure::Unacknowledged(AgentError::Closed)),
                                result = agent.record_scheduling_audit(
                                    ExecutionAuditRecord::QueueReordered(
                                        QueueOrderRecord::caller_requested(
                                            agent.inner.manager.id().clone(),
                                            change.clone(),
                                            actor.clone(),
                                        ),
                                    ),
                                    "queue_reorder",
                                ) => result,
                            }
                        })
                        .await;
                    match audit {
                        Ok(Ok(())) => {}
                        Ok(Err(AuditObservationFailure::Unacknowledged(error))) => return Err(error),
                        Ok(Err(AuditObservationFailure::Panicked)) | Err(()) => {
                            agent.recover_scheduling_panic(None, &notice).await?;
                            return Err(AgentError::SubmissionUnresolved);
                        }
                    }
                    let mut scheduler = agent.inner.scheduler.lock().await;
                    // One budget for the whole phase that holds the scheduler, so
                    // retention and its persistence cannot compose into a longer
                    // block against dispatch, withdrawal, or teardown.
                    let deadline = Instant::now() + AUDIT_OBSERVATION_TIMEOUT;
                    let current = match QueueOrderChange::new(scheduler.queue.pending(), order) {
                        Ok(current) => current,
                        Err(QueueOrderError::PriorityConflict) => {
                            return Ok(QueueReorder::PriorityConflict)
                        }
                        Err(QueueOrderError::QueueChanged) => {
                            return Ok(QueueReorder::QueueChanged)
                        }
                        Err(error) => {
                            return Err(AgentError::InvalidInput(format!(
                                "invalid queue order: {error:?}"
                            )))
                        }
                    };
                    if current != change {
                        return Ok(QueueReorder::QueueChanged);
                    }
                    // Changing the live order and retaining that change are one
                    // transition. Waiting for evidence ownership happens before
                    // either effect, so an interruption here leaves the queue and
                    // its history agreeing on the order dispatch will replay.
                    let retained = agent
                    .catch_scheduling_panic(async {
                        let mut close = notice.clone();
                        tokio::select! { biased;
                            // Nothing has changed yet, so close is teardown here
                            // rather than an interrupted evidence write.
                            _ = close.changed() => Err(AgentError::Closed),
                            result = agent.inner.manager.retain_queue_reorder(
                                change.clone(),
                                actor.clone(),
                                || {
                                    scheduler.queue.apply_order(&change).map_err(|error| {
                                        StorageError::Corrupt(format!(
                                            "exclusive validated queue order: {error:?}"
                                        ))
                                    })
                                },
                            ) => result.map_err(AgentError::Storage),
                            _ = sleep_until(deadline) => Err(AgentError::Storage(
                                StorageError::Io(
                                    "queue reorder retention timed out waiting for session evidence"
                                        .into(),
                                ),
                            )),
                        }
                    })
                    .await;
                    match retained {
                        Ok(result) => result?,
                        Err(()) => {
                            drop(scheduler);
                            agent.recover_scheduling_panic(None, &notice).await?;
                            return Err(AgentError::SubmissionUnresolved);
                        }
                    }
                    let saved = agent
                        .catch_scheduling_panic(async {
                            let mut close = notice.clone();
                            tokio::select! { biased;
                                _ = close.changed() => Err(StorageError::Io(
                                    "queue reorder persistence interrupted by close".into(),
                                )),
                                result = agent.inner.manager.flush_observed() => result,
                                _ = sleep_until(deadline) => Err(StorageError::Io(
                                    "queue reorder persistence timed out".into(),
                                )),
                            }
                        })
                        .await;
                    return match saved {
                        Ok(result) => {
                            result.map_err(AgentError::Storage)?;
                            Ok(QueueReorder::Applied)
                        }
                        Err(()) => {
                            drop(scheduler);
                            agent.recover_scheduling_panic(None, &notice).await?;
                            Err(AgentError::SubmissionUnresolved)
                        }
                    };
                }
                let scheduler = agent.inner.scheduler.lock().await;
                let saved = agent
                    .catch_scheduling_panic(async {
                        let mut close = notice.clone();
                        tokio::select! { biased;
                            _ = close.changed() => Err(StorageError::Io(
                                "queue reorder persistence interrupted by close".into(),
                            )),
                            result = agent.inner.manager.flush_observed() => result,
                            _ = sleep(AUDIT_OBSERVATION_TIMEOUT) => Err(StorageError::Io(
                                "queue reorder persistence timed out".into(),
                            )),
                        }
                    })
                    .await;
                match saved {
                    Ok(result) => {
                        result.map_err(AgentError::Storage)?;
                        Ok(QueueReorder::Unchanged)
                    }
                    Err(()) => {
                        drop(scheduler);
                        agent.recover_scheduling_panic(None, &notice).await?;
                        Err(AgentError::SubmissionUnresolved)
                    }
                }
            }),
        )
        .await
        .map_err(|_| AgentError::SubmissionUnresolved)?
    }

    /// Removes pending `id`, including boundary steering, using host-verified
    /// `actor` attribution. Dispatch and removal serialize: only one can win.
    /// The input and Withdrawn cause stay in history; this does not delete evidence.
    /// Already injected/dispatched inputs return NotPending and cannot be unsent.
    /// Storage failure still removes the input and reports the failed audit write.
    /// Once polled, removal is supervised even if the caller stops waiting.
    ///
    /// The caller's `Waker` is woken when the operation's own task finishes;
    /// a panic from it is logged and does not affect the operation. See
    /// "Caller wakers" in docs/agent_execution/lifecycle.md.
    pub async fn remove_queued(
        &self,
        id: ExecutionId,
        actor: ActionContext,
    ) -> Result<QueueRemoval, AgentError> {
        let agent = self.clone();
        let waiter = CallerWaiter::QueueRemoval(id.clone());
        contain_caller_wake(
            waiter,
            tokio::spawn(async move {
                let mut scheduler = agent.inner.scheduler.lock().await;
                let Some(_) = scheduler.queue.remove(&id) else {
                    return Ok(QueueRemoval::NotPending);
                };
                let pending = scheduler.pending.remove(&id).expect("admitted queue input");
                let close_notice = agent.inner.lifecycle.close_notice();
                // The removed input still owns its receipt and work permit while
                // persistence runs. A save panic must not drop that ownership.
                let saved = agent
                    .catch_scheduling_panic(async {
                        let saved = agent
                            .inner
                            .manager
                            .record_scheduling_with_queue(
                                pending.index,
                                event(
                                    pending.kind,
                                    pending.target.clone(),
                                    Some(InvocationStage::Queued),
                                    InvocationStage::Cancelled,
                                    SchedulingCause::Withdrawn,
                                    Some(actor),
                                ),
                                Some(QueueMutation::Removed {
                                    id: id.clone(),
                                    cause: QueueRemovalCause::Withdrawn,
                                }),
                            )
                            .await;
                        match saved {
                            Ok(()) => None,
                            Err(error) => Some(
                                agent
                                    .inner
                                    .manager
                                    .settle_submission(
                                        pending.index,
                                        Err(AgentError::Storage(error)),
                                    )
                                    .await
                                    .expect_err("failed withdrawal evidence"),
                            ),
                        }
                    })
                    .await;
                let failure = match saved {
                    Ok(failure) => failure,
                    Err(()) => {
                        // Recovery also owns other queued inputs. Release this lock
                        // before joining cleanup and retaining the withdrawal error.
                        drop(scheduler);
                        Some(
                            agent
                                .recover_scheduling_panic(Some(&id), &close_notice)
                                .await
                                .expect_err("withdrawal save panicked"),
                        )
                    }
                };
                pending.publish(Err(failure.clone().unwrap_or(AgentError::Closed)));
                if let Some(error) = failure {
                    return Err(error);
                }
                Ok(QueueRemoval::Removed)
            }),
        )
        .await
        .map_err(|_| AgentError::Closed)?
    }

    async fn enqueue_kind(
        &self,
        input: ExecutionRequest,
        submitter: Submitter,
        kind: InvocationKind,
        candidates: NoteCandidates,
    ) -> Result<QueueAdmission, AgentError> {
        let agent = self.clone();
        let waiter = CallerWaiter::Admission(input.execution_id.clone());
        contain_caller_wake(
            waiter,
            tokio::spawn(
                async move { agent.accept_work(input, submitter, kind, candidates).await },
            ),
        )
        .await
        .map_err(|_| AgentError::SubmissionUnresolved)?
    }

    pub(super) async fn accept_work(
        &self,
        input: ExecutionRequest,
        submitter: Submitter,
        kind: InvocationKind,
        candidates: NoteCandidates,
    ) -> Result<QueueAdmission, AgentError> {
        validate_configured_input(self.capabilities(), &input)?;
        let mut scheduler = self.inner.scheduler.lock().await;
        let mode = match kind {
            InvocationKind::Queued => SubmissionMode::Queued,
            InvocationKind::Steering => SubmissionMode::BoundarySteering,
        };
        if let Some(recovered) = submissions::recover(
            &self.inner.manager,
            &scheduler.receipts,
            &input,
            &submitter,
            mode,
        )
        .await
        {
            return match recovered? {
                SteeringDelivery::Queued(receipt) => Ok(receipt),
                SteeringDelivery::Injected { .. } => Err(AgentError::SubmissionConflict),
            };
        }
        let work = self.inner.lifecycle.accept_waiting_work()?;
        // Captured with the permit. Close detaches the attachment before it
        // takes this scheduler lock, so a later read would name the binding
        // preset for a generation that was admitted under the live one.
        let approval_mode = self.approval_mode_on(work.footing());
        submitter.admits(approval_mode)?;
        scheduler
            .queue
            .validate_enqueue(&input.execution_id)
            .map_err(AgentError::Scheduling)?;
        // Asked under the scheduler lock that admits it: a queued turn that
        // starts from an idle agent takes notes, and one behind other work
        // does not, so nothing it takes waits behind a turn that never saw it.
        let takes_notes = kind == InvocationKind::Queued
            && scheduler.is_idle()
            && self.inner.lifecycle.active().is_none();
        let index = self
            .inner
            .manager
            .begin_with_scheduling(
                input.clone(),
                submitter.clone(),
                event(
                    kind,
                    None,
                    None,
                    InvocationStage::Queued,
                    SchedulingCause::Submitted,
                    Some(submitter.context.clone()),
                ),
                mode,
                NoteIntake {
                    candidates,
                    takes_notes,
                },
            )
            .await?;
        // The scheduler lock spans validation and persistence, so no admission
        // can invalidate this check while the evidence is saved.
        scheduler
            .queue
            .enqueue(input.execution_id.clone(), kind)
            .expect("admission checked under scheduler lock");
        let id = input.execution_id.clone();
        let audit_actor = submitter.context.clone();
        let admission_generation =
            format!("{}:{}", self.inner.instance_id, work.provider_generation());
        let effort_level = self.effort_level();
        let receipt =
            Self::accept_pending(&mut scheduler, input, submitter, index, kind, None, work);
        let audit_record = ExecutionAuditRecord::QueueAdmitted(
            QueueAdmissionRecord::submitted(
                self.inner.manager.id().clone(),
                id.clone(),
                mode,
                audit_actor.clone(),
            )
            .with_approval_context(approval_mode, admission_generation)
            .with_effort_level(effort_level),
        );
        let audit = self
            .record_scheduling_audit(audit_record, "queue_admission")
            .await
            .err()
            .map(AuditObservationFailure::into_agent_error);
        let saved = self
            .catch_scheduling_panic(self.inner.manager.record_queue_admission(
                id.clone(),
                kind,
                audit_actor,
            ))
            .await;
        let storage = match saved {
            Ok(saved) => saved.err(),
            Err(()) => Some(StorageError::Io(
                "queue admission evidence persistence panicked".into(),
            )),
        };
        let failure = self
            .persist_submission_acknowledgement(
                index,
                audit,
                storage,
                "queue admission acknowledgement persistence panicked",
            )
            .await;
        let acknowledged = failure.is_none();
        let evidence = failure.map_or(AdmissionEvidence::Acknowledged, AdmissionEvidence::Failed);
        let mut admission = QueueAdmission::new(receipt, evidence);
        if let Some(SubmissionReceipt::Queued {
            evidence: retained, ..
        }) = scheduler.receipts.get_mut(&id)
        {
            *retained = admission.evidence.clone();
        }
        if acknowledged {
            self.start_runner(&mut scheduler);
            return Ok(admission);
        }
        let attempt = self.start_shutdown(SessionCloseRequest::ExecutionFailed);
        if let Err(error) = self
            .cancel_pending(&mut scheduler, SchedulingCause::RunnerStopped, None)
            .await
        {
            match error {
                AgentError::Storage(error) => admission.retain_storage_failure(error),
                error => tracing::warn!(?error, "queue cancellation evidence failed"),
            }
        }
        if let Some(SubmissionReceipt::Queued {
            evidence: retained, ..
        }) = scheduler.receipts.get_mut(&id)
        {
            *retained = admission.evidence.clone();
        }
        drop(scheduler);
        let agent = self.clone();
        tokio::spawn(async move {
            let _ = agent.inner.lifecycle.complete_stop(&attempt).await;
        });
        Ok(admission)
    }

    fn accept_pending(
        scheduler: &mut Scheduler,
        input: ExecutionRequest,
        submitter: Submitter,
        index: usize,
        kind: InvocationKind,
        target: Option<ExecutionId>,
        work: WorkPermit,
    ) -> QueuedInvocation {
        let (reply, result) = watch::channel(None);
        let id = input.execution_id.clone();
        scheduler.receipts.insert(
            id.clone(),
            SubmissionReceipt::Queued {
                result: result.clone(),
                evidence: AdmissionEvidence::Acknowledged,
            },
        );
        scheduler.pending.insert(
            id.clone(),
            Pending {
                input,
                submitter,
                index,
                kind,
                target,
                reply,
                _work: work,
            },
        );
        QueuedInvocation { id, result }
    }
    pub(super) fn start_runner(&self, scheduler: &mut Scheduler) {
        if !scheduler.running {
            scheduler.running = true;
            let agent = self.clone();
            tokio::spawn(async move {
                agent.run_queue().await;
            });
        }
    }

    async fn run_queue(&self) {
        loop {
            // A runner with nothing to select or settle exits without the
            // invocation slot: holding it would turn a direct `invoke` into
            // Busy with nothing to overlap. This check, `running` and admission
            // share the scheduler lock, so an admission after it starts a new
            // runner. The orderings and their tests are in
            // docs/agent_execution/scheduling.md (#366).
            {
                let mut scheduler = self.inner.scheduler.lock().await;
                if scheduler.has_no_work() {
                    scheduler.running = false;
                    return;
                }
            }
            // Wait for a direct invocation without removing pending work: close
            // can still cancel every waiting item and prevent automatic restart.
            let active = self.inner.invocation.lock().await;
            let (pending, close_notice, selection) = {
                let mut scheduler = self.inner.scheduler.lock().await;
                let close_notice = self.inner.lifecycle.close_notice();
                // No invocation was selected. A stopped waiter's receipt must
                // expose its lifecycle refusal, not this runner's empty slot.
                if self.inner.lifecycle.is_closed()
                    || scheduler
                        .pending
                        .values()
                        .any(|pending| pending._work.cancellation().is_some())
                {
                    drop(active);
                    if let Err(error) = self.settle_stopped_pending(&mut scheduler).await {
                        tracing::warn!(?error, "failed to save stopped queue receipts");
                    }
                    if self.inner.lifecycle.is_closed() {
                        // A stop can race the first drain while it awaits storage.
                        if let Err(error) = self.settle_stopped_pending(&mut scheduler).await {
                            tracing::warn!(?error, "failed to save stopped queue receipts");
                        }
                        scheduler.running = false;
                        return;
                    }
                    continue;
                }
                let Some((next, _)) = scheduler.queue.pending().first().cloned() else {
                    scheduler.running = false;
                    return;
                };
                if self.inner.lifecycle.attachment_status().phase() != AttachmentPhase::Attached {
                    let recovery =
                        match self.authorize_attachment(AttachmentRequest::AutomaticRecovery) {
                            Ok(recovery) => recovery,
                            Err(error) => {
                                tracing::debug!(
                                    ?error,
                                    "queued work has no authorized automatic recovery"
                                );
                                scheduler.running = false;
                                return;
                            }
                        };
                    drop(scheduler);
                    // Startup owns its attachment independently and can publish
                    // failed waiting receipts before this await completes.
                    drop(active);
                    let recovered = match self.start_attachment(recovery) {
                        Ok(wait) => wait.wait().await,
                        Err(error) => Err(error),
                    };
                    if let Err(error) = recovered {
                        let mut scheduler = self.inner.scheduler.lock().await;
                        if let Err(settlement_error) = self
                            .settle_failed_pending(&mut scheduler, error.clone())
                            .await
                        {
                            tracing::warn!(
                                ?settlement_error,
                                ?error,
                                "failed to settle queued work after automatic recovery failed"
                            );
                        }
                        scheduler.running = false;
                        return;
                    }
                    continue;
                }
                if let Err(error) = scheduler
                    .pending
                    .get(&next)
                    .expect("queued input has pending owner")
                    ._work
                    .activate()
                {
                    tracing::debug!(?error, "queue activation fenced before selection");
                    scheduler.running = false;
                    return;
                }
                let Some((id, _)) = scheduler.queue.pop_next() else {
                    scheduler.running = false;
                    return;
                };
                let pending = scheduler.pending.remove(&id).expect("admitted queue input");
                // Selected means local dequeue, not provider execution. Save the
                // queue fact before any reorder can observe the remaining queue.
                let selection = self
                    .catch_scheduling_panic(self.inner.manager.record_queue_selection(id))
                    .await;
                (pending, close_notice, selection)
            };
            let selection = match selection {
                Ok(result) => result,
                Err(()) => {
                    let result = self
                        .recover_scheduling_panic(Some(&pending.input.execution_id), &close_notice)
                        .await;
                    self.inner.scheduler.lock().await.running = false;
                    drop(active);
                    pending.publish(result);
                    return;
                }
            };
            let result = match self
                .catch_scheduling_panic(self.run_pending(&pending, selection))
                .await
            {
                Ok(result) => result,
                Err(()) => {
                    let result = self
                        .recover_scheduling_panic(Some(&pending.input.execution_id), &close_notice)
                        .await;
                    self.inner.scheduler.lock().await.running = false;
                    drop(active);
                    pending.publish(result);
                    return;
                }
            };
            drop(active);
            pending.publish(result);
        }
    }

    async fn run_pending(
        &self,
        pending: &Pending,
        selection: Result<(), StorageError>,
    ) -> Result<ExecutionOutcome, AgentError> {
        let dispatch = self
            .inner
            .manager
            .record_scheduling(
                pending.index,
                event(
                    pending.kind,
                    pending.target.clone(),
                    Some(InvocationStage::Queued),
                    InvocationStage::Running,
                    SchedulingCause::Dispatched,
                    None,
                ),
            )
            .await;
        let dispatch = selection.and(dispatch);
        let dispatch_failed = dispatch.is_err();
        let result = match dispatch {
            Ok(()) => {
                self.supervise_invocation(
                    pending.input.clone(),
                    pending.submitter.clone(),
                    Some(pending.index),
                    &pending._work,
                )
                .await
            }
            Err(error) => Err(AgentError::Storage(error)),
        };
        let cancellation = if !pending._work.execution_started()
            || self.inner.manager.invocation_cancelled(pending.index).await
            || matches!(&result, Err(AgentError::Closed))
        {
            pending._work.cancellation()
        } else {
            None
        };
        // Preserve failure settlement even if dispatch never reached a provider.
        let finish_failure = self
            .inner
            .manager
            .finish(pending.index, result.clone())
            .await
            .err();
        // Domain history chooses success/failed completion from known facts,
        // including provider outcomes preserved beneath later local failures.
        let cause = if let Some(cancellation) = &cancellation {
            Ok(cancellation.cause)
        } else if dispatch_failed {
            Ok(SchedulingCause::DispatchFailed)
        } else {
            self.inner
                .manager
                .scheduling_settlement_cause(pending.index)
                .await
        };
        let settlement = match cause {
            Ok(cause) => {
                self.inner
                    .manager
                    .record_scheduling(
                        pending.index,
                        event(
                            pending.kind,
                            pending.target.clone(),
                            Some(InvocationStage::Running),
                            if cancellation.is_some() {
                                InvocationStage::Cancelled
                            } else {
                                InvocationStage::Settled
                            },
                            cause,
                            cancellation.as_ref().and_then(|event| event.actor.clone()),
                        ),
                    )
                    .await
            }
            Err(error) => Err(error),
        };
        let result = match finish_failure.or_else(|| settlement.err()) {
            None => result,
            Some(error) => Err(AgentError::StorageAfterExecution {
                error,
                execution_result: Box::new(result),
            }),
        };
        let mut result = result;
        if result.is_err() {
            // Never blindly dispatch further work after a potentially
            // uncertain provider or storage failure. Preserve every cause.
            let mut scheduler = self.inner.scheduler.lock().await;
            if let Err(error) = self
                .cancel_pending(
                    &mut scheduler,
                    cancellation
                        .as_ref()
                        .map_or(SchedulingCause::RunnerStopped, |event| event.cause),
                    cancellation.as_ref().and_then(|event| event.actor.clone()),
                )
                .await
            {
                result = Err(match error {
                    AgentError::Storage(error) => AgentError::StorageAfterExecution {
                        error,
                        execution_result: Box::new(result),
                    },
                    error => AgentError::MultipleOperationFailures {
                        first_error: Box::new(result.expect_err("failed queued invocation")),
                        subsequent_error: Box::new(error),
                    },
                });
            }
        }
        let result = self
            .inner
            .manager
            .settle_submission(pending.index, result)
            .await;
        result
    }

    // Keep ownership outside the polled future. Latch admission before dropping
    // any state captured by the failed operation, then recover outside its locks.
    async fn catch_scheduling_panic<T>(&self, operation: impl Future<Output = T>) -> Result<T, ()> {
        let mut operation = Box::pin(operation);
        let result = poll_fn(|context| {
            match catch_unwind(AssertUnwindSafe(|| operation.as_mut().poll(context))) {
                Ok(Poll::Ready(value)) => Poll::Ready(Ok(value)),
                Ok(Poll::Pending) => Poll::Pending,
                Err(_) => {
                    self.stop_control_admission();
                    Poll::Ready(Err(()))
                }
            }
        })
        .await;
        if catch_unwind(AssertUnwindSafe(|| drop(operation))).is_err() {
            self.stop_control_admission();
            return Err(());
        }
        result
    }

    pub(super) async fn record_resumption_audit(
        &self,
        record: ExecutionAuditRecord,
    ) -> Result<(), AgentError> {
        self.record_scheduling_audit(record, "turn_resumption")
            .await
            .map_err(AuditObservationFailure::into_agent_error)
    }

    // Only observe the sink here. The adapter owns physical I/O it has started,
    // including after this bounded observer is dropped; do not grow a detached
    // task queue to keep arbitrary pending sink futures alive.
    async fn record_scheduling_audit(
        &self,
        record: ExecutionAuditRecord,
        operation: &'static str,
    ) -> Result<(), AuditObservationFailure> {
        self.inner
            .audit
            .record(record, operation, || self.stop_control_admission())
            .await
    }

    async fn recover_scheduling_panic(
        &self,
        id: Option<&ExecutionId>,
        close_notice: &watch::Receiver<Option<ActionContext>>,
    ) -> Result<ExecutionOutcome, AgentError> {
        let attempt = self.start_shutdown(SessionCloseRequest::ExecutionFailed);
        let actor = if close_notice.has_changed().unwrap_or(true) {
            close_notice.borrow().clone()
        } else {
            None
        };
        let cause = if actor.is_some() {
            self.inner.lifecycle.caller_close_cause()
        } else {
            SchedulingCause::RunnerStopped
        };
        let cleanup = self.inner.lifecycle.complete_stop(&attempt).await;
        let error = AgentError::Protocol("scheduling task panicked".into());
        let error = match cleanup.into_result() {
            Ok(_) => error,
            Err(cleanup_error) => AgentError::OperationAndCleanupFailure {
                operation_error: Box::new(error),
                cleanup_error: Box::new(cleanup_error),
            }
            .bounded(),
        };
        let metadata = match id {
            Some(id) => self.inner.manager.scheduling_metadata(id).await,
            None => None,
        };
        let mut result = match id {
            Some(id) => {
                self.inner
                    .manager
                    .retain_invocation_failure(id, error)
                    .await
            }
            None => Err(error),
        };
        if let Some((index, last)) = &metadata {
            if matches!(
                last.stage,
                InvocationStage::Queued | InvocationStage::Running
            ) {
                let cause = if last.stage == InvocationStage::Queued {
                    Ok(SchedulingCause::DispatchFailed)
                } else {
                    self.inner.manager.scheduling_settlement_cause(*index).await
                };
                let saved = match cause {
                    Ok(cause) => {
                        self.inner
                            .manager
                            .record_scheduling(
                                *index,
                                event(
                                    last.kind,
                                    last.target.clone(),
                                    Some(last.stage),
                                    InvocationStage::Settled,
                                    cause,
                                    None,
                                ),
                            )
                            .await
                    }
                    Err(error) => Err(error),
                };
                if let Err(error) = saved {
                    result = Err(AgentError::StorageAfterExecution {
                        error,
                        execution_result: Box::new(result),
                    }
                    .bounded());
                }
            }
        }
        let mut scheduler = self.inner.scheduler.lock().await;
        if let Err(error) = self.cancel_pending(&mut scheduler, cause, actor).await {
            result = Err(match error {
                AgentError::Storage(error) => AgentError::StorageAfterExecution {
                    error,
                    execution_result: Box::new(result),
                },
                error => AgentError::MultipleOperationFailures {
                    first_error: Box::new(result.expect_err("scheduling recovery failure")),
                    subsequent_error: Box::new(error),
                },
            }
            .bounded());
        }
        drop(scheduler);
        if let Some((index, _)) = metadata {
            result = self.inner.manager.settle_submission(index, result).await;
        }
        result
    }

    /// Attempts native steering of the active execution using `input` and verified
    /// `submitter`. Idle inputs run at the next invocation boundary, and so does
    /// input whose submitter's initiator is not the one the active turn was saved
    /// with: a turn is never steered by anyone but who started it. Unsupported native
    /// steering returns Unsupported; callers can explicitly choose enqueue_steering.
    ///
    /// A message the selected model cannot take is refused before anything is
    /// saved or steered. Input that passes that check is saved before
    /// contacting the provider. If its target settles during that save, the
    /// undispatched input enters the boundary queue. Once native
    /// delivery starts, only an explicit PromptRequired response allows queuing it
    /// as a new invocation. Timeouts, malformed replies and transport errors are
    /// never retried automatically.
    /// When `submitter` carries [`Submitter::with_after_save`](crate::application::agent_execution::executions::Submitter::with_after_save),
    /// that check runs after the input is saved and before this injects into
    /// the provider or queues the turn. A refusal settles the saved attempt
    /// and leaves the provider uncalled. A permit the check keeps is released
    /// when the provider operation starts, before this waits for the
    /// provider's response, and when this queues or refuses instead.
    /// Dropping the caller's wait does not abandon accepted steering delivery.
    /// Identical retries recover the original injection acknowledgement, queued
    /// receipt, or error; they never inject a second time. Conflicting retries return
    /// SubmissionConflict; unresolved restored delivery returns SubmissionUnresolved.
    ///
    /// The caller's `Waker` is woken when the operation's own task finishes;
    /// a panic from it is logged and does not affect the operation. See
    /// "Caller wakers" in docs/agent_execution/lifecycle.md.
    pub async fn steer(
        &self,
        input: ExecutionRequest,
        submitter: Submitter,
    ) -> Result<SteeringDelivery, AgentError> {
        let agent = self.clone();
        let waiter = CallerWaiter::Steering(input.execution_id.clone());
        contain_caller_wake(
            waiter,
            tokio::spawn(async move {
                let id = input.execution_id.clone();
                let close_notice = agent.inner.lifecycle.close_notice();
                let mut work_owner = None;
                match agent
                    .catch_scheduling_panic(agent.deliver_steering(
                        input,
                        submitter,
                        &mut work_owner,
                    ))
                    .await
                {
                    Ok(delivery) => delivery,
                    Err(()) => {
                        let error = agent
                            .recover_scheduling_panic(Some(&id), &close_notice)
                            .await
                            .expect_err("panic recovery retains failure");
                        if agent.inner.manager.scheduling_metadata(&id).await.is_some() {
                            agent
                                .inner
                                .scheduler
                                .lock()
                                .await
                                .receipts
                                .insert(id, SubmissionReceipt::Steering(Err(error.clone())));
                        }
                        Err(error)
                    }
                }
            }),
        )
        .await
        .map_err(|_| AgentError::Closed)?
    }

    async fn deliver_steering(
        &self,
        input: ExecutionRequest,
        submitter: Submitter,
        work_owner: &mut Option<WorkPermit>,
    ) -> Result<SteeringDelivery, AgentError> {
        // The same check `invoke` and `accept_work` run before a save. A
        // message the selected model cannot take is refused here, with
        // nothing written and nothing handed to the provider.
        validate_configured_input(self.capabilities(), &input)?;
        let mut scheduler = self.inner.scheduler.lock().await;
        if let Some(recovered) = submissions::recover(
            &self.inner.manager,
            &scheduler.receipts,
            &input,
            &submitter,
            SubmissionMode::Steering,
        )
        .await
        {
            return recovered;
        }
        let close_notice = self.inner.lifecycle.close_notice();
        *work_owner = Some(self.inner.lifecycle.accept_waiting_work()?);
        let approval_mode = self.approval_mode_on(
            work_owner
                .as_ref()
                .expect("admitted steering owner")
                .footing(),
        );
        submitter.admits(approval_mode)?;
        scheduler
            .queue
            .validate_enqueue(&input.execution_id)
            .map_err(AgentError::Scheduling)?;
        let target = self.inner.lifecycle.active();
        // A turn's calls are judged under the initiation its record saved, so
        // input from anyone else is never injected into it: it waits for a
        // turn of its own. A target whose record cannot be read is not joined.
        let joins_target = match &target {
            Some(target) => self
                .inner
                .manager
                .initiation_of(target)
                .await
                .is_some_and(|saved| saved.initiator() == submitter.initiation.initiator()),
            None => true,
        };
        // Cloned before the save consumes its own copy. The host's check
        // stays alive across that save and is asked once it has committed,
        // before this task waits on the provider.
        let after_save = submitter.after_save.clone();
        let index = self
            .inner
            .manager
            .begin_with_scheduling(
                input.clone(),
                submitter.clone(),
                event(
                    InvocationKind::Steering,
                    target.clone(),
                    None,
                    InvocationStage::Queued,
                    SchedulingCause::Submitted,
                    Some(submitter.context.clone()),
                ),
                SubmissionMode::Steering,
                NoteIntake::default(),
            )
            .await?;
        let refused = match after_save.as_ref() {
            Some(check) => match check.after_save().await {
                Ok(()) => None,
                Err(AfterSaveRefusal::Refused) => Some(AgentError::RefusedAfterSave),
                Err(AfterSaveRefusal::Unavailable) => Some(AgentError::AfterSaveUnavailable),
            },
            None => None,
        };
        let release_saved = |check: Option<&Arc<dyn AfterSaveCheck>>| {
            if let Some(check) = check {
                check.provider_operation_started();
            }
        };
        // WorkStatus persistence can be overtaken by close or provider settlement.
        // Revalidate the captured target before handing anything to the adapter.
        let mut activated = false;
        let outcome = if let Some(error) = refused {
            release_saved(after_save.as_ref());
            Err(error)
        } else if !self.inner.lifecycle.accepts_queued()
            || close_notice.has_changed().unwrap_or(true)
        {
            release_saved(after_save.as_ref());
            Err(AgentError::Closed)
        } else if self.inner.lifecycle.active().as_ref() != target.as_ref() || !joins_target {
            release_saved(after_save.as_ref());
            Ok(SteeringOutcome::PromptRequired)
        } else {
            match &target {
                Some(target) => {
                    if let Err(error) = work_owner.as_ref().expect("steering owner").activate() {
                        release_saved(after_save.as_ref());
                        return Err(error);
                    }
                    activated = true;
                    self.deliver_native_steering(target.clone(), input.clone(), after_save.as_ref())
                        .await
                }
                None => {
                    release_saved(after_save.as_ref());
                    Ok(SteeringOutcome::PromptRequired)
                }
            }
        };
        let outcome = match outcome {
            Ok(SteeringOutcome::PromptRequired) if activated => work_owner
                .as_ref()
                .expect("steering owner")
                .defer()
                .map(|()| SteeringOutcome::PromptRequired),
            outcome => outcome,
        };
        let id = input.execution_id.clone();
        let delivery = match outcome {
            Ok(SteeringOutcome::Injected) => {
                let target = target.expect("active injection target");
                let audit_record = ExecutionAuditRecord::SteeringAcknowledged(
                    SteeringAcknowledgementRecord::provider_acknowledged(
                        self.inner.manager.id().clone(),
                        id.clone(),
                        target.clone(),
                        submitter.context.clone(),
                    ),
                );
                let audit = self
                    .record_scheduling_audit(audit_record, "steering_acknowledgement")
                    .await
                    .err()
                    .map(AuditObservationFailure::into_agent_error);
                let saved = self
                    .catch_scheduling_panic(self.inner.manager.record_scheduling(
                        index,
                        event(
                            InvocationKind::Steering,
                            Some(target.clone()),
                            Some(InvocationStage::Queued),
                            InvocationStage::Injected,
                            SchedulingCause::SteeringInjected,
                            None,
                        ),
                    ))
                    .await;
                let storage = match saved {
                    Ok(result) => result.err(),
                    Err(()) => Some(StorageError::Io(
                        "native steering evidence persistence panicked".into(),
                    )),
                };
                let failure = self
                    .persist_submission_acknowledgement(
                        index,
                        audit,
                        storage,
                        "native steering acknowledgement persistence panicked",
                    )
                    .await;
                let evidence =
                    failure.map_or(SteeringEvidence::Acknowledged, SteeringEvidence::Failed);
                if matches!(evidence, SteeringEvidence::Failed(_)) {
                    let attempt = self.start_shutdown(SessionCloseRequest::ExecutionFailed);
                    let _ = self
                        .stop_after_steering(
                            &mut scheduler,
                            AgentError::SubmissionUnresolved,
                            &close_notice,
                        )
                        .await;
                    let agent = self.clone();
                    tokio::spawn(async move {
                        let _ = agent.inner.lifecycle.complete_stop(&attempt).await;
                    });
                }
                Ok(SteeringDelivery::Injected { target, evidence })
            }
            Ok(SteeringOutcome::PromptRequired) => {
                scheduler
                    .queue
                    .enqueue(input.execution_id.clone(), InvocationKind::Steering)
                    .expect("admission checked under scheduler lock");
                let audit_actor = submitter.context.clone();
                let admission_generation = format!(
                    "{}:{}",
                    self.inner.instance_id,
                    work_owner
                        .as_ref()
                        .expect("admitted steering owner")
                        .provider_generation()
                );
                let effort_level = self.effort_level();
                let receipt = Self::accept_pending(
                    &mut scheduler,
                    input,
                    submitter,
                    index,
                    InvocationKind::Steering,
                    target,
                    work_owner.take().expect("admitted steering owner"),
                );
                let audit_record = ExecutionAuditRecord::QueueAdmitted(
                    QueueAdmissionRecord::submitted(
                        self.inner.manager.id().clone(),
                        id.clone(),
                        SubmissionMode::Steering,
                        audit_actor.clone(),
                    )
                    .with_approval_context(approval_mode, admission_generation)
                    .with_effort_level(effort_level),
                );
                let audit = self
                    .record_scheduling_audit(audit_record, "steering_queue_admission")
                    .await
                    .err()
                    .map(AuditObservationFailure::into_agent_error);
                let saved = self
                    .catch_scheduling_panic(self.inner.manager.record_queue_admission(
                        id.clone(),
                        InvocationKind::Steering,
                        audit_actor,
                    ))
                    .await;
                let storage = match saved {
                    Ok(saved) => saved.err(),
                    Err(()) => Some(StorageError::Io(
                        "queue admission evidence persistence panicked".into(),
                    )),
                };
                let failure = self
                    .persist_submission_acknowledgement(
                        index,
                        audit,
                        storage,
                        "steering queue acknowledgement persistence panicked",
                    )
                    .await;
                let acknowledged = failure.is_none();
                let evidence =
                    failure.map_or(AdmissionEvidence::Acknowledged, AdmissionEvidence::Failed);
                let mut admission = QueueAdmission::new(receipt, evidence);
                if let Some(SubmissionReceipt::Queued {
                    evidence: retained, ..
                }) = scheduler.receipts.get_mut(&id)
                {
                    *retained = admission.evidence.clone();
                }
                if acknowledged {
                    self.start_runner(&mut scheduler);
                } else {
                    let attempt = self.start_shutdown(SessionCloseRequest::ExecutionFailed);
                    if let Err(error) = self
                        .cancel_pending(&mut scheduler, SchedulingCause::RunnerStopped, None)
                        .await
                    {
                        match error {
                            AgentError::Storage(error) => admission.retain_storage_failure(error),
                            error => {
                                tracing::warn!(
                                    ?error,
                                    "steering queue cancellation evidence failed"
                                )
                            }
                        }
                    }
                    if let Some(SubmissionReceipt::Queued {
                        evidence: retained, ..
                    }) = scheduler.receipts.get_mut(&id)
                    {
                        *retained = admission.evidence.clone();
                    }
                    let agent = self.clone();
                    tokio::spawn(async move {
                        let _ = agent.inner.lifecycle.complete_stop(&attempt).await;
                    });
                }
                Ok(SteeringDelivery::Queued(admission))
            }
            Err(mut error) => {
                // The owning generation records both automatic stops and explicit closes.
                // Capture its first cause before failure cleanup can introduce another stop.
                let cancellation = work_owner.as_ref().expect("steering owner").cancellation();
                let finish = self.inner.manager.finish(index, Err(error.clone())).await;
                let saved = self
                    .inner
                    .manager
                    .record_scheduling(
                        index,
                        event(
                            InvocationKind::Steering,
                            target,
                            Some(InvocationStage::Queued),
                            if cancellation.is_some() {
                                InvocationStage::Cancelled
                            } else {
                                InvocationStage::Settled
                            },
                            cancellation
                                .as_ref()
                                .map_or(SchedulingCause::DispatchFailed, |event| event.cause),
                            cancellation.as_ref().and_then(|event| event.actor.clone()),
                        ),
                    )
                    .await;
                let storage_failure = finish.err().or_else(|| saved.err());
                let persistence_failed = storage_failure.is_some();
                if let Some(storage) = storage_failure {
                    error = AgentError::StorageAfterExecution {
                        error: storage,
                        execution_result: Box::new(Err(error)),
                    };
                }
                if persistence_failed {
                    error = self
                        .stop_after_steering(&mut scheduler, error, &close_notice)
                        .await;
                }
                Err(error)
            }
        };
        let mut delivery = delivery;
        if let Err(error) = self.settle_stopped_pending(&mut scheduler).await {
            delivery = match (delivery, error) {
                (
                    Ok(SteeringDelivery::Injected {
                        target,
                        mut evidence,
                    }),
                    AgentError::Storage(error),
                ) => {
                    evidence.retain_storage_failure(error);
                    Ok(SteeringDelivery::Injected { target, evidence })
                }
                (Ok(SteeringDelivery::Queued(mut admission)), AgentError::Storage(error)) => {
                    admission.retain_storage_failure(error);
                    Ok(SteeringDelivery::Queued(admission))
                }
                (Err(prior), AgentError::Storage(error)) => {
                    Err(AgentError::StorageAfterExecution {
                        error,
                        execution_result: Box::new(Err(prior)),
                    })
                }
                (Ok(_), error) => Err(error),
                (Err(prior), error) => Err(AgentError::MultipleOperationFailures {
                    first_error: Box::new(prior),
                    subsequent_error: Box::new(error),
                }),
            };
        }
        let delivery = match delivery {
            Err(error) => Err(self
                .inner
                .manager
                .settle_submission(index, Err(error))
                .await
                .expect_err("failed steering receipt")),
            other => other,
        };
        match &delivery {
            Ok(SteeringDelivery::Injected { target, evidence }) => {
                scheduler.receipts.insert(
                    id,
                    SubmissionReceipt::Steering(Ok((target.clone(), evidence.clone()))),
                );
            }
            Err(error) => {
                scheduler
                    .receipts
                    .insert(id, SubmissionReceipt::Steering(Err(error.clone())));
            }
            Ok(SteeringDelivery::Queued(_)) => {}
        }
        delivery
    }

    async fn deliver_native_steering(
        &self,
        target: ExecutionId,
        input: ExecutionRequest,
        after_save: Option<&Arc<dyn AfterSaveCheck>>,
    ) -> Result<SteeringOutcome, AgentError> {
        let started = || {
            if let Some(check) = after_save {
                check.provider_operation_started();
            }
        };
        let admission = match self.accept_control() {
            Ok(admission) => admission,
            Err(error) => {
                started();
                return Err(error);
            }
        };
        let attached = match self.inner.lifecycle.attached_provider(&admission) {
            Ok(attached) => attached,
            Err(error) => {
                started();
                return Err(error);
            }
        };
        let mut cleanup_needed = false;
        let mut delivery = Box::pin(attached.session.steer(target, input, &started));
        // The future exists and has not been polled. A close that lands here
        // makes `run_control` return without polling it, so the provider never
        // calls `started`. Release on the way out, below.
        #[cfg(test)]
        if let Some((entered, release)) = self.inner.lifecycle.take_before_native_steer_poll() {
            let _ = entered.send(());
            let _ = release.await;
        }
        let outcome = self
            .run_control(
                admission.clone(),
                poll_fn(|context| {
                    match catch_unwind(AssertUnwindSafe(|| delivery.as_mut().poll(context))) {
                        Ok(Poll::Ready(Err(failure))) => {
                            cleanup_needed =
                                !matches!(failure.session_state(), ProviderSessionState::Usable);
                            Poll::Ready(Err(failure))
                        }
                        Ok(result) => result,
                        Err(_) => {
                            cleanup_needed = true;
                            // run_control holds the admission read fence while polling.
                            // Raise the barrier before that fence can accept_work another control.
                            // The save check's hold is released after this future is
                            // dropped, before cleanup waits.
                            Poll::Ready(Err(ProviderOperationFailure::new(
                                AgentError::Protocol("steering delivery panicked".into()),
                                ProviderSessionState::CleanupRequired,
                            )))
                        }
                    }
                }),
            )
            .await;
        drop(delivery);
        // Release before either return. A close before the first poll never
        // enters the provider future, and a panic before `started` never
        // reaches that callback. `complete_stop` can wait, and so can the
        // settlement after a close that did not ask for cleanup. A provider
        // that already called `started` makes this a second, empty call.
        started();
        if !cleanup_needed {
            return outcome;
        }
        // Only this operation's typed failure owns cleanup here. An unrelated
        // stop already has an owner and must not delay an interrupted response.
        // The scheduler is held, so join cleanup without acquiring it again.
        let Some(attempt) = self.inner.lifecycle.start_control_cleanup(&admission) else {
            return outcome;
        };
        let cleanup = self.inner.lifecycle.complete_stop(&attempt).await;
        match cleanup.into_result() {
            Ok(_) => outcome,
            Err(cleanup_error) => Err(AgentError::OperationAndCleanupFailure {
                operation_error: Box::new(outcome.expect_err("provider failure requires cleanup")),
                cleanup_error: Box::new(cleanup_error),
            }
            .bounded()),
        }
    }

    async fn stop_after_steering(
        &self,
        scheduler: &mut Scheduler,
        mut error: AgentError,
        close_notice: &watch::Receiver<Option<ActionContext>>,
    ) -> AgentError {
        let actor = if close_notice.has_changed().unwrap_or(true) {
            close_notice.borrow().clone()
        } else {
            None
        };
        let cause = if actor.is_some() {
            self.inner.lifecycle.caller_close_cause()
        } else {
            SchedulingCause::RunnerStopped
        };
        if let Err(settlement_error) = self.cancel_pending(scheduler, cause, actor).await {
            error = match settlement_error {
                AgentError::Storage(storage) => AgentError::StorageAfterExecution {
                    error: storage,
                    execution_result: Box::new(Err(error)),
                },
                settlement_error => AgentError::MultipleOperationFailures {
                    first_error: Box::new(error),
                    subsequent_error: Box::new(settlement_error),
                },
            };
        }
        error
    }

    pub(super) async fn cancel_pending(
        &self,
        scheduler: &mut Scheduler,
        cause: SchedulingCause,
        actor: Option<ActionContext>,
    ) -> Result<(), AgentError> {
        // Keep pending owners until their evidence and receipts settle. A first
        // storage panic can then be recovered without losing the rest of a drain.
        let mut ids: Vec<_> = scheduler
            .queue
            .drain()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let ordered: HashSet<_> = ids.iter().cloned().collect();
        for id in scheduler.pending.keys() {
            if !ordered.contains(id) {
                ids.push(id.clone());
            }
        }
        self.settle_cancelled_pending(scheduler, ids, cause, actor)
            .await
    }

    pub(super) async fn settle_failed_pending(
        &self,
        scheduler: &mut Scheduler,
        failure: AgentError,
    ) -> Result<(), AgentError> {
        let ids = scheduler
            .queue
            .drain()
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        let permits = ids
            .iter()
            .map(|id| {
                &scheduler
                    .pending
                    .get(id)
                    .expect("admitted queue input")
                    ._work
            })
            .collect::<Vec<_>>();
        let claims = self.inner.lifecycle.claim_waiting_failure(&permits);
        drop(permits);
        let mut stopped = Vec::new();
        let mut failed = Vec::new();
        for (id, cancellation) in ids.into_iter().zip(claims) {
            if cancellation.is_some() {
                stopped.push(id);
            } else {
                failed.push(id);
            }
        }
        let mut aggregate_failure = self
            .settle_cancelled_pending(scheduler, stopped, SchedulingCause::RunnerStopped, None)
            .await
            .err();
        for id in failed {
            let pending = scheduler.pending.get(&id).expect("admitted queue input");
            let mut result = Err(failure.clone());
            let audit_record = ExecutionAuditRecord::QueueSettled(
                QueueSettlementRecord::automatic_attachment_failed(
                    self.inner.manager.id().clone(),
                    id.clone(),
                    pending.kind,
                    pending.target.clone(),
                    pending.submitter.context.clone(),
                )
                .expect("admitted pending scheduling transition"),
            );
            let audit = self
                .record_scheduling_audit(audit_record, "failed_queue_settlement")
                .await
                .map_err(AuditObservationFailure::into_agent_error);
            if let Err(error) = audit {
                let aggregate = error.clone();
                result = Err(AgentError::MultipleOperationFailures {
                    first_error: Box::new(result.expect_err("failed attachment settlement")),
                    subsequent_error: Box::new(error),
                });
                aggregate_failure = Some(match aggregate_failure.take() {
                    Some(first) => AgentError::MultipleOperationFailures {
                        first_error: Box::new(first),
                        subsequent_error: Box::new(aggregate),
                    },
                    None => aggregate,
                });
            }
            let settlement = self
                .catch_scheduling_panic(async {
                    self.inner
                        .manager
                        .settle_failed_queue(
                            pending.index,
                            event(
                                pending.kind,
                                pending.target.clone(),
                                Some(InvocationStage::Queued),
                                InvocationStage::Settled,
                                SchedulingCause::DispatchFailed,
                                None,
                            ),
                            QueueMutation::Removed {
                                id: id.clone(),
                                cause: QueueRemovalCause::DispatchFailed,
                            },
                            result.clone(),
                        )
                        .await
                })
                .await
                .unwrap_or_else(|()| {
                    Err(StorageError::Io(
                        "failed queue settlement persistence panicked".into(),
                    ))
                });
            if let Err(error) = settlement {
                let aggregate = AgentError::Storage(error.clone());
                aggregate_failure = Some(match aggregate_failure.take() {
                    Some(first) => AgentError::MultipleOperationFailures {
                        first_error: Box::new(first),
                        subsequent_error: Box::new(aggregate),
                    },
                    None => aggregate,
                });
                result = Err(AgentError::StorageAfterExecution {
                    error,
                    execution_result: Box::new(result),
                });
            }
            let retained = result.clone();
            let (result, receipt_storage) = self
                .catch_scheduling_panic(async {
                    self.inner
                        .manager
                        .settle_submission_with_storage_ack(pending.index, result)
                        .await
                })
                .await
                .unwrap_or_else(|()| {
                    let error =
                        StorageError::Io("failed queue receipt persistence panicked".into());
                    (
                        Err(AgentError::StorageAfterExecution {
                            error: error.clone(),
                            execution_result: Box::new(retained),
                        }),
                        Some(error),
                    )
                });
            let (result, retention_storage) = self
                .retain_receipt_after_failed_save(pending.index, result, receipt_storage.is_some())
                .await;
            if let Some(error) = receipt_storage {
                let aggregate = AgentError::Storage(error);
                aggregate_failure = Some(match aggregate_failure.take() {
                    Some(first) => AgentError::MultipleOperationFailures {
                        first_error: Box::new(first),
                        subsequent_error: Box::new(aggregate),
                    },
                    None => aggregate,
                });
            }
            if let Some(error) = retention_storage {
                let aggregate = AgentError::Storage(error);
                aggregate_failure = Some(match aggregate_failure.take() {
                    Some(first) => AgentError::MultipleOperationFailures {
                        first_error: Box::new(first),
                        subsequent_error: Box::new(aggregate),
                    },
                    None => aggregate,
                });
            }
            scheduler
                .pending
                .remove(&id)
                .expect("admitted queue input")
                .publish(result);
        }
        aggregate_failure.map_or(Ok(()), Err)
    }

    async fn settle_stopped_pending(&self, scheduler: &mut Scheduler) -> Result<(), AgentError> {
        let mut stopped_ids = scheduler
            .pending
            .iter()
            .filter(|(_, pending)| pending._work.cancellation().is_some())
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        // Match dispatch order without disturbing waiting work that survived
        // confirmed provider cleanup: steering first, then admission order.
        stopped_ids.sort_by_key(|id| {
            let pending = &scheduler.pending[id];
            (pending.kind != InvocationKind::Steering, pending.index)
        });
        for id in &stopped_ids {
            let _ = scheduler.queue.remove(id);
        }
        self.settle_cancelled_pending(scheduler, stopped_ids, SchedulingCause::RunnerStopped, None)
            .await
    }

    async fn settle_cancelled_pending(
        &self,
        scheduler: &mut Scheduler,
        ids: Vec<ExecutionId>,
        cause: SchedulingCause,
        actor: Option<ActionContext>,
    ) -> Result<(), AgentError> {
        let mut failure = None;
        for id in ids {
            let pending = scheduler.pending.get(&id).expect("admitted queue input");
            // Each waiting owner may have stopped earlier than this drain.
            let first_stop = pending._work.cancellation();
            let cause = first_stop.as_ref().map_or(cause, |event| event.cause);
            let actor = first_stop
                .as_ref()
                .map_or_else(|| actor.clone(), |event| event.actor.clone());
            let audit_record = ExecutionAuditRecord::QueueSettled(
                QueueSettlementRecord::cancelled(
                    self.inner.manager.id().clone(),
                    id.clone(),
                    pending.kind,
                    pending.target.clone(),
                    cause,
                    pending.submitter.context.clone(),
                    actor.clone(),
                )
                .expect("admitted pending cancellation transition"),
            );
            let audit = self
                .record_scheduling_audit(audit_record, "cancelled_queue_settlement")
                .await
                .map_err(AuditObservationFailure::into_agent_error);
            if let Err(error) = &audit {
                failure = Some(match failure.take() {
                    Some(first) => AgentError::MultipleOperationFailures {
                        first_error: Box::new(first),
                        subsequent_error: Box::new(error.clone()),
                    },
                    None => error.clone(),
                });
            }
            let recovering = self
                .inner
                .manager
                .scheduling_event(pending.index)
                .await
                .is_some_and(|last| last.stage == InvocationStage::Cancelled);
            // The map retains every Pending owner while user storage code runs.
            // Catch each item separately so a failed save cannot discard the tail.
            let saved = self
                .catch_scheduling_panic(async {
                    if recovering {
                        Ok(())
                    } else {
                        self.inner
                            .manager
                            .record_scheduling_with_queue(
                                pending.index,
                                event(
                                    pending.kind,
                                    pending.target.clone(),
                                    Some(InvocationStage::Queued),
                                    InvocationStage::Cancelled,
                                    cause,
                                    actor.clone(),
                                ),
                                Some(QueueMutation::Removed {
                                    id: id.clone(),
                                    cause: match cause {
                                        SchedulingCause::SessionClosed => {
                                            QueueRemovalCause::SessionClosed
                                        }
                                        SchedulingCause::Suspended => QueueRemovalCause::Suspended,
                                        _ => QueueRemovalCause::RunnerStopped,
                                    },
                                }),
                            )
                            .await
                    }
                })
                .await
                .unwrap_or_else(|()| {
                    Err(StorageError::Io(
                        "pending cancellation persistence panicked".into(),
                    ))
                });
            let saved = saved.and_then(|()| {
                if recovering {
                    Err(StorageError::Io(
                        "pending cancellation persistence was interrupted".into(),
                    ))
                } else {
                    Ok(())
                }
            });
            let mut result = match audit {
                Ok(()) => Err(AgentError::Closed),
                Err(error) => Err(AgentError::MultipleOperationFailures {
                    first_error: Box::new(AgentError::Closed),
                    subsequent_error: Box::new(error),
                }),
            };
            if let Err(error) = saved {
                let aggregate = AgentError::Storage(error.clone());
                failure = Some(match failure.take() {
                    Some(first) => AgentError::MultipleOperationFailures {
                        first_error: Box::new(first),
                        subsequent_error: Box::new(aggregate),
                    },
                    None => aggregate,
                });
                result = Err(AgentError::StorageAfterExecution {
                    error,
                    execution_result: Box::new(result),
                });
            }
            // Retain the cancellation failure without letting a second storage
            // panic interrupt receipt delivery or the remaining cancellations.
            let retained = result.clone();
            let (result, receipt_storage) = if recovering || result != Err(AgentError::Closed) {
                self.catch_scheduling_panic(async {
                    self.inner
                        .manager
                        .settle_submission_with_storage_ack(pending.index, result)
                        .await
                })
                .await
                // The manager retains this result before calling storage; use the
                // same result for the receipt if that save panics, then retain the
                // newly known failure without recursively attempting storage.
                .unwrap_or_else(|()| {
                    let error = StorageError::Io(
                        "pending cancellation receipt persistence panicked".into(),
                    );
                    let result = Err(AgentError::StorageAfterExecution {
                        error: error.clone(),
                        execution_result: Box::new(retained),
                    });
                    (result, Some(error))
                })
            } else {
                (result, None)
            };
            let (result, retention_storage) = self
                .retain_receipt_after_failed_save(pending.index, result, receipt_storage.is_some())
                .await;
            if let Some(error) = receipt_storage {
                let aggregate = AgentError::Storage(error);
                failure = Some(match failure.take() {
                    Some(first) => AgentError::MultipleOperationFailures {
                        first_error: Box::new(first),
                        subsequent_error: Box::new(aggregate),
                    },
                    None => aggregate,
                });
            }
            if let Some(error) = retention_storage {
                let aggregate = AgentError::Storage(error);
                failure = Some(match failure.take() {
                    Some(first) => AgentError::MultipleOperationFailures {
                        first_error: Box::new(first),
                        subsequent_error: Box::new(aggregate),
                    },
                    None => aggregate,
                });
            }
            scheduler
                .pending
                .remove(&id)
                .expect("admitted queue input")
                .publish(result);
        }
        failure.map_or(Ok(()), Err)
    }

    async fn retain_receipt_after_failed_save(
        &self,
        index: usize,
        result: Result<ExecutionOutcome, AgentError>,
        save_failed: bool,
    ) -> (Result<ExecutionOutcome, AgentError>, Option<StorageError>) {
        if !save_failed {
            return (result, None);
        }
        match self
            .inner
            .manager
            .retain_submission_result(index, result.clone())
            .await
        {
            Ok(()) => (result, None),
            Err(error) => (
                Err(AgentError::MultipleOperationFailures {
                    first_error: Box::new(result.expect_err("failed save has failed receipt")),
                    subsequent_error: Box::new(AgentError::Storage(error.clone())),
                }
                .bounded()),
                Some(error),
            ),
        }
    }

    pub(super) async fn close_scheduled(
        &self,
        actor: ActionContext,
        mode: CloseMode,
    ) -> Result<CloseOutcome, AgentError> {
        let agent = self.clone();
        let waiter = CallerWaiter::Close(self.inner.manager.id().clone());
        contain_caller_wake(
            waiter,
            tokio::spawn(async move {
                let attempt = match mode {
                    CloseMode::Final => agent.start_shutdown(SessionCloseRequest::Explicit(actor)),
                    CloseMode::Suspend => agent.inner.lifecycle.start_suspend(actor),
                };
                let (cause, actor) = match &attempt.request {
                    SessionCloseRequest::Explicit(actor) => (
                        agent.inner.lifecycle.caller_close_cause(),
                        Some(actor.clone()),
                    ),
                    _ => (SchedulingCause::RunnerStopped, None),
                };
                let mut scheduler = agent.inner.scheduler.lock().await;
                let saved = agent.cancel_pending(&mut scheduler, cause, actor).await;
                drop(scheduler);
                let _invocation = agent.inner.invocation.lock().await;
                agent.inner.manager.await_admission_writes().await;
                agent.inner.lifecycle.wait_for_work().await;
                let cleanup = agent.inner.lifecycle.complete_stop(&attempt).await;
                let cleanup = cleanup.into_result();
                match saved {
                    Ok(()) => cleanup,
                    Err(AgentError::Storage(error)) => Err(AgentError::StorageDuringClose {
                        error,
                        cleanup_result: Box::new(cleanup),
                    }),
                    Err(error) => Err(error),
                }
            }),
        )
        .await
        .map_err(|_| AgentError::Closed)?
    }
}

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/agents/scheduling.rs"]
mod tests;
