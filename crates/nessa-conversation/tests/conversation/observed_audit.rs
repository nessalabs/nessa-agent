//! `ObservedExecutionAudit`: a written record is told as turn telemetry; a
//! record that failed to write is told as nothing.
use super::*;
use nessa_sdk::{
    application::agent_execution::{
        agents::AgentError,
        executions::{QueueAdmissionRecord, QueueSettlementRecord},
        permissions::ActionContext,
    },
    domain::agent_execution::{
        executions::{ExecutionId, InvocationKind, SubmissionMode},
        sessions::SessionId,
    },
};
use nessa_telemetry::{Shape, Sink};
use std::{
    sync::Mutex,
    time::{Duration, SystemTime},
};

#[derive(Default)]
struct Kept(Mutex<Vec<Event>>);
struct Recording(Arc<Kept>);
impl Sink for Recording {
    fn emit(&self, event: &Event) {
        self.0 .0.lock().unwrap().push(event.clone());
    }
}

struct Audit(bool);
impl ExecutionAudit for Audit {
    fn record(&self, _: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        let ok = self.0;
        Box::pin(async move {
            if ok {
                Ok(())
            } else {
                Err(AgentError::AuditFailure)
            }
        })
    }
}

struct Fixed;
impl Clock for Fixed {
    fn unix_seconds(&self) -> u64 {
        7
    }
    fn unix_milliseconds(&self) -> u64 {
        7_000
    }
}

fn records() -> (QueueAdmissionRecord, QueueSettlementRecord) {
    let session = SessionId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let execution = ExecutionId::new("turn-1").unwrap();
    let actor = ActionContext::new("owner", "credential", "request").unwrap();
    (
        QueueAdmissionRecord::submitted(
            session.clone(),
            execution.clone(),
            SubmissionMode::Queued,
            actor.clone(),
        ),
        QueueSettlementRecord::automatic_attachment_failed(
            session,
            execution,
            InvocationKind::Queued,
            None,
            actor,
        )
        .unwrap(),
    )
}

fn observed(written: bool) -> (ObservedExecutionAudit, Telemetry, Arc<Kept>) {
    let kept = Arc::new(Kept::default());
    let telemetry = Telemetry::new(vec![Box::new(Recording(kept.clone()))], 8).unwrap();
    (
        ObservedExecutionAudit::new(Arc::new(Audit(written)), telemetry.clone(), Arc::new(Fixed)),
        telemetry,
        kept,
    )
}

#[tokio::test]
async fn a_written_admission_and_settlement_are_told_inside_the_turn() {
    let (audit, telemetry, kept) = observed(true);
    let (admitted, settled) = records();
    audit
        .record(ExecutionAuditRecord::QueueAdmitted(admitted))
        .await
        .unwrap();
    audit
        .record(ExecutionAuditRecord::QueueSettled(settled))
        .await
        .unwrap();
    assert!(telemetry.flush(Duration::from_secs(5)));
    let events = kept.0.lock().unwrap().clone();
    let span = crate::application::turn_telemetry::turn_span("turn-1");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].name(), "nessa.turn.admitted");
    assert_eq!(
        events[0].shape(),
        &Shape::Point {
            within: Some(span.clone())
        }
    );
    assert_eq!(
        events[0].at(),
        SystemTime::UNIX_EPOCH + Duration::from_millis(7_000)
    );
    assert_eq!(events[1].shape(), &Shape::SpanEnd { span });
    let outcome: Vec<_> = events[1]
        .attributes()
        .iter()
        .map(|(key, value)| (key.to_owned(), value.clone()))
        .collect();
    assert!(outcome.contains(&("nessa.turn.outcome".into(), "settled".into())));
    assert!(outcome.contains(&("nessa.turn.cause".into(), "dispatch_failed".into())));
}

#[tokio::test]
async fn a_failed_write_emits_nothing() {
    let (audit, telemetry, kept) = observed(false);
    let (admitted, _) = records();
    assert!(matches!(
        audit
            .record(ExecutionAuditRecord::QueueAdmitted(admitted))
            .await,
        Err(AgentError::AuditFailure)
    ));
    assert!(telemetry.flush(Duration::from_secs(5)));
    assert!(kept.0.lock().unwrap().is_empty());
}
