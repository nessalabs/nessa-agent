//! Continues turns that a restart, the host's own shutdown, or a failure a
//! retry can fix cut off.
//!
//! ```text
//! host opens the Agent again -> owed_turns -> OwedTurn per cut-off turn
//! host checks who may resume it -> resume_turn
//!     |-> cut off (no result, nothing running it): record Cancelled(Interrupted)
//!     |-> derived successor id (Resumption) -> accept_work as an ordinary queued turn
//!     '-> TurnResumed audit: Continued { successor } | Exhausted
//! ```
//!
//! The saved history alone decides what is owed. A turn is owed when its last
//! scheduling edge is a cancellation whose cause leaves work owed
//! ([`SchedulingCause::leaves_work_owed`]); when a failure ended it and its
//! retained error is one a retry can fix ([`AgentError::leaves_work_owed`]),
//! its own failure or the runner's that stopped behind another's; or when it
//! has no result and is still Queued or Running with nothing in this process
//! running it. In each case no successor has been saved for it yet. The SDK
//! does not replay saved tool calls directly. A new model turn receives a notice
//! and the original request and is asked to check what is done before carrying
//! on. The model may still issue a tool call again; this is not an exactly-once
//! external-effect guarantee.
#![deny(missing_docs)]

use super::scheduling::Scheduler;
use super::{Agent, AgentError, QueueAdmission};
use crate::application::agent_execution::{
    caller_wake::{contain_caller_wake, CallerWaiter},
    executions::{
        ExecutionAuditRecord, ExecutionRequest, Submitter, TurnResumption, TurnResumptionRecord,
    },
    permissions::ActionContext,
    providers::validate_configured_input,
    sessions::{
        validation::invocation_settlement_cause, InvocationRecord, InvocationSchedulingEvent,
        NoteCandidates,
    },
};
use crate::domain::agent_execution::{
    executions::{
        ExecutionId, ExecutionOutcome, Initiation, InvocationKind, InvocationStage,
        SchedulingCause, SchedulingInitiator, SchedulingTransition, SubmissionMode,
    },
    prompts::{PromptText, UserMessage},
    sessions::SessionId,
};
use lineage::{Continuation, Lineage};

/// Opens the successor of a turn the provider had been given.
const CUT_OFF_NOTICE: &str = "[Nessa restarted while you were working on this request, so your \
last turn was cut off before it finished. Some of it may already be done: check the current \
state before repeating any step, then carry on. If the work is already finished, say so and \
stop.]\n\nThe request was:\n\n";
/// Opens the successor of a turn that never reached the provider.
const UNSENT_NOTICE: &str = "[Nessa restarted before this message reached you, so it is sent \
again now. Check the current state first: earlier work may have changed it.]\n\n";
/// Opens the successor of a turn the provider failed while working on it.
const FAILED_NOTICE: &str = "[Your last turn on this request ended early because the agent \
failed, so it is sent again now. Some of it may already be done: check the current state before \
repeating any step, then carry on. If the work is already finished, say so and stop.]\n\nThe \
request was:\n\n";
/// Opens the successor of a turn a failure kept from reaching the provider.
const HELD_NOTICE: &str = "[This message did not reach you because the agent failed before it \
could, so it is sent again now. Check the current state first: earlier work may have changed \
it.]\n\n";

/// One turn whose work is owed, as [`Agent::owed_turns`] found it.
/// Bound to its persistent session: it can be used after that same session
/// is reopened, but resuming or declining it in another session has no effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwedTurn {
    session_id: SessionId,
    index: usize,
    execution_id: ExecutionId,
    submitted_by: ActionContext,
    initiation: Initiation,
    dispatched: bool,
}
impl OwedTurn {
    /// The turn that was cut off.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    /// Who submitted it: the successor is submitted under the same caller.
    pub fn submitted_by(&self) -> &ActionContext {
        &self.submitted_by
    }
    /// Whether the owner or a share grant started it. A host resumes a
    /// grant's turn only while that grant still holds.
    pub fn initiation(&self) -> &Initiation {
        &self.initiation
    }
    /// Whether the provider had been given the turn before it stopped.
    pub fn dispatched(&self) -> bool {
        self.dispatched
    }
}

