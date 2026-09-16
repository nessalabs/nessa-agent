//! Atomic order changes preserve receipts and provider priority under deterministic races.
use super::*;
use nessa_sdk::application::agent_execution::agents::QueueReorder;
use nessa_sdk::application::agent_execution::executions::{ExecutionAuditRecord, QueueOrderCause};
use nessa_sdk::domain::agent_execution::executions::{QueueMutation, QueueOrderChange};
fn reorders(
    snapshot: &SessionSnapshot,
) -> Vec<&nessa_sdk::application::agent_execution::sessions::QueueHistoryRecord> {
    snapshot
        .queue_history
        .iter()
        .filter(|entry| matches!(entry.mutation, QueueMutation::Reordered(_)))
        .collect()
}
fn ids(values: &[&str]) -> Vec<ExecutionId> {
    values
        .iter()
        .map(|value| ExecutionId::new(*value).unwrap())
        .collect()
}
fn audited_reorders(backend: &WorkflowBackend) -> Vec<QueueOrderRecord> {
    backend
        .audit
        .records
        .lock()
        .unwrap()
        .iter()
        .filter_map(|record| match record {
            ExecutionAuditRecord::QueueReordered(record) => Some(record.clone()),
            _ => None,
        })
        .collect()
}
async fn waiting() -> (
    Agent,
    Arc<WorkflowBackend>,
    MemoryStorage,
    oneshot::Sender<()>,
    Vec<QueuedInvocation>,
) {
    let (agent, backend, storage) = workflow().await;
    let (release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let first = agent.enqueue(request("running"), actor()).await.unwrap();
    bounded(backend.dispatched.notified()).await;
    let b = agent.enqueue(request("b"), actor()).await.unwrap();
    let c = agent.enqueue(request("c"), actor()).await.unwrap();
    let s = agent.enqueue_steering(request("s"), actor()).await.unwrap();
    (agent, backend, storage, release, vec![first, b, c, s])
}
#[tokio::test]
async fn reorder_is_atomic_preserves_priority_and_dispatches_original_receipts() {
    let (agent, backend, storage, release, receipts) = waiting().await;
    assert_eq!(agent.queued_ids().await, ids(&["s", "b", "c"]));
    assert_eq!(
        agent
            .reorder_queued(ids(&["b", "s", "c"]), actor())
            .await
            .unwrap(),
        QueueReorder::PriorityConflict
    );
    assert_eq!(
        agent
            .reorder_queued(ids(&["s", "c"]), actor())
            .await
            .unwrap(),
        QueueReorder::QueueChanged
    );
    assert!(matches!(
        agent.reorder_queued(ids(&["s", "c", "c"]), actor()).await,
        Err(AgentError::InvalidInput(_))
    ));
    assert_eq!(
        agent
            .reorder_queued(ids(&["s", "c", "b"]), actor())
            .await
            .unwrap(),
        QueueReorder::Applied
    );
    let saved = storage.snapshot();
    assert_eq!(reorders(&saved).len(), 1);
    assert_eq!(reorders(&saved)[0].actor, Some(actor()));
    assert_eq!(
        match &reorders(&saved)[0].mutation {
            QueueMutation::Reordered(change) => change.after(),
            _ => unreachable!(),
        },
        ids(&["s", "c", "b"])
    );
    let audited = audited_reorders(&backend);
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0].session_id().as_str(), "conversation");
    assert_eq!(audited[0].actor(), &actor());
    assert_eq!(audited[0].cause(), QueueOrderCause::CallerRequested);
    assert_eq!(audited[0].change().after(), ids(&["s", "c", "b"]));

    release.send(()).unwrap();
    for receipt in receipts {
        bounded(receipt.wait()).await.unwrap();
    }
    assert_eq!(
        *backend.executions.lock().unwrap(),
        ids(&["running", "s", "c", "b"])
    );
    assert!(agent.queued_ids().await.is_empty());
    agent.close(actor()).await.unwrap();
}
#[tokio::test]
async fn failed_save_retains_live_order_and_unchanged_retry_flushes_evidence() {
    let (agent, backend, storage, release, receipts) = waiting().await;
    {
        let mut state = storage.0.lock().unwrap();
        state.fail_write = Some(state.writes + 1);
    }
    assert!(matches!(
        agent.reorder_queued(ids(&["s", "c", "b"]), actor()).await,
        Err(AgentError::Storage(_))
    ));
    assert_eq!(agent.queued_ids().await, ids(&["s", "c", "b"]));
    assert!(reorders(&storage.snapshot()).is_empty());
    assert_eq!(audited_reorders(&backend).len(), 1);
    assert_eq!(
        agent
            .reorder_queued(ids(&["s", "c", "b"]), actor())
            .await
            .unwrap(),
        QueueReorder::Unchanged
    );
    assert_eq!(reorders(&storage.snapshot()).len(), 1);
    release.send(()).unwrap();
    for receipt in receipts {
        bounded(receipt.wait()).await.unwrap();
    }
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn rejected_audit_leaves_order_and_storage_unchanged() {
    let (agent, backend, storage, release, receipts) = waiting().await;
    backend.audit.reject.store(true, Ordering::SeqCst);
    assert_eq!(
        agent.reorder_queued(ids(&["s", "c", "b"]), actor()).await,
        Err(AgentError::AuditFailure)
    );
    assert_eq!(agent.queued_ids().await, ids(&["s", "b", "c"]));
    assert!(reorders(&storage.snapshot()).is_empty());
    assert_eq!(audited_reorders(&backend).len(), 1);
    backend.audit.reject.store(false, Ordering::SeqCst);
    release.send(()).unwrap();
    for receipt in receipts {
        bounded(receipt.wait()).await.unwrap();
    }
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn caller_loss_during_audit_keeps_the_owned_reorder_transaction() {
    let (agent, backend, storage, release, receipts) = waiting().await;
    let (entered, waiting) = oneshot::channel();
    let (resume, paused) = oneshot::channel();
    *backend.audit.pause.lock().unwrap() = Some((entered, paused));
    let caller = tokio::spawn({
        let agent = agent.clone();
        async move { agent.reorder_queued(ids(&["s", "c", "b"]), actor()).await }
    });
    bounded(waiting).await.unwrap();
    caller.abort();
    resume.send(()).unwrap();
    tokio::task::yield_now().await;
    assert_eq!(bounded(agent.queued_ids()).await, ids(&["s", "c", "b"]));
    assert_eq!(reorders(&storage.snapshot()).len(), 1);
    assert_eq!(audited_reorders(&backend).len(), 1);
    release.send(()).unwrap();
    for receipt in receipts {
        bounded(receipt.wait()).await.unwrap();
    }
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn close_interrupts_stalled_reorder_audit_and_settles_pending_receipts() {
    let (agent, backend, storage, _release, receipts) = waiting().await;
    let (entered, waiting) = oneshot::channel();
    let (_never_resume, stalled) = oneshot::channel();
    *backend.audit.pause.lock().unwrap() = Some((entered, stalled));
    let reordering = tokio::spawn({
        let agent = agent.clone();
        async move { agent.reorder_queued(ids(&["s", "c", "b"]), actor()).await }
    });
    bounded(waiting).await.unwrap();
    assert!(!bounded(agent.close(actor())).await.unwrap().forced);
    assert_eq!(bounded(reordering).await.unwrap(), Err(AgentError::Closed));
    assert!(reorders(&storage.snapshot()).is_empty());
    for (index, receipt) in receipts.into_iter().enumerate() {
        let result = bounded(receipt.wait()).await;
        if index == 0 {
            assert_eq!(result, Ok(ExecutionOutcome::Completed));
        } else {
            assert_eq!(result, Err(AgentError::Closed));
        }
    }
}

#[tokio::test]
async fn panicking_reorder_audit_fences_and_recovers_without_applying_order() {
    let (agent, backend, storage, _release, receipts) = waiting().await;
    backend.audit.panic.store(true, Ordering::SeqCst);
    assert!(matches!(
        agent.reorder_queued(ids(&["s", "c", "b"]), actor()).await,
        Err(AgentError::Protocol(message)) if message == "scheduling task panicked"
    ));
    assert!(reorders(&storage.snapshot()).is_empty());
    assert!(matches!(
        agent.enqueue(request("later"), actor()).await,
        Err(AgentError::Closed)
    ));
    for (index, receipt) in receipts.into_iter().enumerate() {
        let result = receipt.wait().await;
        if index == 0 {
            assert_eq!(result, Ok(ExecutionOutcome::Completed));
        } else {
            assert!(result.is_err());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn stalled_reorder_audit_has_a_bounded_acknowledgement() {
    let (agent, backend, storage, release, receipts) = waiting().await;
    let (entered, waiting) = oneshot::channel();
    let (_never_resume, stalled) = oneshot::channel();
    *backend.audit.pause.lock().unwrap() = Some((entered, stalled));
    let reordering = tokio::spawn({
        let agent = agent.clone();
        async move { agent.reorder_queued(ids(&["s", "c", "b"]), actor()).await }
    });
    bounded(waiting).await.unwrap();
    tokio::time::advance(Duration::from_secs(30)).await;
    assert_eq!(reordering.await.unwrap(), Err(AgentError::AuditFailure));
    assert!(reorders(&storage.snapshot()).is_empty());
    release.send(()).unwrap();
    for receipt in receipts {
        bounded(receipt.wait()).await.unwrap();
    }
    agent.close(actor()).await.unwrap();
}
#[tokio::test]
async fn caller_loss_during_save_does_not_abandon_the_order_transaction() {
    let (agent, _, storage, release, receipts) = waiting().await;
    let (started, waiting) = oneshot::channel();
    let (commit, gate) = oneshot::channel();
    storage.0.lock().unwrap().pause_save = Some((started, gate));
    let caller = tokio::spawn({
        let agent = agent.clone();
        async move { agent.reorder_queued(ids(&["s", "c", "b"]), actor()).await }
    });
    bounded(waiting).await.unwrap();
    caller.abort();
    commit.send(()).unwrap();
    assert_eq!(bounded(agent.queued_ids()).await, ids(&["s", "c", "b"]));
    assert_eq!(reorders(&storage.snapshot()).len(), 1);
    release.send(()).unwrap();
    for receipt in receipts {
        bounded(receipt.wait()).await.unwrap();
    }
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn close_interrupts_stalled_reorder_persistence_and_settles_pending_work() {
    let (agent, _, storage, _release, receipts) = waiting().await;
    let (started, waiting) = oneshot::channel();
    let (_never_commit, stalled) = oneshot::channel();
    storage.0.lock().unwrap().pause_save = Some((started, stalled));
    let reordering = tokio::spawn({
        let agent = agent.clone();
        async move { agent.reorder_queued(ids(&["s", "c", "b"]), actor()).await }
    });
    bounded(waiting).await.unwrap();
    assert!(!bounded(agent.close(actor())).await.unwrap().forced);
    assert!(matches!(
        bounded(reordering).await.unwrap(),
        Err(AgentError::Storage(_))
    ));
    for (index, receipt) in receipts.into_iter().enumerate() {
        let result = bounded(receipt.wait()).await;
        if index == 0 {
            assert_eq!(result, Ok(ExecutionOutcome::Completed));
        } else {
            assert_eq!(result, Err(AgentError::Closed));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn stalled_reorder_persistence_is_bounded_and_flushes_before_dispatch() {
    let (agent, backend, storage, release, receipts) = waiting().await;
    let (started, waiting) = oneshot::channel();
    let (_never_commit, stalled) = oneshot::channel();
    storage.0.lock().unwrap().pause_save = Some((started, stalled));
    let reordering = tokio::spawn({
        let agent = agent.clone();
        async move { agent.reorder_queued(ids(&["s", "c", "b"]), actor()).await }
    });
    bounded(waiting).await.unwrap();
    tokio::time::advance(Duration::from_secs(30)).await;
    assert!(matches!(
        reordering.await.unwrap(),
        Err(AgentError::Storage(_))
    ));
    assert_eq!(agent.queued_ids().await, ids(&["s", "c", "b"]));
    assert!(reorders(&storage.snapshot()).is_empty());

    release.send(()).unwrap();
    for receipt in receipts {
        bounded(receipt.wait()).await.unwrap();
    }
    assert_eq!(
        *backend.executions.lock().unwrap(),
        ids(&["running", "s", "c", "b"])
    );
    assert_eq!(reorders(&storage.snapshot()).len(), 1);
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn panicking_reorder_persistence_fences_and_recovers_with_evidence() {
    let (agent, _backend, storage, _release, receipts) = waiting().await;
    let change = QueueOrderChange::new(
        vec![
            (ExecutionId::new("s").unwrap(), InvocationKind::Steering),
            (ExecutionId::new("b").unwrap(), InvocationKind::Queued),
            (ExecutionId::new("c").unwrap(), InvocationKind::Queued),
        ],
        ids(&["s", "c", "b"]),
    )
    .unwrap();
    storage.0.lock().unwrap().panic_queue = Some(QueueMutation::Reordered(change));
    assert!(matches!(
        agent.reorder_queued(ids(&["s", "c", "b"]), actor()).await,
        Err(AgentError::Protocol(message)) if message == "scheduling task panicked"
    ));
    assert!(matches!(
        agent.enqueue(request("later"), actor()).await,
        Err(AgentError::Closed)
    ));
    assert_eq!(reorders(&storage.snapshot()).len(), 1);
    for (index, receipt) in receipts.into_iter().enumerate() {
        let result = bounded(receipt.wait()).await;
        if index == 0 {
            assert_eq!(result, Ok(ExecutionOutcome::Completed));
        } else {
            assert!(result.is_err());
        }
    }
}

#[tokio::test]
async fn failed_membership_admission_keeps_one_receipt_and_dispatches_once_on_retry() {
    let (agent, backend, storage) = workflow().await;
    storage.0.lock().unwrap().fail_queue = Some(QueueMutation::Admitted {
        id: ExecutionId::new("owned").unwrap(),
        kind: InvocationKind::Queued,
    });
    assert!(matches!(
        agent.enqueue(request("owned"), actor()).await,
        Err(AgentError::Storage(_))
    ));
    let receipt = agent.enqueue(request("owned"), actor()).await.unwrap();
    bounded(receipt.wait()).await.unwrap();
    assert_eq!(*backend.executions.lock().unwrap(), ids(&["owned"]));
    assert_eq!(storage.snapshot().queue_history.iter().filter(|entry| matches!(&entry.mutation,QueueMutation::Admitted{id,..} if id.as_str()=="owned")).count(),1);
    agent.close(actor()).await.unwrap();
}
#[tokio::test]
async fn panicking_membership_admission_cancels_the_pending_owner_once() {
    let (agent, backend, storage) = workflow().await;
    storage.0.lock().unwrap().panic_queue = Some(QueueMutation::Admitted {
        id: ExecutionId::new("owned").unwrap(),
        kind: InvocationKind::Queued,
    });
    assert!(agent.enqueue(request("owned"), actor()).await.is_err());
    let receipt = agent.enqueue(request("owned"), actor()).await.unwrap();
    assert!(bounded(receipt.wait()).await.is_err());
    assert!(backend.executions.lock().unwrap().is_empty());
    assert!(agent.queued_ids().await.is_empty());
    assert_eq!(storage.snapshot().queue_history.iter().filter(|entry| matches!(&entry.mutation,QueueMutation::Removed{id,..} if id.as_str()=="owned")).count(),1);
}
#[tokio::test]
async fn selected_write_failure_or_panic_never_dispatches_or_double_removes() {
    for panic in [false, true] {
        let (agent, backend, storage) = workflow().await;
        let mutation = QueueMutation::Selected {
            id: ExecutionId::new("owned").unwrap(),
        };
        if panic {
            storage.0.lock().unwrap().panic_queue = Some(mutation);
        } else {
            storage.0.lock().unwrap().fail_queue = Some(mutation);
        }
        let receipt = agent.enqueue(request("owned"), actor()).await.unwrap();
        assert!(bounded(receipt.wait()).await.is_err());
        assert!(backend.executions.lock().unwrap().is_empty());
        assert!(agent.queued_ids().await.is_empty());
        let saved = storage.snapshot();
        assert_eq!(saved.queue_history.iter().filter(|entry| matches!(&entry.mutation,QueueMutation::Selected{id} if id.as_str()=="owned")).count(),1);
        assert!(!saved.queue_history.iter().any(
            |entry| matches!(&entry.mutation,QueueMutation::Removed{id,..} if id.as_str()=="owned")
        ));
        assert!(saved.invocations[0].result.as_ref().unwrap().is_err());
    }
}

#[tokio::test]
async fn selection_persistence_fences_reorder_and_keeps_dequeued_input_out_of_before_state() {
    let (agent, backend, storage) = workflow().await;
    let (finish_a, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let a = agent.enqueue(request("a"), actor()).await.unwrap();
    bounded(backend.dispatched.notified()).await;
    let b = agent.enqueue(request("b"), actor()).await.unwrap();
    let c = agent.enqueue(request("c"), actor()).await.unwrap();
    let d = agent.enqueue(request("d"), actor()).await.unwrap();
    let (selected, waiting) = oneshot::channel();
    let (save, gate) = oneshot::channel();
    storage.0.lock().unwrap().pause_queue = Some((
        QueueMutation::Selected {
            id: ExecutionId::new("b").unwrap(),
        },
        selected,
        gate,
    ));
    let (finish_b, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    finish_a.send(()).unwrap();
    bounded(waiting).await.unwrap();
    let reorder = tokio::spawn({
        let agent = agent.clone();
        async move { agent.reorder_queued(ids(&["d", "c"]), actor()).await }
    });
    tokio::task::yield_now().await;
    assert!(!reorder.is_finished());
    assert_eq!(*backend.executions.lock().unwrap(), ids(&["a"]));
    save.send(()).unwrap();
    assert_eq!(
        bounded(reorder).await.unwrap().unwrap(),
        QueueReorder::Applied
    );
    let saved = storage.snapshot();
    let selected = saved
        .queue_history
        .iter()
        .position(|entry| matches!(&entry.mutation,QueueMutation::Selected{id} if id.as_str()=="b"))
        .unwrap();
    let reordered = saved
        .queue_history
        .iter()
        .position(|entry| matches!(&entry.mutation, QueueMutation::Reordered(_)))
        .unwrap();
    assert!(selected < reordered);
    finish_b.send(()).unwrap();
    for receipt in [a, b, c, d] {
        bounded(receipt.wait()).await.unwrap();
    }
    assert_eq!(
        *backend.executions.lock().unwrap(),
        ids(&["a", "b", "d", "c"])
    );
    agent.close(actor()).await.unwrap();
}
