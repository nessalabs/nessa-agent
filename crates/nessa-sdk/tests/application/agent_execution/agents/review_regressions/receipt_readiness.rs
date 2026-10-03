//! Original receipt visibility follows ownership retirement, not executor timing.
use super::*;
use nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock;
use std::{
    pin::Pin,
    task::{Context, Wake, Waker},
};

type InvocationWait = Pin<Box<dyn Future<Output = Result<ExecutionOutcome, AgentError>> + Send>>;

struct ReceiptWake {
    next: Mutex<Option<InvocationWait>>,
    first_poll: Mutex<Option<Poll<Result<ExecutionOutcome, AgentError>>>>,
    outer: Mutex<Option<Waker>>,
}
impl ReceiptWake {
    fn probe(&self) {
        let mut first = self.first_poll.lock().unwrap();
        if first.is_some() {
            return;
        }
        // Probe only a separately owned public invoke. The receipt's watch
        // publication may still hold its lock while invoking this consumer.
        let mut next = self.next.lock().unwrap();
        let mut context = Context::from_waker(Waker::noop());
        *first = Some(next.as_mut().unwrap().as_mut().poll(&mut context));
    }
    fn outcome(&self) -> Poll<Result<ExecutionOutcome, AgentError>> {
        self.first_poll
            .lock()
            .unwrap()
            .clone()
            .expect("original receipt must notify its registered consumer")
    }
    fn take_next(&self) -> InvocationWait {
        self.next.lock().unwrap().take().unwrap()
    }
    fn notify(&self) {
        self.probe();
        let outer = self.outer.lock().unwrap().clone();
        if let Some(outer) = outer {
            outer.wake();
        }
    }
}
impl Wake for ReceiptWake {
    fn wake(self: Arc<Self>) {
        self.notify();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.notify();
    }
}