/// What [`Agent::resume_turn`] did.
pub enum Resumed {
    /// A successor turn is queued; its receipt settles when it does.
    Continued(QueueAdmission),
    /// A successor turn is queued, and its [`TurnResumption::Continued`]
    /// record could not be saved. The successor runs regardless: the host
    /// still watches its receipt, and reports `error`.
    ContinuedUnaudited {
        /// The successor's receipt.
        admission: QueueAdmission,
        /// Why the audit record was not saved.
        error: AgentError,
    },
    /// The turn was cut off [`Resumption::MAX_RESUMES`](crate::domain::agent_execution::executions::Resumption::MAX_RESUMES) times running and is
    /// left ended.
    Exhausted,
    /// The turn is no longer owed: it was resumed already, is running here,
    /// or belongs to another persistent session.
    NotOwed,
}

/// How an owed turn stands in saved history.
enum Stop {
    /// Its last edge is this owed cancellation, or this failure.
    Recorded(InvocationSchedulingEvent),
    /// A saved prefix determines an ending the old process did not record.
    /// An explicit resume or decline saves this edge before its own decision.
    Unrecorded { edge: InvocationSchedulingEvent },
}

impl Stop {
    fn edge(&self) -> &InvocationSchedulingEvent {
        match self {
            Self::Recorded(edge) | Self::Unrecorded { edge } => edge,
        }
    }
}

/// The ending established by one valid record. A live owner retains an active
/// prefix; a result or captured stop from an earlier process can establish its
/// local ending without rewriting the acknowledged record during restoration.
fn ending(record: &InvocationRecord, live: bool) -> Option<Stop> {
    let last = record.scheduling.last()?;
    if matches!(
        last.stage,
        InvocationStage::Cancelled | InvocationStage::Settled
    ) {
        return Some(Stop::Recorded(last.clone()));
    }
    if !matches!(
        last.stage,
        InvocationStage::Queued | InvocationStage::Running
    ) || live
    {
        return None;
    }
    let (stage, cause, actor) = if let Some(stop) = &record.local_cancellation {
        // Validated local cancellation is paired with its LocalCancellation
        // report. It already establishes the applied stop, including its actor.
        (InvocationStage::Cancelled, stop.cause, stop.actor.clone())
    } else if record.result.is_some() {
        let settlement = invocation_settlement_cause(record).ok().flatten()?;
        let cause = if last.stage == InvocationStage::Queued {
            SchedulingCause::DispatchFailed
        } else {
            settlement
        };
        (InvocationStage::Settled, cause, None)
    } else {
        (
            InvocationStage::Cancelled,
            SchedulingCause::Interrupted,
            None,
        )
    };
    let transition = SchedulingTransition::new(
        last.kind,
        last.target.clone(),
        Some(last.stage),
        stage,
        cause,
        if actor.is_some() {
            SchedulingInitiator::Caller
        } else {
            SchedulingInitiator::Automatic
        },
    )
    .ok()?;
    Some(Stop::Unrecorded {
        edge: InvocationSchedulingEvent {
            kind: transition.kind(),
            target: transition.target().cloned(),
            before: transition.before(),
            stage: transition.stage(),
            cause: transition.cause(),
            actor,
        },
    })
}

/// Whether this ending leaves work owed. The same projection serves live
/// decisions and committed report frames; retry eligibility remains owned by
/// failure_owed and the retained cause, with later ordinary work superseding
/// failures as before.
fn owed(
    record: &InvocationRecord,
    lineage: &Lineage,
    superseded: bool,
    live: impl Fn(&ExecutionId) -> bool,
) -> Option<Stop> {
    let id = &record.request.execution_id;
    if record.submission == SubmissionMode::Immediate || lineage.successor_of(id).is_some() {
        return None;
    }
    let stop = ending(record, live(id))?;
    let last = stop.edge();
    if (last.stage == InvocationStage::Cancelled && last.cause.leaves_work_owed())
        || (last.cause.stopped_by_failure() && !superseded && failure_owed(record))
    {
        Some(stop)
    } else {
        None
    }
}

/// Whether `record` is a turn that moved past every failure before it: one
/// that ran, however it ended, or that ended for good or this process
/// admitted (it is not owed and was not resumed). A turn that only waited
/// behind a failure and was stopped with it has not. A steering message
/// belongs to the turn it steered, and a successor carries on the host's
/// resume rather than a person's next step, so neither counts.
fn ended_for_good(
    record: &InvocationRecord,
    lineage: &Lineage,
    live: impl Fn(&ExecutionId) -> bool,
) -> bool {
    record.submission != SubmissionMode::Immediate
        && !lineage.is_successor(&record.request.execution_id)
        && (dispatched(record)
            || (lineage.successor_of(&record.request.execution_id).is_none()
                && owed(record, lineage, false, live).is_none()))
}

