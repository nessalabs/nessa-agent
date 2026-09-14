//! Owns accepted work independently of a surface's wait future.
#![deny(missing_docs)]

use super::lifecycle::{SessionLifecycle, WorkGeneration, WorkPermit};
use super::submissions::{self, Settlement, SubmissionReceipt};
use super::{Agent, AgentError};
use crate::application::agent_execution::{
    executions::{ExecutionRequest, SubmissionMode},
    permissions::ActionContext,
    providers::{
        CloseOutcome, ProviderOperationFailure, ProviderSessionState, SessionCloseRequest,
        SteeringOutcome,
    },
    sessions::{InvocationSchedulingEvent, StorageError},
};
use crate::domain::agent_execution::executions::{
    ExecutionId, ExecutionOutcome, InvocationKind, InvocationQueue, InvocationStage,
    SchedulingCause, SchedulingInitiator, SchedulingTransition,
};
use std::{
    collections::{HashMap, HashSet},
    future::{poll_fn, Future},
    panic::{catch_unwind, AssertUnwindSafe},
    task::Poll,
};
use tokio::sync::watch;

/// Receipt for admitted work. Dropping it stops waiting, not execution.
/// Accepted inputs are saved before this receipt is returned. The owning Tokio
/// runtime must remain alive; process/runtime shutdown does not replay unfinished
/// work automatically. Use Agent::close before shutting down the runtime.
pub struct QueuedInvocation {
    id: ExecutionId,
    result: watch::Receiver<Settlement>,
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
    pub async fn wait(mut self) -> Result<ExecutionOutcome, AgentError> {
        loop {
            if let Some(result) = self.result.borrow_and_update().clone() {
                return result;
            }
            self.result
                .changed()
                .await
                .map_err(|_| AgentError::SubmissionUnresolved)?;
        }
    }
}

/// Acknowledged delivery path for one steering input.
pub enum SteeringDelivery {
    /// Provider acknowledged injection into `target`; its output and completion
    /// still belong to that original execution. This is not a completed tool effect.
    Injected {
        /// Active execution that accepted this additional input.
        target: ExecutionId,
    },
    /// No turn consumed the input; it was admitted for the next invocation boundary.
    Queued(QueuedInvocation),
}

/// Result of removing an input from the local pending queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueRemoval {
    /// Removed before dispatch; its receipt resolves Closed and evidence remains.
    Removed,
    /// Unknown, already removed, dispatched, or injected. No effect was undone.
    NotPending,
}

