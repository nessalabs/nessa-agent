//! The execution audit, also told as turn telemetry.
//!
//! Wraps the durable execution audit. Each record is written first; only once
//! the write succeeded is the turn event it tells emitted, so telemetry never
//! says something happened that the audit does not hold. Telemetry is lossy
//! and the audit is not: a failed write fails exactly as it would unwrapped,
//! and emits nothing (`a_failed_write_emits_nothing`).
//!
//! ```text
//! SDK ──record──▶ ObservedExecutionAudit ──record──▶ durable audit
//!                          │ written
//!                          └──emit──▶ Telemetry (turn_telemetry's events)
//! ```
use super::audit_mapping::{
    cancellation_reason, execution_outcome, invocation_stage, scheduling_cause, submission_mode,
};
use crate::application::turn_telemetry::{turn_ended, turn_point, turn_resumed};
use nessa_auth::application::ports::Clock;
use nessa_sdk::{
    application::agent_execution::{
        agents::AgentFuture,
        executions::{ExecutionAudit, ExecutionAuditRecord, TurnResumption},
    },
    domain::agent_execution::executions::InvocationStage,
};
use nessa_telemetry::{Event, Telemetry};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// Most turns whose conversation is remembered at once, for their ends.
const MOST_REMEMBERED: usize = 4096;

/// An execution audit whose written records are also emitted as telemetry.
pub struct ObservedExecutionAudit {
    audit: Arc<dyn ExecutionAudit>,
    telemetry: Telemetry,
    clock: Arc<dyn Clock>,
    admitted: Mutex<Admitted>,
}

/// Which conversation admitted each running execution. `Finished` names the
/// provider's session, not the conversation, and execution ids are unique
/// only within a conversation, so a turn's end is matched to the admission
/// that named it. Bounded: past [`MOST_REMEMBERED`] the oldest is forgotten,
/// and its end is not told (telemetry may be lost; the audit is not).
#[derive(Default)]
struct Admitted {
    /// Execution id to the conversations that admitted it, each with the
    /// order it was remembered in.
    by_execution: HashMap<String, Vec<(String, u64)>>,
    order: VecDeque<(String, String, u64)>,
    next: u64,
}

impl Admitted {
    fn remember(&mut self, conversation: &str, execution: &str) {
        if self.order.len() >= MOST_REMEMBERED {
            if let Some((execution, conversation, seq)) = self.order.pop_front() {
                self.forget_exactly(&execution, &conversation, Some(seq));
            }
        }
        let seq = self.next;
        self.next += 1;
        let held = self.by_execution.entry(execution.to_owned()).or_default();
        held.retain(|(held, _)| held != conversation);
        held.push((conversation.to_owned(), seq));
        self.order
            .push_back((execution.to_owned(), conversation.to_owned(), seq));
    }

    fn forget_exactly(&mut self, execution: &str, conversation: &str, seq: Option<u64>) {
        if let Some(held) = self.by_execution.get_mut(execution) {
            held.retain(|(held, at)| held != conversation || seq.is_some_and(|seq| seq != *at));
            if held.is_empty() {
                self.by_execution.remove(execution);
            }
        }
    }

    fn forget(&mut self, conversation: &str, execution: &str) {
        self.forget_exactly(execution, conversation, None);
    }

    /// The one conversation running `execution`, taken out. Two
    /// conversations running the same id cannot be told apart, so neither is
    /// ended rather than the wrong one.
    fn finished(&mut self, execution: &str) -> Option<String> {
        let held = self.by_execution.get(execution)?;
        let [(conversation, _)] = held.as_slice() else {
            return None;
        };
        let conversation = conversation.clone();
        self.by_execution.remove(execution);
        Some(conversation)
    }
}

