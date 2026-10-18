//! Bounded audit delivery through the ACP worker, including spent task budgets.
use super::dispatch_policy::worker_with_ready_frames;
use super::*;
use crate::infrastructure::clock::RuntimeClock;
use std::{
    future::{poll_fn, Future},
    pin::pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
    task::Poll,
};

struct ObservedClock {
    runtime: RuntimeClock,
    deadlines: Mutex<Vec<ClockInstant>>,
}
impl Clock for ObservedClock {
    fn now(&self) -> ClockInstant {
        self.runtime.now()
    }
    fn unix_ms_at(&self, at: ClockInstant) -> u64 {
        self.runtime.unix_ms_at(at)
    }
    fn sleep_until(&self, deadline: ClockInstant) -> ClockSleep {
        self.deadlines.lock().unwrap().push(deadline);
        self.runtime.sleep_until(deadline)
    }
}

#[derive(Default)]
struct BudgetAudit {
    ready: AtomicBool,
    polls: AtomicUsize,
    dropped: AtomicBool,
    records: Mutex<Vec<ExecutionAuditRecord>>,
}
struct AuditDrop<'a>(&'a AtomicBool);
impl Drop for AuditDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
impl ExecutionAudit for BudgetAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        self.records.lock().unwrap().push(record);
        Box::pin(async {
            let _drop = AuditDrop(&self.dropped);
            poll_fn(|cx| {
                // Model a sink draining ready Tokio work. Leave no budget for
                // the deadline's poll, on every poll of this operation.
                while tokio::task::coop::has_budget_remaining() {
                    let mut spend = pin!(tokio::task::consume_budget());
                    assert!(spend.as_mut().poll(cx).is_ready());
                }
                self.polls.fetch_add(1, Ordering::SeqCst);
                if self.ready.load(Ordering::SeqCst) {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Pending
                }
            })
            .await
        })
    }
}

fn finished_record() -> ExecutionAuditRecord {
    let mut controller = ExecutionController::new(ExecutionSessionId::new("context").unwrap());
    let execution = ExecutionId::new("audit-deadline").unwrap();
    controller.begin_execution(execution.clone()).unwrap();
    controller
        .finish_execution(
            &execution,
            Err(PermissionCancellationReason::execution_failed()),
        )
        .unwrap()
        .remove(0)
}

#[tokio::test]
async fn expired_audit_deadline_survives_a_spent_cooperative_budget() {
    let (mut worker, _commands, _close, _events) = worker_with_ready_frames(&[], "").await;
    tokio::time::pause();
    let clock = Arc::new(ObservedClock {
        runtime: RuntimeClock::new(),
        deadlines: Mutex::new(Vec::new()),
    });
    let audit = Arc::new(BudgetAudit::default());
    worker.config.clock = clock.clone();
    worker.audit = audit.clone();
    let grace = worker.config.shutdown_grace;
    assert!(grace > Duration::ZERO);
    let expected = clock.now() + grace;
    // advance() moves time but does not promise the timer driver has delivered
    // expiry. A separate runtime sleep witnesses readiness before the probe.
    let expired_timer = clock.runtime.sleep_until(expected);
    let record = finished_record();
    let mut recording = Box::pin(worker.record_audit(record.clone(), AuditEffectPhase::Lifecycle));
    assert!(poll_fn(|cx| Poll::Ready(recording.as_mut().poll(cx)))
        .await
        .is_pending());
    tokio::time::advance(grace - Duration::from_nanos(1)).await;
    assert!(poll_fn(|cx| Poll::Ready(recording.as_mut().poll(cx)))
        .await
        .is_pending());
    assert!(!audit.dropped.load(Ordering::SeqCst));
    tokio::time::advance(Duration::from_nanos(1)).await;
    tokio::task::unconstrained(expired_timer).await;
    assert!(clock.now() >= expected);
    let expired = poll_fn(|cx| Poll::Ready(recording.as_mut().poll(cx))).await;
    drop(recording);
    // Clean up the real fixture even when the assertion below fails.
    tokio::time::resume();
    worker
        .scope
        .cleanup(Duration::ZERO, Duration::from_secs(2))
        .await
        .unwrap();
    let Poll::Ready(Err(failure)) = expired else {
        panic!("expired ACP audit stayed pending after its operation spent the task budget");
    };
    assert_eq!(failure.error(), &AgentError::AuditFailure);
    assert!(audit.dropped.load(Ordering::SeqCst));
    assert_eq!(audit.polls.load(Ordering::SeqCst), 3);
    assert_eq!(*audit.records.lock().unwrap(), vec![record]);
    assert_eq!(*clock.deadlines.lock().unwrap(), vec![expected]);
}

#[tokio::test]
async fn ready_audit_wins_at_its_deadline_after_spending_the_budget() {
    let (mut worker, _commands, _close, _events) = worker_with_ready_frames(&[], "").await;
    tokio::time::pause();
    let clock = Arc::new(ObservedClock {
        runtime: RuntimeClock::new(),
        deadlines: Mutex::new(Vec::new()),
    });
    let audit = Arc::new(BudgetAudit::default());
    worker.config.clock = clock.clone();
    worker.audit = audit.clone();
    let grace = worker.config.shutdown_grace;
    let expected = clock.now() + grace;
    let expired_timer = clock.runtime.sleep_until(expected);
    let mut recording =
        Box::pin(worker.record_audit(finished_record(), AuditEffectPhase::Lifecycle));
    assert!(poll_fn(|cx| Poll::Ready(recording.as_mut().poll(cx)))
        .await
        .is_pending());
    tokio::time::advance(grace).await;
    tokio::task::unconstrained(expired_timer).await;
    assert!(clock.now() >= expected);
    audit.ready.store(true, Ordering::SeqCst);
    assert!(matches!(
        poll_fn(|cx| Poll::Ready(recording.as_mut().poll(cx))).await,
        Poll::Ready(Ok(()))
    ));
    drop(recording);
    assert!(audit.dropped.load(Ordering::SeqCst));
    assert_eq!(*clock.deadlines.lock().unwrap(), vec![expected]);
    tokio::time::resume();
    worker
        .scope
        .cleanup(Duration::ZERO, Duration::from_secs(2))
        .await
        .unwrap();
}

#[tokio::test]
async fn dropping_the_audit_wait_cancels_its_pending_sink_future() {
    let (mut worker, _commands, _close, _events) = worker_with_ready_frames(&[], "").await;
    let audit = Arc::new(BudgetAudit::default());
    worker.audit = audit.clone();
    let mut recording =
        Box::pin(worker.record_audit(finished_record(), AuditEffectPhase::Lifecycle));
    assert!(poll_fn(|cx| Poll::Ready(recording.as_mut().poll(cx)))
        .await
        .is_pending());
    assert!(!audit.dropped.load(Ordering::SeqCst));
    drop(recording);
    assert!(audit.dropped.load(Ordering::SeqCst));
    assert_eq!(audit.polls.load(Ordering::SeqCst), 1);
    worker
        .scope
        .cleanup(Duration::ZERO, Duration::from_secs(2))
        .await
        .unwrap();
}
