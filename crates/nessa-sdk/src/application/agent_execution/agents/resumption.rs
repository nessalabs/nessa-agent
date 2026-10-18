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
//! running it. In each case no successor has been saved for it yet. Nessa never re-runs a tool: the
//! successor gives the model a notice and the original request, and the model
//! checks what is done and carries on, or says it is finished.
#![deny(missing_docs)]

use super::{Agent, AgentError, QueueAdmission};
use crate::application::agent_execution::{
    executions::{
        ExecutionAuditRecord, ExecutionRequest, Submitter, TurnResumption, TurnResumptionRecord,
    },
    permissions::ActionContext,
    sessions::{InvocationRecord, InvocationSchedulingEvent, NoteCandidates, StorageError},
};
use crate::domain::agent_execution::{
    executions::{
        ExecutionId, ExecutionOutcome, Initiation, InvocationKind, InvocationStage, Resumption,
        SchedulingCause, SchedulingInitiator, SchedulingTransition, SubmissionMode,
    },
    prompts::{PromptText, UserMessage},
};
use std::collections::{HashMap, HashSet};

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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwedTurn {
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
    /// The turn was cut off [`Resumption::MAX_RESUMES`] times running and is
    /// left ended.
    Exhausted,
    /// The turn is no longer owed: it was resumed already, or is running here.
    NotOwed,
}

/// The exact canonical successor already saved for `execution`, if any.
/// The SDK owner's saved resumption admission, forward queued admission,
/// a recorded owed stop, identity, caller,
/// initiation and the continuation request must agree. This
/// reads a relationship, not runtime completion or permission to resume; later
/// person input cannot erase an acknowledged successor. Use [`owed_turn_ids`]
/// for historical resumption eligibility and [`Agent::unsettled_execution_ids`]
/// for unfinished work admitted by a live Agent.
pub fn resumed_turn_id(
    history: &[InvocationRecord],
    execution: &ExecutionId,
) -> Option<ExecutionId> {
    let positions = positions(history);
    canonical_successor(history, *positions.get(execution)?, &positions).cloned()
}

fn positions(history: &[InvocationRecord]) -> HashMap<&ExecutionId, usize> {
    history
        .iter()
        .enumerate()
        .map(|(index, record)| (&record.request.execution_id, index))
        .collect()
}

/// The same saved relationship decides observation and owed supersession.
/// Inferring a cut-off is enough to propose a resume, never to prove that its
/// successor was admitted: resume records that stop before saving queued work.
fn canonical_successor<'a>(
    history: &'a [InvocationRecord],
    index: usize,
    positions: &HashMap<&ExecutionId, usize>,
) -> Option<&'a ExecutionId> {
    let record = &history[index];
    let execution = &record.request.execution_id;
    let next = Resumption::of(execution).successor()?;
    let successor_index = *positions.get(&next)?;
    if successor_index <= index {
        return None;
    }
    let Stop::Recorded(edge) = owed(record, false, false, |_| false)? else {
        return None;
    };
    let successor = &history[successor_index];
    matches_resumption_admission(record, successor, edge.cause)
        .then_some(&successor.request.execution_id)
}

fn matches_resumption_admission(
    source: &InvocationRecord,
    successor: &InvocationRecord,
    cause: SchedulingCause,
) -> bool {
    successor.resumed_from.as_ref() == Some(&source.request.execution_id)
        && successor.submission == SubmissionMode::Queued
        && successor.actor == source.actor
        && successor.initiation == source.initiation
        && Resumption::of(&source.request.execution_id)
            .successor()
            .is_some_and(|next| {
                successor_request(source, cause, next)
                    .is_ok_and(|request| request == successor.request)
            })
}

/// Validate the retained admission fact, independently of whether its source
/// is still owed today. The persistence owner supplies only a preceding source.
pub(crate) fn validate_saved_resumption(
    successor: &InvocationRecord,
    source: Option<&InvocationRecord>,
) -> Result<(), StorageError> {
    if successor.resumed_from.is_none() {
        return Ok(());
    }
    if source.is_some_and(|source| {
        source.submission != SubmissionMode::Immediate
            && source.scheduling.iter().any(|edge| {
                recorded_resume_stop(source, edge, false)
                    && matches_resumption_admission(source, successor, edge.cause)
            })
    }) {
        Ok(())
    } else {
        Err(StorageError::Corrupt(
            "resumption origin does not match a preceding resumable admission".into(),
        ))
    }
}

