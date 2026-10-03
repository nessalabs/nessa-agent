//! Public receipt-consumer faults must not abandon independently admitted work.
//!
//! Rows of "Receipt notification faults" in docs/agent_execution/scheduling.md.
use super::*;
use std::{
    pin::Pin,
    task::{Context, Wake, Waker},
};

type ReceiptWait = Pin<Box<dyn Future<Output = Result<ExecutionOutcome, AgentError>> + Send>>;

/// Same-submission retries that join the original receipt. Each is a separate
/// waiting consumer, not another admitted work owner.
const HEALTHY_WAITERS: usize = 16;

struct CallerWake {
    calls: AtomicUsize,
    observed: Mutex<Option<oneshot::Sender<()>>>,
    panic_once: bool,
}
impl Wake for CallerWake {
    fn wake(self: Arc<Self>) {
        let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
        // Signal the consumer task before the fault, with no fixture lock held.
        // The callback never polls its receipt.
        let observed = self.observed.lock().unwrap().take();
        if let Some(observed) = observed {
            let _ = observed.send(());
        }
        if first && self.panic_once {
            panic!("issue431 receipt consumer callback");
        }
    }
}
fn caller_wake(panic_once: bool) -> (Arc<CallerWake>, oneshot::Receiver<()>) {
    let (observed, notification) = oneshot::channel();
    let caller = Arc::new(CallerWake {
        calls: AtomicUsize::new(0),
        observed: Mutex::new(Some(observed)),
        panic_once,
    });
    (caller, notification)
}

struct HealthyCallerWake(AtomicUsize);
impl Wake for HealthyCallerWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn register(wait: &mut ReceiptWait, waker: Waker) {
    assert!(wait
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
}

#[tokio::test]
async fn panicking_receipt_consumer_preserves_independent_queued_work() {
    // The non-panicking pass is the positive counterpart of the same journey.
    for panic_consumer in [false, true] {
        let bound = Duration::from_secs(3);
        let (agent, backend, storage) = probe(false).await;
        let (release, gate) = oneshot::channel();
        *backend.execution_gate.lock().unwrap() = Some(gate);
        let original = agent.enqueue(input("original"), actor()).await.unwrap();
        timeout(bound, backend.executing.notified()).await.unwrap();
        let tail = agent
            .enqueue(input("independent-tail"), actor())
            .await
            .unwrap();
        let mut retries = Vec::new();
        for _ in 0..HEALTHY_WAITERS {
            let retry = agent.enqueue(input("original"), actor()).await.unwrap();
            assert_eq!(original.id(), retry.id());
            retries.push(retry);
        }
        assert_eq!(tail.id().as_str(), "independent-tail");

        let (caller, notification) = caller_wake(panic_consumer);
        let mut original: ReceiptWait = Box::pin(original.wait());
        register(&mut original, Waker::from(caller.clone()));
        let mut healthy = Vec::new();
        for retry in retries {
            let counter = Arc::new(HealthyCallerWake(AtomicUsize::new(0)));
            let mut wait: ReceiptWait = Box::pin(retry.wait());
            register(&mut wait, Waker::from(counter.clone()));
            healthy.push((counter, wait));
        }
        release.send(()).unwrap();
        // Do not await any receipt before this witness: that would replace its
        // custom caller registration before publication.
        timeout(bound, notification).await.unwrap().unwrap();
        let tail_result = timeout(bound, tail.wait()).await;
        let dispatches = backend.executions.load(Ordering::SeqCst);
        // The tail settled after the original's publication returned, so every
        // registered sibling has been notified by now if it ever will be.
        let unwoken = healthy
            .iter()
            .filter(|(counter, _)| counter.0.load(Ordering::SeqCst) == 0)
            .count();
        let original_result = timeout(bound, original).await;
        let mut retry_results = Vec::new();
        for (_, wait) in healthy {
            retry_results.push(timeout(bound, wait).await);
        }
        let saved = storage.snapshot();
        // Cleanup occurs after the public liveness observation; closing cannot
        // manufacture a successful tail dispatch or alter the captured result.
        let close_result = timeout(bound, agent.close(actor())).await;
        eprintln!(
            "issue431 public witness: panic_consumer={panic_consumer}, caller_wakes={}, \
             unwoken_siblings={unwoken}, dispatches={dispatches}, tail_result={tail_result:?}",
            caller.calls.load(Ordering::SeqCst)
        );

        assert_eq!(
            caller.calls.load(Ordering::SeqCst),
            1,
            "the original registered caller must be notified"
        );
        assert_eq!(
            dispatches, 2,
            "an independently admitted tail must dispatch after another receipt consumer panics"
        );
        assert_eq!(
            tail_result.expect("the independent tail must settle"),
            Ok(ExecutionOutcome::Completed)
        );
        assert_eq!(
            unwoken, 0,
            "every registered sibling waiter must be notified despite one consumer's panic"
        );
        assert_eq!(original_result.unwrap(), Ok(ExecutionOutcome::Completed));
        for retry_result in retry_results {
            assert_eq!(retry_result.unwrap(), Ok(ExecutionOutcome::Completed));
        }
        close_result.unwrap().unwrap();
        assert_eq!(saved.invocations.len(), 2);
        for (record, id) in saved
            .invocations
            .iter()
            .zip(["original", "independent-tail"])
        {
            assert_eq!(record.request.execution_id.as_str(), id);
            assert_eq!(record.result, Some(Ok(ExecutionOutcome::Completed)));
            assert_eq!(
                record.scheduling.last().unwrap().stage,
                InvocationStage::Settled
            );
        }
    }
}

#[tokio::test]
async fn panicking_receipt_consumer_does_not_fail_its_withdrawal() {
    for panic_consumer in [false, true] {
        let bound = Duration::from_secs(3);
        let (agent, backend, storage) = probe(false).await;
        let (release, gate) = oneshot::channel();
        *backend.execution_gate.lock().unwrap() = Some(gate);
        let active = agent.enqueue(input("active"), actor()).await.unwrap();
        timeout(bound, backend.executing.notified()).await.unwrap();
        let withdrawn = agent.enqueue(input("withdrawn"), actor()).await.unwrap();
        let (caller, notification) = caller_wake(panic_consumer);
        let mut withdrawn: ReceiptWait = Box::pin(withdrawn.wait());
        register(&mut withdrawn, Waker::from(caller.clone()));

        let removal = timeout(
            bound,
            agent.remove_queued(ExecutionId::new("withdrawn").unwrap(), actor()),
        )
        .await
        .unwrap();
        timeout(bound, notification).await.unwrap().unwrap();
        assert_eq!(
            removal,
            Ok(QueueRemoval::Removed),
            "a consumer's wake panic must not turn an applied withdrawal into an error"
        );
        assert_eq!(caller.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            timeout(bound, withdrawn).await.unwrap(),
            Err(AgentError::Closed)
        );
        release.send(()).unwrap();
        assert_eq!(
            timeout(bound, active.wait()).await.unwrap(),
            Ok(ExecutionOutcome::Completed)
        );
        assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
        let saved = storage.snapshot();
        let record = saved
            .invocations
            .iter()
            .find(|record| record.request.execution_id.as_str() == "withdrawn")
            .unwrap();
        let last = record.scheduling.last().unwrap();
        assert_eq!(last.stage, InvocationStage::Cancelled);
        assert_eq!(last.cause, SchedulingCause::Withdrawn);
        timeout(bound, agent.close(actor())).await.unwrap().unwrap();
    }
}