/// Whether a turn that a failure ended still owes its work: no stop but the
/// runner's was asked for it, and it has no result, or a cancelled one, or
/// an error a retry can fix ([`AgentError::leaves_work_owed`]). A turn the
/// provider finished or refused, and one a person stopped, is not owed.
fn failure_owed(record: &InvocationRecord) -> bool {
    let stopped_by_others = record
        .local_cancellation
        .as_ref()
        .is_some_and(|stop| stop.cause != SchedulingCause::RunnerStopped);
    !stopped_by_others
        && match &record.result {
            None | Some(Ok(ExecutionOutcome::Cancelled)) => true,
            Some(Ok(_)) => false,
            Some(Err(error)) => error.leaves_work_owed(),
        }
}

/// The host's decision not to resume a turn whose last edge is `owed`, an
/// owed cancellation or a failure a retry can fix, made by `actor`.
fn not_resumed(
    owed: &InvocationSchedulingEvent,
    actor: ActionContext,
) -> Result<InvocationSchedulingEvent, AgentError> {
    let transition = SchedulingTransition::new(
        owed.kind,
        owed.target.clone(),
        Some(owed.stage),
        InvocationStage::Cancelled,
        SchedulingCause::NotResumed,
        SchedulingInitiator::Caller,
    )
    .map_err(|error| AgentError::InvalidInput(error.to_string()))?;
    Ok(InvocationSchedulingEvent {
        kind: transition.kind(),
        target: transition.target().cloned(),
        before: transition.before(),
        stage: transition.stage(),
        cause: transition.cause(),
        actor: Some(actor),
    })
}

fn dispatched(record: &InvocationRecord) -> bool {
    record
        .scheduling
        .iter()
        .any(|edge| edge.stage == InvocationStage::Running)
}

/// The successor's request: a notice, then the original message. The images
/// of a turn the provider was given are left out, since the model has seen
/// them; a turn it never had is sent whole.
fn successor_request(
    original_request: &ExecutionRequest,
    dispatched: bool,
    cause: SchedulingCause,
    successor: ExecutionId,
) -> Result<ExecutionRequest, AgentError> {
    let original = &original_request.user_message;
    let request = original.text_str();
    let notice = match (dispatched, cause.stopped_by_failure()) {
        (true, false) => CUT_OFF_NOTICE,
        (false, false) => UNSENT_NOTICE,
        (true, true) => FAILED_NOTICE,
        (false, true) => HELD_NOTICE,
    };
    let room = ExecutionRequest::MAX_MESSAGE_BYTES - notice.len();
    let mut end = request.len().min(room);
    while !request.is_char_boundary(end) {
        end -= 1;
    }
    let text = PromptText::new(format!("{notice}{}", &request[..end]))
        .map_err(|error| AgentError::InvalidInput(error.to_string()))?;
    let images = if dispatched {
        Vec::new()
    } else {
        original.images().to_vec()
    };
    let message = UserMessage::new(Some(text), images, original.files().to_vec())
        .map_err(|error| AgentError::InvalidInput(error.to_string()))?
        .sent_by(original.sender().clone());
    Ok(ExecutionRequest {
        execution_id: successor,
        user_message: message,
        estimated_input_tokens: original_request.estimated_input_tokens,
        reserved_output_tokens: original_request.reserved_output_tokens,
    })
}

struct SettledTurn {
    record: InvocationRecord,
    cause: SchedulingCause,
    continuation: Continuation,
}

impl Agent {
    /// Every turn in this conversation's saved history whose work is owed —
    /// cut off by a restart, stopped by the host's own shutdown
    /// ([`Self::suspend`]), or ended by a failure a retry can fix — and not
    /// yet resumed, in saved order. Turns this Agent has queued or is running
    /// are never owed; one it ended with such a failure is. Read-only:
    /// nothing is recorded until [`Self::resume_turn`].
    pub async fn owed_turns(&self) -> Vec<OwedTurn> {
        contain_caller_wake(
            CallerWaiter::ResumptionState(self.inner.manager.id().clone()),
            async {
                let scheduler = self.inner.scheduler.lock().await;
                let active = self.inner.lifecycle.active();
                let live =
                    |id: &ExecutionId| scheduler.has_receipt(id) || active.as_ref() == Some(id);
                self.inner
                    .manager
                    .with_invocations(|records| {
                        let lineage = Lineage::new(records);
                        let newest_final = records
                            .iter()
                            .rposition(|record| ended_for_good(record, &lineage, live));
                        records
                            .iter()
                            .enumerate()
                            .filter_map(|(index, record)| {
                                owed(record, &lineage, newest_final > Some(index), live).map(|_| {
                                    OwedTurn {
                                        session_id: self.inner.manager.id().clone(),
                                        index,
                                        execution_id: record.request.execution_id.clone(),
                                        submitted_by: record.actor.clone(),
                                        initiation: record.initiation.clone(),
                                        dispatched: dispatched(record),
                                    }
                                })
                            })
                            .collect()
                    })
                    .await
            },
        )
        .await
    }