pub(super) struct Scheduler {
    queue: InvocationQueue,
    pending: HashMap<ExecutionId, Pending>,
    running: bool,
    receipts: HashMap<ExecutionId, SubmissionReceipt>,
}
struct Pending {
    input: ExecutionRequest,
    actor: ActionContext,
    index: usize,
    kind: InvocationKind,
    target: Option<ExecutionId>,
    reply: watch::Sender<Settlement>,
    _work: WorkPermit,
}
impl Scheduler {
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
    /// Saves `input` and its verified `actor`, then schedules a sequential invocation.
    ///
    /// Ordinary inputs run FIFO in admission order across all Agent clones. At
    /// most 64 inputs may wait; a full queue returns a Scheduling error. Retrying
    /// the same execution ID, input, actor, and operation joins its original receipt,
    /// even after settlement. Changed intent returns SubmissionConflict. Restored
    /// unresolved delivery returns SubmissionUnresolved without resending it.
    /// WorkStatus is supervised once polled, including while saving input.
    /// Validation/storage errors prevent admission. Each dispatched input runs the
    /// normal invocation hooks. Dropping the returned receipt does not cancel it.
    /// Queued work is retained but is never automatically replayed after restart.
    ///
    /// # Examples
    /// ```
    /// use nessa_sdk::{Agent, application::agent_execution::{
    ///     agents::AgentError, executions::{ExecutionRequest, SubmissionMode}, permissions::ActionContext,
    /// }};
    /// # async fn submit(agent: &Agent, input: ExecutionRequest, actor: ActionContext) -> Result<(), AgentError> {
    /// let receipt = agent.enqueue(input, actor).await?;
    /// let outcome = receipt.wait().await?;
    /// # let _ = outcome;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn enqueue(
        &self,
        input: ExecutionRequest,
        actor: ActionContext,
    ) -> Result<QueuedInvocation, AgentError> {
        self.enqueue_kind(input, actor, InvocationKind::Queued)
            .await
    }

    /// Saves `input` from verified `actor` for the next invocation boundary, ahead
    /// of ordinary queued inputs. Steering inputs preserve their own FIFO order.
    /// This explicit boundary operation does not interrupt or inject into a turn.
    /// WorkStatus and failure guarantees are the same as [`Self::enqueue`].
    pub async fn enqueue_steering(
        &self,
        input: ExecutionRequest,
        actor: ActionContext,
    ) -> Result<QueuedInvocation, AgentError> {
        self.enqueue_kind(input, actor, InvocationKind::Steering)
            .await
    }

    /// Removes pending `id`, including boundary steering, using host-verified
    /// `actor` attribution. Dispatch and removal serialize: only one can win.
    /// The input and Withdrawn cause stay in history; this does not delete evidence.
    /// Already injected/dispatched inputs return NotPending and cannot be unsent.
    /// Storage failure still removes the input and reports the failed audit write.
    /// Once polled, removal is supervised even if the caller stops waiting.
    pub async fn remove_queued(
        &self,
        id: ExecutionId,
        actor: ActionContext,
    ) -> Result<QueueRemoval, AgentError> {
        let agent = self.clone();
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
                        .record_scheduling(
                            pending.index,
                            event(
                                pending.kind,
                                pending.target.clone(),
                                Some(InvocationStage::Queued),
                                InvocationStage::Cancelled,
                                SchedulingCause::Withdrawn,
                                Some(actor),
                            ),
                        )
                        .await;
                    match saved {
                        Ok(()) => None,
                        Err(error) => Some(
                            agent
                                .inner
                                .manager
                                .settle_submission(pending.index, Err(AgentError::Storage(error)))
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
                            .recover_scheduling_panic(&id, &close_notice)
                            .await
                            .expect_err("withdrawal save panicked"),
                    )
                }
            };
            pending
                .reply
                .send_replace(Some(Err(failure.clone().unwrap_or(AgentError::Closed))));
            if let Some(error) = failure {
                return Err(error);
            }
            Ok(QueueRemoval::Removed)
        })
        .await
        .map_err(|_| AgentError::Closed)?
    }

    async fn enqueue_kind(
        &self,
        input: ExecutionRequest,
        actor: ActionContext,
        kind: InvocationKind,
    ) -> Result<QueuedInvocation, AgentError> {
        let agent = self.clone();
        tokio::spawn(async move { agent.accept_work(input, actor, kind).await })
            .await
            .map_err(|_| AgentError::SubmissionUnresolved)?
    }

    async fn accept_work(
        &self,
        input: ExecutionRequest,
        actor: ActionContext,
        kind: InvocationKind,
    ) -> Result<QueuedInvocation, AgentError> {
        let mut scheduler = self.inner.scheduler.lock().await;
        let mode = match kind {
            InvocationKind::Queued => SubmissionMode::Queued,
            InvocationKind::Steering => SubmissionMode::BoundarySteering,
        };
        if let Some(recovered) = submissions::recover(
            &self.inner.manager,
            &scheduler.receipts,
            &input,
            &actor,
            mode,
        )
        .await
        {
            return match recovered? {
                SteeringDelivery::Queued(receipt) => Ok(receipt),
                SteeringDelivery::Injected { .. } => Err(AgentError::SubmissionConflict),
            };
        }
        self.inner.session.validate(&input)?;
        let work = self.inner.lifecycle.accept_waiting_work()?;
        scheduler
            .queue
            .validate_enqueue(&input.execution_id)
            .map_err(AgentError::Scheduling)?;
        let index = self
            .inner
            .manager
            .begin_with_scheduling(
                input.clone(),
                actor.clone(),
                event(
                    kind,
                    None,
                    None,
                    InvocationStage::Queued,
                    SchedulingCause::Submitted,
                    Some(actor.clone()),
                ),
                mode,
            )
            .await?;
        // The scheduler lock spans validation and persistence, so no admission
        // can invalidate this check while the evidence is saved.
        scheduler
            .queue
            .enqueue(input.execution_id.clone(), kind)
            .expect("admission checked under scheduler lock");
        let receipt = Self::accept_pending(&mut scheduler, input, actor, index, kind, None, work);
        self.start_runner(&mut scheduler);
        Ok(receipt)
    }

    fn accept_pending(
        scheduler: &mut Scheduler,
        input: ExecutionRequest,
        actor: ActionContext,
        index: usize,
        kind: InvocationKind,
        target: Option<ExecutionId>,
        work: WorkPermit,
    ) -> QueuedInvocation {
        let (reply, result) = watch::channel(None);
        let id = input.execution_id.clone();
        scheduler
            .receipts
            .insert(id.clone(), SubmissionReceipt::Queued(result.clone()));
        scheduler.pending.insert(
            id.clone(),
            Pending {
                input,
                actor,
                index,
                kind,
                target,
                reply,
                _work: work,
            },
        );
        QueuedInvocation { id, result }
    }
    fn start_runner(&self, scheduler: &mut Scheduler) {
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
            // Wait for a direct invocation without removing pending work: close
            // can still cancel every waiting item and prevent automatic restart.
            let _active = self.inner.invocation.lock().await;
            let (pending, close_notice) = {
                let mut scheduler = self.inner.scheduler.lock().await;
                let close_notice = self.inner.lifecycle.close_notice();
                // A stop may come from permission or execution cleanup, without
                // a native-steering caller available to settle queued receipts.
                if let Err(error) = self.settle_stopped_pending(&mut scheduler).await {
                    // Each affected receipt already retains its persistence error.
                    tracing::warn!(?error, "failed to save stopped queue receipts");
                }
                if self.inner.lifecycle.is_closed() {
                    // A stop can race the first drain while it awaits storage.
                    // Once closed is observed, collect those newly stopped owners too.
                    if let Err(error) = self.settle_stopped_pending(&mut scheduler).await {
                        tracing::warn!(?error, "failed to save stopped queue receipts");
                    }
                    scheduler.running = false;
                    return;
                }
                let Some((id, _)) = scheduler.queue.pop_next() else {
                    scheduler.running = false;
                    return;
                };
                (
                    scheduler.pending.remove(&id).expect("admitted queue input"),
                    close_notice,
                )
            };
            let result = match self
                .catch_scheduling_panic(self.run_pending(&pending))
                .await
            {
                Ok(result) => result,
                Err(()) => {
                    let result = self
                        .recover_scheduling_panic(&pending.input.execution_id, &close_notice)
                        .await;
                    pending.reply.send_replace(Some(result));
                    self.inner.scheduler.lock().await.running = false;
                    return;
                }
            };
            pending.reply.send_replace(Some(result));
        }
    }

    async fn run_pending(&self, pending: &Pending) -> Result<ExecutionOutcome, AgentError> {
        pending._work.activate();
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
        let dispatch_failed = dispatch.is_err();
        let result = match dispatch {
            Ok(()) => {
                self.supervise_invocation(
                    pending.input.clone(),
                    pending.actor.clone(),
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
            if let Err(AgentError::Storage(error)) = self
                .cancel_pending(
                    &mut scheduler,
                    cancellation
                        .as_ref()
                        .map_or(SchedulingCause::RunnerStopped, |event| event.cause),
                    cancellation.as_ref().and_then(|event| event.actor.clone()),
                )
                .await
            {
                result = Err(AgentError::StorageAfterExecution {
                    error,
                    execution_result: Box::new(result),
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

    async fn recover_scheduling_panic(
        &self,
        id: &ExecutionId,
        close_notice: &watch::Receiver<Option<ActionContext>>,
    ) -> Result<ExecutionOutcome, AgentError> {
        let attempt = self.start_shutdown(SessionCloseRequest::ExecutionFailed);
        let actor = if close_notice.has_changed().unwrap_or(true) {
            close_notice.borrow().clone()
        } else {
            None
        };
        let cause = if actor.is_some() {
            SchedulingCause::SessionClosed
        } else {
            SchedulingCause::RunnerStopped
        };
        let cleanup = attempt.clone().wait().await;
        let cleanup = self.inner.lifecycle.finalize_stop(&attempt, &cleanup).await;
        let error = AgentError::Protocol("scheduling task panicked".into());
        let error = match cleanup.into_result() {
            Ok(_) => error,
            Err(cleanup_error) => AgentError::OperationAndCleanupFailure {
                operation_error: Box::new(error),
                cleanup_error: Box::new(cleanup_error),
            }
            .bounded(),
        };
        let metadata = self.inner.manager.scheduling_metadata(id).await;
        let mut result = self
            .inner
            .manager
            .retain_invocation_failure(id, error)
            .await;
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
        if let Err(AgentError::Storage(error)) =
            self.cancel_pending(&mut scheduler, cause, actor).await
        {
            result = Err(AgentError::StorageAfterExecution {
                error,
                execution_result: Box::new(result),
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
    /// `actor`. Idle inputs run at the next invocation boundary. Unsupported native
    /// steering returns Unsupported; callers can explicitly choose enqueue_steering.
    ///
    /// Input is saved before contacting the provider. If its target settles during
    /// that save, the undispatched input enters the boundary queue. Once native
    /// delivery starts, only an explicit PromptRequired response allows queuing it
    /// as a new invocation. Timeouts, malformed replies and transport errors are
    /// never retried automatically.
    /// Dropping the caller's wait does not abandon accepted steering delivery.
    /// Identical retries recover the original injection acknowledgement, queued
    /// receipt, or error; they never inject a second time. Conflicting retries return
    /// SubmissionConflict; unresolved restored delivery returns SubmissionUnresolved.
    pub async fn steer(
        &self,
        input: ExecutionRequest,
        actor: ActionContext,
    ) -> Result<SteeringDelivery, AgentError> {
        let agent = self.clone();
        tokio::spawn(async move {
            let id = input.execution_id.clone();
            let close_notice = agent.inner.lifecycle.close_notice();
            let mut work_owner = None;
            match agent
                .catch_scheduling_panic(agent.deliver_steering(input, actor, &mut work_owner))
                .await
            {
                Ok(delivery) => delivery,
                Err(()) => {
                    let error = agent
                        .recover_scheduling_panic(&id, &close_notice)
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
        })
        .await
        .map_err(|_| AgentError::Closed)?
    }

    async fn deliver_steering(
        &self,
        input: ExecutionRequest,
        actor: ActionContext,
        work_owner: &mut Option<WorkPermit>,
    ) -> Result<SteeringDelivery, AgentError> {
        let mut scheduler = self.inner.scheduler.lock().await;
        if let Some(recovered) = submissions::recover(
            &self.inner.manager,
            &scheduler.receipts,
            &input,
            &actor,
            SubmissionMode::Steering,
        )
        .await
        {
            return recovered;
        }
        self.inner.session.validate(&input)?;
        let close_notice = self.inner.lifecycle.close_notice();
        if !self.inner.lifecycle.accepts_queued() {
            return Err(AgentError::Closed);
        }
        *work_owner = Some(self.inner.lifecycle.accept_waiting_work()?);
        scheduler
            .queue
            .validate_enqueue(&input.execution_id)
            .map_err(AgentError::Scheduling)?;
        let target = self.inner.lifecycle.active();
        let index = self
            .inner
            .manager
            .begin_with_scheduling(
                input.clone(),
                actor.clone(),
                event(
                    InvocationKind::Steering,
                    target.clone(),
                    None,
                    InvocationStage::Queued,
                    SchedulingCause::Submitted,
                    Some(actor.clone()),
                ),
                SubmissionMode::Steering,
            )
            .await?;
        // WorkStatus persistence can be overtaken by close or provider settlement.
        // Revalidate the captured target before handing anything to the adapter.
        let outcome = if !self.inner.lifecycle.accepts_queued()
            || close_notice.has_changed().unwrap_or(true)
        {
            Err(AgentError::Closed)
        } else if self.inner.lifecycle.active().as_ref() != target.as_ref() {
            Ok(SteeringOutcome::PromptRequired)
        } else {
            match &target {
                Some(target) => {
                    work_owner.as_ref().expect("steering owner").activate();
                    self.deliver_native_steering(target.clone(), input.clone())
                        .await
                }
                None => Ok(SteeringOutcome::PromptRequired),
            }
        };
        let id = input.execution_id.clone();
        let delivery = match outcome {
            Ok(SteeringOutcome::Injected) => {
                let saved = self
                    .inner
                    .manager
                    .record_scheduling(
                        index,
                        event(
                            InvocationKind::Steering,
                            target.clone(),
                            Some(InvocationStage::Queued),
                            InvocationStage::Injected,
                            SchedulingCause::SteeringInjected,
                            None,
                        ),
                    )
                    .await;
                if let Err(error) = saved {
                    let error = self
                        .stop_after_steering(
                            &mut scheduler,
                            AgentError::Storage(error),
                            &close_notice,
                        )
                        .await;
                    let error = self
                        .inner
                        .manager
                        .settle_submission(index, Err(error))
                        .await
                        .expect_err("failed steering receipt");
                    scheduler
                        .receipts
                        .insert(id, SubmissionReceipt::Steering(Err(error.clone())));
                    return Err(error);
                }
                Ok(SteeringDelivery::Injected {
                    target: target.expect("active injection target"),
                })
            }
            Ok(SteeringOutcome::PromptRequired) => {
                scheduler
                    .queue
                    .enqueue(input.execution_id.clone(), InvocationKind::Steering)
                    .expect("admission checked under scheduler lock");
                let receipt = Self::accept_pending(
                    &mut scheduler,
                    input,
                    actor,
                    index,
                    InvocationKind::Steering,
                    target,
                    work_owner.take().expect("admitted steering owner"),
                );
                self.start_runner(&mut scheduler);
                Ok(SteeringDelivery::Queued(receipt))
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
        if let Err(AgentError::Storage(error)) = self.settle_stopped_pending(&mut scheduler).await {
            delivery = Err(match delivery {
                Err(prior) => AgentError::StorageAfterExecution {
                    error,
                    execution_result: Box::new(Err(prior)),
                },
                Ok(_) => AgentError::Storage(error),
            });
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
            Ok(SteeringDelivery::Injected { target }) => {
                scheduler
                    .receipts
                    .insert(id, SubmissionReceipt::Steering(Ok(target.clone())));
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
    ) -> Result<SteeringOutcome, AgentError> {
        let admission = self.accept_control()?;
        let mut cleanup_needed = false;
        let mut delivery = Box::pin(async { self.inner.session.steer(target, input).await });
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
        if !cleanup_needed {
            return outcome;
        }
        // Only this operation's typed failure owns cleanup here. An unrelated
        // stop already has an owner and must not delay an interrupted response.
        // The scheduler is held, so join cleanup without acquiring it again.
        let Some(attempt) = self.inner.lifecycle.start_control_cleanup(&admission) else {
            return outcome;
        };
        let cleanup = attempt.clone().wait().await;
        let cleanup = self.inner.lifecycle.finalize_stop(&attempt, &cleanup).await;
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
            SchedulingCause::SessionClosed
        } else {
            SchedulingCause::RunnerStopped
        };
        if let Err(AgentError::Storage(storage)) =
            self.cancel_pending(scheduler, cause, actor).await
        {
            error = AgentError::StorageAfterExecution {
                error: storage,
                execution_result: Box::new(Err(error)),
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
                            .record_scheduling(
                                pending.index,
                                event(
                                    pending.kind,
                                    pending.target.clone(),
                                    Some(InvocationStage::Queued),
                                    InvocationStage::Cancelled,
                                    cause,
                                    actor.clone(),
                                ),
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
            let result = match saved {
                Ok(()) => Err(AgentError::Closed),
                Err(error) => {
                    failure.get_or_insert(error.clone());
                    Err(AgentError::Storage(error))
                }
            };
            // Retain the cancellation failure without letting a second storage
            // panic interrupt receipt delivery or the remaining cancellations.
            let result = if recovering || matches!(&result, Err(AgentError::Storage(_))) {
                let retained = result.clone();
                self.catch_scheduling_panic(async {
                    self.inner
                        .manager
                        .settle_submission(pending.index, result)
                        .await
                })
                .await
                // The manager retains this result before calling storage; use the
                // same result for the receipt if that save panics.
                .unwrap_or(retained)
            } else {
                result
            };
            if let Err(
                AgentError::StorageAfterExecution { error, .. } | AgentError::Storage(error),
            ) = &result
            {
                failure.get_or_insert(error.clone());
            }
            pending.reply.send_replace(Some(result));
            scheduler.pending.remove(&id);
        }
        failure.map_or(Ok(()), |error| Err(AgentError::Storage(error)))
    }

    pub(super) async fn close_scheduled(
        &self,
        actor: ActionContext,
    ) -> Result<CloseOutcome, AgentError> {
        let agent = self.clone();
        tokio::spawn(async move {
            let attempt = agent.start_shutdown(SessionCloseRequest::Explicit(actor));
            let (cause, actor) = match &attempt.request {
                SessionCloseRequest::Explicit(actor) => {
                    (SchedulingCause::SessionClosed, Some(actor.clone()))
                }
                _ => (SchedulingCause::RunnerStopped, None),
            };
            let (mut scheduler, cleanup) =
                tokio::join!(agent.inner.scheduler.lock(), attempt.clone().wait());
            let saved = agent.cancel_pending(&mut scheduler, cause, actor).await;
            drop(scheduler);
            let _invocation = agent.inner.invocation.lock().await;
            agent.inner.manager.await_admission_writes().await;
            agent.inner.lifecycle.wait_for_work().await;
            let cleanup = agent
                .inner
                .lifecycle
                .finalize_stop(&attempt, &cleanup)
                .await;
            let cleanup = cleanup.into_result();
            match saved {
                Ok(()) => cleanup,
                Err(AgentError::Storage(error)) => Err(AgentError::StorageDuringClose {
                    error,
                    cleanup_result: Box::new(cleanup),
                }),
                Err(error) => Err(error),
            }
        })
        .await
        .map_err(|_| AgentError::Closed)?
    }
}