impl ObservedExecutionAudit {
    /// Write to `audit`; tell what was written to `telemetry`, at times read
    /// from `clock`.
    pub fn new(
        audit: Arc<dyn ExecutionAudit>,
        telemetry: Telemetry,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            audit,
            telemetry,
            clock,
            admitted: Mutex::new(Admitted::default()),
        }
    }

    /// Tell what the written `told` says, keeping track of which
    /// conversation each running turn belongs to.
    fn tell(&self, told: Told) {
        let Ok(mut admitted) = self.admitted.lock() else {
            return;
        };
        let event = match told {
            Told::Admitted {
                conversation,
                execution,
                event,
            } => {
                admitted.remember(&conversation, &execution);
                event
            }
            Told::Ended {
                conversation,
                execution,
                event,
            } => {
                admitted.forget(&conversation, &execution);
                event
            }
            Told::Finished {
                execution,
                outcome,
                cause,
                unix_ms,
            } => {
                let Some(conversation) = admitted.finished(&execution) else {
                    return;
                };
                turn_ended(&conversation, &execution, outcome, cause, unix_ms)
            }
            Told::Other(event) => event,
        };
        drop(admitted);
        self.telemetry.emit(event);
    }
}

impl ExecutionAudit for ObservedExecutionAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        let told = if self.telemetry.is_on() {
            told(&record, self.clock.unix_milliseconds())
        } else {
            None
        };
        Box::pin(async move {
            self.audit.record(record).await?;
            if let Some(told) = told {
                self.tell(told);
            }
            Ok(())
        })
    }
}

/// What a record tells, before the conversation of a finished turn is known.
enum Told {
    Admitted {
        conversation: String,
        execution: String,
        event: Event,
    },
    Ended {
        conversation: String,
        execution: String,
        event: Event,
    },
    Finished {
        execution: String,
        outcome: &'static str,
        cause: &'static str,
        unix_ms: u64,
    },
    Other(Event),
}

/// The turn event `record` tells, when it tells one.
fn told(record: &ExecutionAuditRecord, unix_ms: u64) -> Option<Told> {
    match record {
        ExecutionAuditRecord::QueueAdmitted(record) => {
            let conversation = record.session_id().as_str();
            let execution = record.execution_id().as_str();
            let event = turn_point(conversation, execution, "nessa.turn.admitted", unix_ms)
                .attr("nessa.turn.mode", submission_mode(record.mode()))
                .attr_some(
                    "nessa.turn.approval_mode",
                    record.approval_mode().map(|mode| mode.as_str()),
                )
                .attr_some(
                    "nessa.turn.effort",
                    record
                        .effort_level()
                        .map(|effort| effort.as_str().to_owned()),
                );
            let event = match record.model() {
                Some(model) => event
                    .attr("gen_ai.provider.name", model.name().to_owned())
                    .attr("gen_ai.request.model", model.model_id().to_owned()),
                None => event,
            };
            Some(Told::Admitted {
                conversation: conversation.to_owned(),
                execution: execution.to_owned(),
                event,
            })
        }
        ExecutionAuditRecord::QueueSettled(record) => {
            let conversation = record.session_id().as_str();
            let execution = record.execution_id().as_str();
            match record.after() {
                InvocationStage::Queued => None,
                InvocationStage::Running => Some(Told::Other(
                    turn_point(conversation, execution, "nessa.turn.dispatched", unix_ms)
                        .attr("nessa.turn.cause", scheduling_cause(record.cause())),
                )),
                stage @ (InvocationStage::Injected
                | InvocationStage::Settled
                | InvocationStage::Cancelled) => Some(Told::Ended {
                    conversation: conversation.to_owned(),
                    execution: execution.to_owned(),
                    event: turn_ended(
                        conversation,
                        execution,
                        invocation_stage(stage),
                        scheduling_cause(record.cause()),
                        unix_ms,
                    ),
                }),
            }
        }
        // A turn that ran ends here: the provider's terminal result, or the
        // cause that forced it to stop.
        ExecutionAuditRecord::Finished(finish) => {
            let (outcome, cause) = match finish.result() {
                Ok(outcome) => (execution_outcome(outcome), "finished"),
                Err(reason) => ("failed", cancellation_reason(reason)),
            };
            Some(Told::Finished {
                execution: finish.execution_id().as_str().to_owned(),
                outcome,
                cause,
                unix_ms,
            })
        }
        ExecutionAuditRecord::TurnResumed(record) => match record.resumption() {
            TurnResumption::Continued { successor } => Some(Told::Other(turn_resumed(
                record.session_id().as_str(),
                record.execution_id().as_str(),
                successor.as_str(),
                scheduling_cause(record.cause()),
                unix_ms,
            ))),
            TurnResumption::Exhausted | TurnResumption::Declined => None,
        },
        _ => None,
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/observed_audit.rs"]
mod tests;