    /// Whether every turn this Agent admitted has settled: none is queued,
    /// and none is running, from the queue or invoked directly. A queued turn
    /// leaves the queue only while the runner holds the invocation slot,
    /// which it lets go just before the turn's receipt settles, so a host that
    /// asks as a receipt settles is not told "idle" while the next turn is
    /// being started. Admission takes the scheduler lock this is read under,
    /// so nothing admitted before the answer is missed by it.
    pub async fn is_idle(&self) -> bool {
        contain_caller_wake(
            CallerWaiter::ResumptionState(self.inner.manager.id().clone()),
            async {
                let scheduler = self.inner.scheduler.lock().await;
                scheduler.has_no_work() && self.inner.invocation.try_lock().is_ok()
            },
        )
        .await
    }

    /// Continue `turn` with a successor turn, resumed by the host's verified
    /// `actor`.
    ///
    /// A turn cut off with no result is first recorded cancelled with
    /// [`SchedulingCause::Interrupted`]. The successor is then queued as an
    /// ordinary turn under the original caller and initiation: a notice that
    /// the turn was cut off, and the original request, so the model checks
    /// what is already done and carries on or says it is finished. Its
    /// identity is derived from `turn` ([`Resumption`](crate::domain::agent_execution::executions::Resumption)), so resuming the same
    /// turn again answers [`Resumed::NotOwed`] and queues nothing more
    /// (`resume_twice_queues_one_successor`). A turn whose chain of resumes is
    /// spent is recorded cancelled with [`SchedulingCause::NotResumed`] and left
    /// ended ([`Resumed::Exhausted`]). Either way one
    /// [`ExecutionAuditRecord::TurnResumed`] records it.
    ///
    /// Once polled, an owned task retains the decision, successor admission,
    /// and audit through caller loss. Selection and admission share the
    /// scheduler lock (`concurrent_resumptions_of_one_owed_turn_make_one_decision`).
    ///
    /// The host decides who may resume a turn before calling this; the SDK
    /// does not re-check a share grant.
    /// A token from another persistent session answers [`Resumed::NotOwed`]
    /// without changing history or recording a resumption decision.
    ///
    /// # Errors
    /// What saving the cancellation edge returns, and what
    /// [`Self::enqueue`] returns for the successor.
    pub async fn resume_turn(
        &self,
        turn: &OwedTurn,
        actor: ActionContext,
    ) -> Result<Resumed, AgentError> {
        let agent = self.clone();
        let turn = turn.clone();
        let waiter = CallerWaiter::TurnResumption(turn.execution_id.clone());
        contain_caller_wake(
            waiter,
            tokio::spawn(async move { agent.resume_owed(turn, actor).await }),
        )
        .await
        .map_err(|_| AgentError::SubmissionUnresolved)?
    }

    async fn resume_owed(
        &self,
        turn: OwedTurn,
        actor: ActionContext,
    ) -> Result<Resumed, AgentError> {
        let scheduler = self.inner.scheduler.lock().await;
        let Some(settled) = self.settle_owed(&scheduler, &turn, &actor, false).await? else {
            return Ok(Resumed::NotOwed);
        };
        let SettledTurn {
            record,
            cause,
            continuation,
        } = settled;
        let successor = continuation.resumption.successor();
        let audit = |resumption| {
            ExecutionAuditRecord::TurnResumed(TurnResumptionRecord::new(
                self.inner.manager.id().clone(),
                turn.execution_id.clone(),
                cause,
                dispatched(&record),
                record.actor.clone(),
                actor.clone(),
                resumption,
            ))
        };
        let Some(successor) = successor else {
            drop(scheduler);
            self.record_resumption_audit(audit(TurnResumption::Exhausted))
                .await?;
            return Ok(Resumed::Exhausted);
        };
        let request = successor_request(
            &continuation.original,
            continuation.dispatched,
            cause,
            successor.clone(),
        )?;
        validate_configured_input(self.capabilities(), &request)?;
        let admission = self
            .accept_work_locked(
                request,
                Submitter::new(record.actor.clone(), record.initiation.clone()),
                InvocationKind::Queued,
                NoteCandidates::default(),
                scheduler,
            )
            .await?;
        Ok(
            match self
                .record_resumption_audit(audit(TurnResumption::Continued { successor }))
                .await
            {
                Ok(()) => Resumed::Continued(admission),
                Err(error) => Resumed::ContinuedUnaudited { admission, error },
            },
        )
    }

