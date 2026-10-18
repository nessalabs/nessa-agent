//! Canonical relationships come from possible SDK admission histories, not IDs.
use super::*;

/// Obtain the exact continuation input from a genuine SDK resume in a separate
/// Agent. The Agents under test below admit only ordinary caller inputs.
async fn canonical_requests(id: &str) -> (ExecutionRequest, ExecutionRequest) {
    let saved = cut_off_while_running(id).await;
    let (agent, backend, storage, _hold) = restore_holding(saved).await;
    let owed = agent.owed_turns().await.remove(0);
    let Resumed::Continued(next) = agent.resume_turn(&owed, resumer()).await.unwrap() else {
        panic!("the template is a real continuation")
    };
    bounded(backend.dispatched.notified()).await;
    let snapshot = storage.snapshot();
    let original = snapshot.invocations[0].request.clone();
    let successor = snapshot
        .invocations
        .iter()
        .find(|record| record.request.execution_id == *next.id())
        .unwrap()
        .request
        .clone();
    bounded(agent.close(actor())).await.unwrap();
    (original, successor)
}

#[tokio::test]
async fn a_completed_caller_input_before_its_source_is_not_a_canonical_successor() {
    caller_created_successor(false).await;
}

#[tokio::test]
async fn a_queued_caller_input_behind_an_unstopped_source_is_not_a_canonical_successor() {
    caller_created_successor(true).await;
}

async fn caller_created_successor(source_first: bool) {
    let (original, successor) = canonical_requests("source").await;
    let original_id = original.execution_id.clone();
    let successor_id = successor.execution_id.clone();
    let (agent, backend, storage) = workflow().await;
    if !source_first {
        let earlier = agent
            .enqueue(successor.clone(), Submitter::owner(actor()))
            .await
            .unwrap();
        assert_eq!(
            bounded(earlier.wait()).await,
            Ok(ExecutionOutcome::Completed)
        );
        bounded(backend.dispatched.notified()).await;
    }
    let (_release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let original = agent
        .enqueue(original, Submitter::owner(actor()))
        .await
        .unwrap();
    bounded(backend.dispatched.notified()).await;
    let later = if source_first {
        Some(
            agent
                .enqueue(successor, Submitter::owner(actor()))
                .await
                .unwrap(),
        )
    } else {
        None
    };
    assert_eq!(agent.active_execution_id(), Some(original_id.clone()));
    if !source_first {
        // A real owed stop still cannot make an earlier caller input its
        // successor. This isolates forward order from stop validation.
        *backend.execution_report.lock().unwrap() = Some(ExecutionReport::cancelled_locally(
            CleanupReport::confirmed(CloseOutcome { forced: false }),
        ));
        bounded(agent.suspend(resumer())).await.unwrap();
    }
    let snapshot = storage.snapshot();
    assert_eq!(resumed_turn_id(&snapshot.invocations, &original_id), None);
    assert!(owed_turn_ids(&snapshot.invocations).contains(&original_id));
    let saved = MemoryStorage::default();
    saved.0.lock().unwrap().snapshot = Some(snapshot);
    bounded(agent.close(actor())).await.unwrap();
    drop((original, later));

    let (restored, _, storage) = workflow_from_storage(saved).await;
    let before = storage.snapshot();
    let owed = restored
        .owed_turns()
        .await
        .into_iter()
        .find(|turn| turn.execution_id() == &original_id)
        .expect("the unrelated ID cannot pay the original work");
    assert_eq!(resumed_turn_id(&before.invocations, &original_id), None);
    assert!(owed_turn_ids(&before.invocations).contains(&original_id));
    assert!(matches!(
        restored.resume_turn(&owed, resumer()).await,
        Err(AgentError::SubmissionConflict)
    ));
    let after = storage.snapshot();
    for id in [&original_id, &successor_id] {
        let find = |snapshot: &SessionSnapshot| {
            snapshot
                .invocations
                .iter()
                .find(|record| &record.request.execution_id == id)
                .unwrap()
                .clone()
        };
        let prior = find(&before);
        let current = find(&after);
        assert_eq!(current.request, prior.request);
        assert_eq!(current.scheduling, prior.scheduling);
        assert_eq!(current.result, prior.result);
    }
    bounded(restored.decline_turn(&owed, resumer()))
        .await
        .unwrap();
    assert!(!restored
        .owed_turns()
        .await
        .iter()
        .any(|turn| turn.execution_id() == &original_id));
    let declined = storage.snapshot();
    let unrelated = declined
        .invocations
        .iter()
        .find(|record| record.request.execution_id == successor_id)
        .unwrap();
    let prior = before
        .invocations
        .iter()
        .find(|record| record.request.execution_id == successor_id)
        .unwrap();
    assert_eq!(unrelated.request, prior.request);
    assert_eq!(unrelated.scheduling, prior.scheduling);
    assert_eq!(unrelated.result, prior.result);
    bounded(restored.close(actor())).await.unwrap();
}

#[tokio::test]
async fn an_immediate_caller_input_after_an_owed_stop_is_not_a_queued_successor() {
    let (_, successor) = canonical_requests("stopped").await;
    let saved = stopped_storage(Stop::Suspend).await;
    let (agent, _, storage) = workflow_from_storage(saved).await;
    let id = ExecutionId::new("stopped").unwrap();
    assert!(agent
        .owed_turns()
        .await
        .iter()
        .any(|turn| turn.execution_id() == &id));
    assert_eq!(
        bounded(agent.invoke(successor, Submitter::owner(actor()))).await,
        Ok(ExecutionOutcome::Completed)
    );
    let history = storage.snapshot().invocations;
    assert_eq!(history[1].submission, SubmissionMode::Immediate);
    assert_eq!(resumed_turn_id(&history, &id), None);
    assert!(owed_turn_ids(&history).contains(&id));
    assert!(agent
        .owed_turns()
        .await
        .iter()
        .any(|turn| turn.execution_id() == &id));
    bounded(agent.close(actor())).await.unwrap();
}