/// Exact saved turns whose work is owed when no local execution owns it.
/// This is the historical counterpart of [`Agent::owed_turns`], sharing its
/// supersession rules. It grants no permission or instruction to resume. An
/// exhausted chain remains owed until the host declines it; a running final
/// attempt is therefore still an exact Stop target.
pub fn owed_turn_ids(history: &[InvocationRecord]) -> Vec<ExecutionId> {
    let read = OwedHistory::new(history, |_| false);
    (0..history.len())
        .filter(|index| read.at(*index).is_some())
        .map(|index| history[index].request.execution_id.clone())
        .collect()
}

struct OwedHistory<'a, F> {
    history: &'a [InvocationRecord],
    resumed: HashSet<&'a ExecutionId>,
    newest_final: Option<usize>,
    newest_message: Option<usize>,
    live: F,
}
impl<'a, F: Fn(&ExecutionId) -> bool> OwedHistory<'a, F> {
    fn new(history: &'a [InvocationRecord], live: F) -> Self {
        let positions = positions(history);
        let resumed = history
            .iter()
            .enumerate()
            .filter(|(index, _)| canonical_successor(history, *index, &positions).is_some())
            .map(|(_, record)| &record.request.execution_id)
            .collect();
        let newest_final = history
            .iter()
            .rposition(|record| ended_for_good(record, &resumed, &live));
        let newest_message = history.iter().rposition(|record| {
            record.submission != SubmissionMode::Immediate && record.resumed_from.is_none()
        });
        Self {
            history,
            resumed,
            newest_final,
            newest_message,
            live,
        }
    }
    fn at(&self, index: usize) -> Option<Stop> {
        let record = &self.history[index];
        let retry_moved_past = record.resumed_from.is_some() && self.newest_message > Some(index);
        owed(
            record,
            self.resumed.contains(&record.request.execution_id),
            self.newest_final > Some(index) || retry_moved_past,
            &self.live,
        )
    }
    fn turns(&self) -> Vec<OwedTurn> {
        self.history
            .iter()
            .enumerate()
            .filter_map(|(index, record)| {
                self.at(index).map(|_| OwedTurn {
                    index,
                    execution_id: record.request.execution_id.clone(),
                    submitted_by: record.actor.clone(),
                    initiation: record.initiation.clone(),
                    dispatched: dispatched(record),
                })
            })
            .collect()
    }
}

/// How an owed turn stands in saved history.
enum Stop {
    /// Its last edge is this owed cancellation, or this failure.
    Recorded(InvocationSchedulingEvent),
    /// No result and still Queued or Running: the process that ran it ended.
    /// Resuming records the cancellation first, with this cause and caller.
    CutOff { edge: InvocationSchedulingEvent },
}