    /// Leave `turn` ended instead of resuming it, decided by the host's
    /// verified `actor`: a share grant's turn whose grant no longer admits
    /// it. The turn is recorded cancelled with [`SchedulingCause::NotResumed`]
    /// after its owed stop, a cut-off turn's stop being recorded first, so
    /// it is no longer owed and the decision is made once. One
    /// [`TurnResumption::Declined`] record says so. Once polled, an owned
    /// task retains the decision and its audit through caller loss; selection
    /// shares the scheduler lock with resumption admission.
    /// A token from another persistent session is a successful no-op.
    ///
    /// # Errors
    /// What saving the cancellation edges or the audit record returns.
    pub async fn decline_turn(
        &self,
        turn: &OwedTurn,
        actor: ActionContext,
    ) -> Result<(), AgentError> {
        let agent = self.clone();
        let turn = turn.clone();
        let waiter = CallerWaiter::TurnResumption(turn.execution_id.clone());
        contain_caller_wake(
            waiter,
            tokio::spawn(async move { agent.decline_owed(turn, actor).await }),
        )
        .await
        .map_err(|_| AgentError::SubmissionUnresolved)?
    }

    pub(super) async fn decline_owed(
        &self,
        turn: OwedTurn,
        actor: ActionContext,
    ) -> Result<(), AgentError> {
        let scheduler = self.inner.scheduler.lock().await;
        let Some(settled) = self.settle_owed(&scheduler, &turn, &actor, true).await? else {
            return Ok(());
        };
        let SettledTurn { record, cause, .. } = settled;
        drop(scheduler);
        self.record_resumption_audit(ExecutionAuditRecord::TurnResumed(
            TurnResumptionRecord::new(
                self.inner.manager.id().clone(),
                turn.execution_id.clone(),
                cause,
                dispatched(&record),
                record.actor,
                actor,
                TurnResumption::Declined,
            ),
        ))
        .await
    }

    /// Select and settle one owed turn while the caller holds the scheduler
    /// through any successor admission. A decline or spent chain ends it here.
    async fn settle_owed(
        &self,
        scheduler: &Scheduler,
        turn: &OwedTurn,
        actor: &ActionContext,
        decline: bool,
    ) -> Result<Option<SettledTurn>, AgentError> {
        if &turn.session_id != self.inner.manager.id() {
            return Ok(None);
        }
        let active = self.inner.lifecycle.active();
        let live = |id: &ExecutionId| scheduler.has_receipt(id) || active.as_ref() == Some(id);
        let found = self
            .inner
            .manager
            .with_invocations(|records| {
                let lineage = Lineage::new(records);
                let newest_final = records
                    .iter()
                    .rposition(|record| ended_for_good(record, &lineage, live));
                let record = records.get(turn.index)?;
                if record.request.execution_id != turn.execution_id {
                    return None;
                }
                owed(record, &lineage, newest_final > Some(turn.index), live).map(|stop| {
                    (
                        record.clone(),
                        stop,
                        lineage.continuation(records, &turn.execution_id),
                    )
                })
            })
            .await;
        let Some((record, stop, continuation)) = found else {
            return Ok(None);
        };
        let owed_edge = match stop {
            Stop::Recorded(edge) => edge,
            Stop::Unrecorded { edge } => {
                self.inner
                    .manager
                    .record_scheduling(turn.index, edge.clone())
                    .await
                    .map_err(AgentError::Storage)?;
                edge
            }
        };
        if decline || continuation.resumption.successor().is_none() {
            self.inner
                .manager
                .record_scheduling(turn.index, not_resumed(&owed_edge, actor.clone())?)
                .await
                .map_err(AgentError::Storage)?;
        }
        Ok(Some(SettledTurn {
            record,
            cause: owed_edge.cause,
            continuation,
        }))
    }
}

pub(super) mod lineage;
mod progress;
pub use progress::ResumptionHistory;
