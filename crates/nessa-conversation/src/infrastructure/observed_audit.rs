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
use std::sync::Arc;

/// An execution audit whose written records are also emitted as telemetry.
pub struct ObservedExecutionAudit {
    audit: Arc<dyn ExecutionAudit>,
    telemetry: Telemetry,
    clock: Arc<dyn Clock>,
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
        }
    }
}

impl ExecutionAudit for ObservedExecutionAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        let event = if self.telemetry.is_on() {
            turn_event(&record, self.clock.unix_milliseconds())
        } else {
            None
        };
        Box::pin(async move {
            self.audit.record(record).await?;
            if let Some(event) = event {
                self.telemetry.emit(event);
            }
            Ok(())
        })
    }
}

/// The turn event `record` tells, when it tells one.
fn turn_event(record: &ExecutionAuditRecord, unix_ms: u64) -> Option<Event> {
    match record {
        ExecutionAuditRecord::QueueAdmitted(record) => {
            let event = turn_point(
                record.execution_id().as_str(),
                "nessa.turn.admitted",
                unix_ms,
            )
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
            Some(match record.model() {
                Some(model) => event
                    .attr("gen_ai.provider.name", model.name().to_owned())
                    .attr("gen_ai.request.model", model.model_id().to_owned()),
                None => event,
            })
        }
        ExecutionAuditRecord::QueueSettled(record) => {
            let execution = record.execution_id().as_str();
            match record.after() {
                InvocationStage::Queued => None,
                InvocationStage::Running => Some(
                    turn_point(execution, "nessa.turn.dispatched", unix_ms)
                        .attr("nessa.turn.cause", scheduling_cause(record.cause())),
                ),
                stage @ (InvocationStage::Injected
                | InvocationStage::Settled
                | InvocationStage::Cancelled) => Some(turn_ended(
                    execution,
                    invocation_stage(stage),
                    scheduling_cause(record.cause()),
                    unix_ms,
                )),
            }
        }
        // A turn that ran ends here: the provider's terminal result, or the
        // cause that forced it to stop.
        ExecutionAuditRecord::Finished(finish) => Some(match finish.result() {
            Ok(outcome) => turn_ended(
                finish.execution_id().as_str(),
                execution_outcome(outcome),
                "finished",
                unix_ms,
            ),
            Err(reason) => turn_ended(
                finish.execution_id().as_str(),
                "failed",
                cancellation_reason(reason),
                unix_ms,
            ),
        }),
        ExecutionAuditRecord::TurnResumed(record) => match record.resumption() {
            TurnResumption::Continued { successor } => Some(turn_resumed(
                record.session_id().as_str(),
                record.execution_id().as_str(),
                successor.as_str(),
                scheduling_cause(record.cause()),
                unix_ms,
            )),
            TurnResumption::Exhausted | TurnResumption::Declined => None,
        },
        _ => None,
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/observed_audit.rs"]
mod tests;