/// Whether `record` is owed, and how it stopped. `resumed` means the SDK's
/// canonical relationship is saved; `superseded` whether a later turn ended
/// for good or is this process's own work (`OwedHistory`), or, for a successor, whether any
/// message was saved after it ([`Agent::owed_turns`]); `live` whether this
/// process admitted or is running `id`. A failure is owed only while nothing
/// came after it: a person who sent the next message moved on, and so did one
/// who stopped it.
fn owed(
    record: &InvocationRecord,
    resumed: bool,
    superseded: bool,
    live: impl Fn(&ExecutionId) -> bool,
) -> Option<Stop> {
    let id = &record.request.execution_id;
    if record.submission == SubmissionMode::Immediate || resumed {
        return None;
    }
    let last = record.scheduling.last()?;
    match last.stage {
        InvocationStage::Cancelled | InvocationStage::Settled
            if recorded_resume_stop(record, last, superseded) =>
        {
            Some(Stop::Recorded(last.clone()))
        }
        // Queued or Running here is this process's own work, not owed.
        InvocationStage::Queued | InvocationStage::Running
            if record.result.is_none() && !live(id) =>
        {
            // A stop already captured keeps its own cause: one the host's
            // shutdown started is owed, and is recorded as that; anyone
            // else's leaves the turn as it is.
            let (cause, actor) = match &record.local_cancellation {
                Some(stop)
                    if stop.cause.leaves_work_owed()
                        || (stop.cause == SchedulingCause::RunnerStopped
                            && !superseded
                            && failure_owed(record)) =>
                {
                    (stop.cause, stop.actor.clone())
                }
                Some(_) => return None,
                None => (SchedulingCause::Interrupted, None),
            };
            let transition = SchedulingTransition::new(
                last.kind,
                last.target.clone(),
                Some(last.stage),
                InvocationStage::Cancelled,
                cause,
                if actor.is_some() {
                    SchedulingInitiator::Caller
                } else {
                    SchedulingInitiator::Automatic
                },
            )
            .ok()?;
            Some(Stop::CutOff {
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
        _ => None,
    }
}

fn recorded_resume_stop(
    record: &InvocationRecord,
    edge: &InvocationSchedulingEvent,
    superseded: bool,
) -> bool {
    (edge.stage == InvocationStage::Cancelled && edge.cause.leaves_work_owed())
        || (matches!(
            edge.stage,
            InvocationStage::Cancelled | InvocationStage::Settled
        ) && edge.cause.stopped_by_failure()
            && !superseded
            && failure_owed(record))
}

/// Whether `record` is a turn that moved past every failure before it: one
/// that ran, however it ended, or that ended for good or this process
/// admitted (it is not owed and was not resumed). A turn that only waited
/// behind a failure and was stopped with it has not. A steering message
/// belongs to the turn it steered, and a successor carries on the host's
/// resume rather than a person's next step, so neither counts.
fn ended_for_good(
    record: &InvocationRecord,
    resumed: &HashSet<&ExecutionId>,
    live: impl Fn(&ExecutionId) -> bool,
) -> bool {
    record.submission != SubmissionMode::Immediate
        && record.resumed_from.is_none()
        && (dispatched(record)
            || (!resumed.contains(&record.request.execution_id)
                && owed(record, false, false, live).is_none()))
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
    record: &InvocationRecord,
    cause: SchedulingCause,
    successor: ExecutionId,
) -> Result<ExecutionRequest, AgentError> {
    let original = &record.request.user_message;
    // A successor's own text opens with its notice: the next one replaces
    // it rather than stacking on it, and says the provider had the request
    // when an earlier turn in the chain reached it.
    let mut request = original.text_str();
    let mut dispatched = dispatched(record);
    while let Some((rest, reached)) = [
        (CUT_OFF_NOTICE, true),
        (FAILED_NOTICE, true),
        (UNSENT_NOTICE, false),
        (HELD_NOTICE, false),
    ]
    .into_iter()
    .find_map(|(notice, reached)| request.strip_prefix(notice).map(|rest| (rest, reached)))
    {
        request = rest;
        dispatched |= reached;
    }
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
        estimated_input_tokens: record.request.estimated_input_tokens,
        reserved_output_tokens: record.request.reserved_output_tokens,
    })
}

impl Agent {
    /// Every turn in this conversation's saved history whose work is owed —
    /// cut off by a restart, stopped by the host's own shutdown
    /// ([`Self::suspend`]), or ended by a failure a retry can fix — and not
    /// yet resumed, in saved order. Turns this Agent has queued or is running
    /// are never owed; one it ended with such a failure is. Read-only:
    /// nothing is recorded until [`Self::resume_turn`].
    pub async fn owed_turns(&self) -> Vec<OwedTurn> {
        let scheduler = self.inner.scheduler.lock().await;
        let active = self.inner.lifecycle.active();
        let live = |id: &ExecutionId| scheduler.has_receipt(id) || active.as_ref() == Some(id);
        self.inner
            .manager
            .read_invocations(|history| OwedHistory::new(history, live).turns())
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
        let scheduler = self.inner.scheduler.lock().await;
        scheduler.has_no_work() && self.inner.invocation.try_lock().is_ok()
    }

    /// Continue `turn` with a successor turn, resumed by the host's verified
    /// `actor`.
    ///
    /// A turn cut off with no result is first recorded cancelled with
    /// [`SchedulingCause::Interrupted`]. The successor is then queued as an
    /// ordinary turn under the original caller and initiation: a notice that
    /// the turn was cut off, and the original request, so the model checks
    /// what is already done and carries on or says it is finished. Its
    /// identity is derived from `turn` ([`Resumption`]), so resuming the same
    /// turn again answers [`Resumed::NotOwed`] and queues nothing more
    /// (`resume_twice_queues_one_successor`). A turn whose chain of resumes is
    /// spent is recorded cancelled with [`SchedulingCause::NotResumed`] and left
    /// ended ([`Resumed::Exhausted`]). Either way one
    /// [`ExecutionAuditRecord::TurnResumed`] records it.
    ///
    /// The host decides who may resume a turn before calling this; the SDK
    /// does not re-check a share grant.
    ///
    /// # Errors
    /// What saving the cancellation edge returns, and what
    /// [`Self::enqueue`] returns for the successor. An unrelated saved input
    /// occupying the derived successor ID returns [`AgentError::SubmissionConflict`]
    /// before the source is changed; [`Self::decline_turn`] remains available.
    pub async fn resume_turn(
        &self,
        turn: &OwedTurn,
        actor: ActionContext,
    ) -> Result<Resumed, AgentError> {
        let successor = Resumption::of(&turn.execution_id).successor();
        // A spent chain is ended by this decision, so it is no longer owed.
        let end = successor.is_none().then(|| actor.clone());
        let Some((record, cause)) = self.settle_owed(turn, end).await? else {
            return Ok(Resumed::NotOwed);
        };
        let audit = |resumption| {
            ExecutionAuditRecord::TurnResumed(TurnResumptionRecord::new(
                self.inner.manager.id().clone(),
                turn.execution_id.clone(),
                cause,
                turn.dispatched,
                record.actor.clone(),
                actor.clone(),
                resumption,
            ))
        };
        let Some(successor) = successor else {
            self.record_resumption_audit(audit(TurnResumption::Exhausted))
                .await?;
            return Ok(Resumed::Exhausted);
        };
        let request = successor_request(&record, cause, successor.clone())?;
        let admission = self
            .accept_work(
                request,
                Submitter::new(record.actor.clone(), record.initiation.clone()),
                InvocationKind::Queued,
                NoteCandidates::default(),
                Some(turn.execution_id.clone()),
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
    /// [`TurnResumption::Declined`] record says so.
    ///
    /// # Errors
    /// What saving the cancellation edges or the audit record returns.
    pub async fn decline_turn(
        &self,
        turn: &OwedTurn,
        actor: ActionContext,
    ) -> Result<(), AgentError> {
        let Some((record, cause)) = self.settle_owed(turn, Some(actor.clone())).await? else {
            return Ok(());
        };
        self.record_resumption_audit(ExecutionAuditRecord::TurnResumed(
            TurnResumptionRecord::new(
                self.inner.manager.id().clone(),
                turn.execution_id.clone(),
                cause,
                turn.dispatched,
                record.actor,
                actor,
                TurnResumption::Declined,
            ),
        ))
        .await
    }

    /// Find `turn` if it is still owed and record its owed stop if it was
    /// cut off; with `end`, also record that `end` decided not to resume it.
    /// All under the scheduler lock, so two callers never both decide one
    /// turn. Answers the turn's record and why its work was owed.
    async fn settle_owed(
        &self,
        turn: &OwedTurn,
        end: Option<ActionContext>,
    ) -> Result<Option<(InvocationRecord, SchedulingCause)>, AgentError> {
        let scheduler = self.inner.scheduler.lock().await;
        let active = self.inner.lifecycle.active();
        let live = |id: &ExecutionId| scheduler.has_receipt(id) || active.as_ref() == Some(id);
        let found = self
            .inner
            .manager
            .read_invocations(|history| {
                let read = OwedHistory::new(history, live);
                let found = history
                    .get(turn.index)
                    .filter(|record| record.request.execution_id == turn.execution_id)
                    .and_then(|record| read.at(turn.index).map(|stop| (record.clone(), stop)));
                if found.is_some()
                    && end.is_none()
                    && Resumption::of(&turn.execution_id)
                        .successor()
                        .is_some_and(|next| {
                            history
                                .iter()
                                .any(|record| record.request.execution_id == next)
                        })
                {
                    // A canonical successor made this turn NotOwed above. An
                    // occupied noncanonical ID cannot be adopted by first
                    // recording an owed stop. Decline remains available.
                    return Err(AgentError::SubmissionConflict);
                }
                Ok(found)
            })
            .await?;
        let Some((record, stop)) = found else {
            return Ok(None);
        };
        let owed_edge = match stop {
            Stop::Recorded(edge) => edge,
            Stop::CutOff { edge } => {
                self.inner
                    .manager
                    .record_scheduling(turn.index, edge.clone())
                    .await
                    .map_err(AgentError::Storage)?;
                edge
            }
        };
        if let Some(actor) = end {
            self.inner
                .manager
                .record_scheduling(turn.index, not_resumed(&owed_edge, actor)?)
                .await
                .map_err(AgentError::Storage)?;
        }
        drop(scheduler);
        Ok(Some((record, owed_edge.cause)))
    }
}