fn register_receipt(receipt: QueueAdmission, agent: Agent) -> (InvocationWait, Arc<ReceiptWake>) {
    let probe = Arc::new(ReceiptWake {
        next: Mutex::new(Some(Box::pin(async move {
            agent.invoke(input("after-receipt"), actor()).await
        }))),
        first_poll: Mutex::new(None),
        outer: Mutex::new(None),
    });
    let waker = Waker::from(probe.clone());
    let mut receipt: InvocationWait = Box::pin(receipt.wait());
    assert!(receipt
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
    let consumer = probe.clone();
    let receipt: InvocationWait = Box::pin(poll_fn(move |context| {
        // Awaiting the raw receipt would replace the custom watch registration.
        // Retain it on every poll, forwarding to the actual waiter only after
        // notification probes the separately owned invocation.
        *consumer.outer.lock().unwrap() = Some(context.waker().clone());
        receipt.as_mut().poll(&mut Context::from_waker(&waker))
    }));
    (receipt, probe)
}

#[tokio::test]
async fn completed_queue_receipt_admits_immediate_invocation_without_stale_busy() {
    for steering in [false, true] {
        let (agent, backend, storage) = probe(false).await;
        let (release, gate) = oneshot::channel();
        *backend.execution_gate.lock().unwrap() = Some(gate);
        let receipt = if steering {
            agent.enqueue_steering(input("original"), actor()).await
        } else {
            agent.enqueue(input("original"), actor()).await
        }
        .unwrap();
        assert_eq!(receipt.id().as_str(), "original");
        timeout(Duration::from_secs(5), backend.executing.notified())
            .await
            .unwrap();
        let (receipt, probe) = register_receipt(receipt, agent.clone());
        release.send(()).unwrap();
        assert_eq!(
            timeout(Duration::from_secs(5), receipt).await.unwrap(),
            Ok(ExecutionOutcome::Completed)
        );
        assert_eq!(
            probe.outcome(),
            Poll::Pending,
            "the original receipt must retire its own invocation slot before waking its consumer"
        );
        assert_eq!(
            timeout(Duration::from_secs(5), probe.take_next())
                .await
                .unwrap(),
            Ok(ExecutionOutcome::Completed)
        );
        assert_eq!(backend.executions.load(Ordering::SeqCst), 2);
        let saved = storage.snapshot();
        assert_eq!(saved.invocations.len(), 2);
        assert_eq!(
            saved.invocations[0].request.execution_id.as_str(),
            "original"
        );
        assert_eq!(
            saved.invocations[0].result,
            Some(Ok(ExecutionOutcome::Completed))
        );
        assert_eq!(
            saved.invocations[1].request.execution_id.as_str(),
            "after-receipt"
        );
        agent.close(actor()).await.unwrap();
    }
}

#[tokio::test]
async fn completed_queue_receipt_retires_work_after_confirmed_attachment_cleanup() {
    let (agent, backend, storage) = probe(false).await;
    *backend.execution_attachment.lock().unwrap() = ProviderSessionState::CleanupRequired;
    let (release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let original = agent.enqueue(input("original"), actor()).await.unwrap();
    timeout(Duration::from_secs(5), backend.executing.notified())
        .await
        .unwrap();
    let (receipt, probe) = register_receipt(original, agent.clone());
    release.send(()).unwrap();
    assert_eq!(
        timeout(Duration::from_secs(5), receipt).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    assert_eq!(backend.closes.load(Ordering::SeqCst), 1);
    assert_eq!(
        probe.outcome(),
        Poll::Ready(Err(AgentError::AttachmentUnavailable(AttachmentPhase::Absent))),
        "confirmed automatic cleanup cannot remain Closed because original work still owns its old generation"
    );
    let saved = storage.snapshot();
    assert_eq!(saved.invocations.len(), 1);
    assert_eq!(
        saved.invocations[0].request.execution_id.as_str(),
        "original"
    );
    assert_eq!(
        saved.invocations[0].result,
        Some(Ok(ExecutionOutcome::Completed))
    );
    *backend.execution_attachment.lock().unwrap() = ProviderSessionState::Usable;
    timeout(Duration::from_secs(5), recover_after_automatic_stop(&agent))
        .await
        .unwrap();
    assert_eq!(
        agent.invoke(input("recovered"), actor()).await,
        Ok(ExecutionOutcome::Completed)
    );
    assert_eq!(backend.executions.load(Ordering::SeqCst), 2);
    agent.close(actor()).await.unwrap();
}

#[derive(Clone, Copy)]
enum ReceiptFault {
    Selection,
    Dispatch,
    Settlement,
}
struct ReceiptPanicStorage {
    backing: MemoryStorage,
    fault: ReceiptFault,
}
struct ReceiptPanicLease {
    backing: Box<dyn SessionStorageLease>,
    fault: ReceiptFault,
    panicked: AtomicBool,
}
impl SessionStorage for ReceiptPanicStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(ReceiptPanicLease {
                backing: self.backing.open(id).await?,
                fault: self.fault,
                panicked: AtomicBool::new(false),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for ReceiptPanicLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.backing.load()
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            let selected = matches!(self.fault, ReceiptFault::Selection)
                && snapshot.queue_history.last().is_some_and(|entry| {
                    matches!(&entry.mutation, QueueMutation::Selected { id } if id.as_str() == "queued")
                });
            let scheduled = snapshot.invocations.iter().any(|record| {
                record.request.execution_id.as_str() == "queued"
                    && record.scheduling.last().is_some_and(|event| {
                        matches!(
                            (self.fault, event.stage),
                            (ReceiptFault::Dispatch, InvocationStage::Running)
                                | (ReceiptFault::Settlement, InvocationStage::Settled)
                        )
                    })
            });
            if (selected || scheduled) && !self.panicked.swap(true, Ordering::SeqCst) {
                panic!("original queued receipt persistence panicked");
            }
            self.backing.save(snapshot).await
        })
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.backing.erase()
    }
}

#[tokio::test]
async fn panicked_queue_receipt_exposes_actual_refusal_after_original_retirement() {
    for fault in [
        ReceiptFault::Selection,
        ReceiptFault::Dispatch,
        ReceiptFault::Settlement,
    ] {
        let storage = MemoryStorage::default();
        let manager = SessionManager::open(
            Some(SessionId::new("conversation").unwrap()),
            Arc::new(ReceiptPanicStorage {
                backing: storage.clone(),
                fault,
            }),
            Arc::new(RuntimeMessageCommitClock::new()),
        )
        .await
        .unwrap();
        let (agent, backend) = probe_with_manager(false, manager).await;
        let (release, gate) = oneshot::channel();
        *backend.execution_gate.lock().unwrap() = Some(gate);
        let active = tokio::spawn({
            let agent = agent.clone();
            async move { agent.invoke(input("active"), actor()).await }
        });
        timeout(Duration::from_secs(5), backend.executing.notified())
            .await
            .unwrap();
        let queued = agent.enqueue(input("queued"), actor()).await.unwrap();
        let (receipt, probe) = register_receipt(queued, agent.clone());
        release.send(()).unwrap();
        assert_eq!(
            timeout(Duration::from_secs(5), active)
                .await
                .unwrap()
                .unwrap(),
            Ok(ExecutionOutcome::Completed)
        );
        let result = timeout(Duration::from_secs(5), receipt).await.unwrap();
        let failure = AgentError::Protocol("scheduling task panicked".into());
        let expected = match fault {
            ReceiptFault::Selection | ReceiptFault::Dispatch => Err(failure),
            // Terminal persistence already owns Completed before this save panic.
            ReceiptFault::Settlement => Err(AgentError::ExecutionObservation {
                error: Box::new(failure),
                execution_result: Some(Box::new(Ok(ExecutionOutcome::Completed))),
            }),
        };
        assert_eq!(result, expected);
        assert_eq!(
            probe.outcome(),
            Poll::Ready(Err(AgentError::Closed)),
            "recovered original slot must not conceal the lifecycle refusal"
        );
        let expected_dispatches = if matches!(fault, ReceiptFault::Settlement) {
            2
        } else {
            1
        };
        assert_eq!(
            backend.executions.load(Ordering::SeqCst),
            expected_dispatches
        );
        let saved = storage.snapshot();
        assert_eq!(saved.invocations.len(), 2);
        assert_eq!(saved.invocations[1].request.execution_id.as_str(), "queued");
        assert_eq!(saved.invocations[1].result, Some(result));
        assert_eq!(
            saved.invocations[1].scheduling.last().unwrap().stage,
            InvocationStage::Settled
        );
        agent.close(actor()).await.unwrap();
        reattach_after_explicit_close(&agent).await;
        assert_eq!(
            agent.invoke(input("recovered"), actor()).await,
            Ok(ExecutionOutcome::Completed)
        );
        assert_eq!(
            backend.executions.load(Ordering::SeqCst),
            expected_dispatches + 1
        );
        agent.close(actor()).await.unwrap();
    }
}

#[tokio::test]
async fn withdrawn_receipt_preserves_busy_from_independent_active_invocation() {
    let (agent, backend, storage) = probe(false).await;
    let (release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let active = tokio::spawn({
        let agent = agent.clone();
        async move { agent.invoke(input("active"), actor()).await }
    });
    timeout(Duration::from_secs(5), backend.executing.notified())
        .await
        .unwrap();
    let queued = agent.enqueue(input("withdrawn"), actor()).await.unwrap();
    let (receipt, probe) = register_receipt(queued, agent.clone());
    let withdrawer = ActionContext::new("owner", "phone", "withdraw").unwrap();
    assert_eq!(
        agent
            .remove_queued(input("withdrawn").execution_id, withdrawer.clone())
            .await,
        Ok(QueueRemoval::Removed)
    );
    assert_eq!(
        timeout(Duration::from_secs(5), receipt).await.unwrap(),
        Err(AgentError::Closed)
    );
    assert_eq!(
        probe.outcome(),
        Poll::Ready(Err(AgentError::Busy)),
        "a distinct provider invocation still owns the slot"
    );
    let saved = storage.snapshot();
    let withdrawn = &saved.invocations[1];
    assert_eq!(withdrawn.request.execution_id.as_str(), "withdrawn");
    let last = withdrawn.scheduling.last().unwrap();
    assert_eq!(last.stage, InvocationStage::Cancelled);
    assert_eq!(last.cause, SchedulingCause::Withdrawn);
    assert_eq!(last.actor, Some(withdrawer));
    assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
    release.send(()).unwrap();
    assert_eq!(
        timeout(Duration::from_secs(5), active)
            .await
            .unwrap()
            .unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn stopped_waiting_receipt_exposes_cleanup_refusal_without_runner_busy() {
    let (agent, backend, storage) = probe(false).await;
    let (release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let active = tokio::spawn({
        let agent = agent.clone();
        async move { agent.invoke(input("active"), actor()).await }
    });
    timeout(Duration::from_secs(5), backend.executing.notified())
        .await
        .unwrap();
    let queued = agent.enqueue(input("stopped"), actor()).await.unwrap();
    let (receipt, probe) = register_receipt(queued, agent.clone());
    *backend.execution_attachment.lock().unwrap() = ProviderSessionState::CleanupReported(
        CleanupReport::unconfirmed(AgentError::CleanupUncertain),
    );
    release.send(()).unwrap();
    assert_eq!(
        timeout(Duration::from_secs(5), active)
            .await
            .unwrap()
            .unwrap(),
        Err(AgentError::ExecutionObservation {
            error: Box::new(AgentError::CleanupUncertain),
            execution_result: Some(Box::new(Ok(ExecutionOutcome::Completed))),
        }),
    );
    assert_eq!(
        timeout(Duration::from_secs(5), receipt).await.unwrap(),
        Err(AgentError::Closed)
    );
    assert_eq!(
        probe.outcome(),
        Poll::Ready(Err(AgentError::Closed)),
        "no independent invocation remains; unconfirmed cleanup is the real refusal"
    );
    assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
    assert_eq!(backend.closes.load(Ordering::SeqCst), 0);
    let saved = storage.snapshot();
    assert_eq!(
        saved.invocations[1].request.execution_id.as_str(),
        "stopped"
    );
    let stopped = saved.invocations[1].scheduling.last().unwrap();
    assert_eq!(stopped.stage, InvocationStage::Cancelled);
    assert_eq!(stopped.cause, SchedulingCause::RunnerStopped);
    assert!(stopped.actor.is_none());
    assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Attached);
    agent.close(actor()).await.unwrap();
    assert_eq!(backend.closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failed_invocation_tail_receipt_preserves_independent_settlement_busy() {
    let (agent, backend, storage) = probe(false).await;
    *backend.execution_error.lock().unwrap() =
        Some(AgentError::InvalidInput("provider rejected input".into()));
    backend.execution_rejected.store(true, Ordering::SeqCst);
    backend.suppress_terminal.store(true, Ordering::SeqCst);
    let (release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let original = agent.enqueue(input("original"), actor()).await.unwrap();
    timeout(Duration::from_secs(5), backend.executing.notified())
        .await
        .unwrap();
    let tail = agent.enqueue(input("tail"), actor()).await.unwrap();
    let (tail, probe) = register_receipt(tail, agent.clone());
    release.send(()).unwrap();
    assert_eq!(
        timeout(Duration::from_secs(5), tail).await.unwrap(),
        Err(AgentError::Closed)
    );
    assert_eq!(probe.outcome(), Poll::Ready(Err(AgentError::Busy)), "the selected original still owns its terminal persistence independently of this tail receipt");
    assert_eq!(
        timeout(Duration::from_secs(5), original.wait())
            .await
            .unwrap(),
        Err(AgentError::InvalidInput("provider rejected input".into()))
    );
    let saved = storage.snapshot();
    assert_eq!(saved.invocations.len(), 2);
    assert_eq!(saved.invocations[1].request.execution_id.as_str(), "tail");
    let stopped = saved.invocations[1].scheduling.last().unwrap();
    assert_eq!(stopped.stage, InvocationStage::Cancelled);
    assert_eq!(stopped.cause, SchedulingCause::RunnerStopped);
    assert!(stopped.actor.is_none());
    assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
    *backend.execution_error.lock().unwrap() = None;
    backend.execution_rejected.store(false, Ordering::SeqCst);
    backend.suppress_terminal.store(false, Ordering::SeqCst);
    assert_eq!(
        agent.invoke(input("following"), actor()).await,
        Ok(ExecutionOutcome::Completed)
    );
    assert_eq!(backend.executions.load(Ordering::SeqCst), 2);
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn failed_automatic_attachment_receipt_exposes_exact_attachment_refusal() {
    let (agent, backend, storage) = probe(false).await;
    let (release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let active = tokio::spawn({
        let agent = agent.clone();
        async move { agent.invoke(input("active"), actor()).await }
    });
    timeout(Duration::from_secs(5), backend.executing.notified())
        .await
        .unwrap();
    let waiting = agent.enqueue(input("waiting"), actor()).await.unwrap();
    let (receipt, probe) = register_receipt(waiting, agent.clone());
    let failure = AgentError::InvalidInput("replacement attachment unavailable".into());
    *backend.open_error.lock().unwrap() = Some(failure.clone());
    *backend.execution_attachment.lock().unwrap() =
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        }));
    release.send(()).unwrap();
    assert_eq!(
        timeout(Duration::from_secs(5), active)
            .await
            .unwrap()
            .unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    assert_eq!(
        timeout(Duration::from_secs(5), receipt).await.unwrap(),
        Err(failure.clone())
    );
    assert_eq!(
        probe.outcome(),
        Poll::Ready(Err(AgentError::AttachmentUnavailable(
            AttachmentPhase::Failed(AttachmentFailureCode::Provider)
        ))),
        "the attachment producer must not publish behind its caller's obsolete runner slot",
    );
    assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
    let saved = storage.snapshot();
    assert_eq!(
        saved.invocations[1].request.execution_id.as_str(),
        "waiting"
    );
    assert_eq!(saved.invocations[1].result, Some(Err(failure)));
    let failed = saved.invocations[1].scheduling.last().unwrap();
    assert_eq!(failed.stage, InvocationStage::Settled);
    assert_eq!(failed.cause, SchedulingCause::DispatchFailed);
    assert!(failed.actor.is_none());
    agent.close(actor()).await.unwrap();
}
