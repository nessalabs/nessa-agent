//! Canonical relationships require the SDK owner's saved admission, not input shape.
use super::*;
use nessa_sdk::domain::agent_execution::executions::{GrantInitiator, Initiation};

#[tokio::test]
async fn contradictory_restored_origin_cannot_revive_superseded_work() {
    let (agent, backend, storage) = workflow().await;
    let error = AgentError::Transport("recoverable failure".into());
    *backend.execution_fault.lock().unwrap() = Some(ProviderOperationFailure::new(
        error.clone(),
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    ));
    let old = agent
        .enqueue(request("old-failure"), Submitter::owner(actor()))
        .await
        .unwrap();
    assert_eq!(bounded(old.wait()).await, Err(error));
    *backend.execution_fault.lock().unwrap() = None;
    let later = agent
        .enqueue(request("later-person"), Submitter::owner(actor()))
        .await
        .unwrap();
    assert_eq!(bounded(later.wait()).await, Ok(ExecutionOutcome::Completed));
    assert!(agent.owed_turns().await.is_empty());
    let snapshot = storage.snapshot();
    bounded(agent.close(actor())).await.unwrap();
    // The unmodified ordinary input remains valid and supersedes the failure.
    let ordinary = MemoryStorage::default();
    ordinary.0.lock().unwrap().snapshot = Some(snapshot.clone());
    let (restored, _, _) = workflow_from_storage(ordinary).await;
    assert!(restored.owed_turns().await.is_empty());
    bounded(restored.close(actor())).await.unwrap();
    for source in ["missing-source", "later-person", "old-failure"] {
        let mut invalid = snapshot.clone();
        invalid.invocations[1].resumed_from = Some(ExecutionId::new(source).unwrap());
        let saved = MemoryStorage::default();
        saved.0.lock().unwrap().snapshot = Some(invalid);
        let backend = workflow_backend();
        let result = Agent::prepare(
            Arc::new(WorkflowProvider(backend.clone())),
            saved.manager().await,
            backend.audit.clone(),
        )
        .await;
        assert!(
            matches!(result, Err(error) if matches!(error.cause(), AgentError::Storage(StorageError::Corrupt(_)))),
            "contradictory origin {source} must be refused before restoring work"
        );
        assert!(backend.executions.lock().unwrap().is_empty());
        assert_eq!(saved.0.lock().unwrap().writes, 0);
    }
}

#[tokio::test]
async fn restored_resume_origin_requires_the_exact_admitted_binding() {
    let saved = stopped_storage(Stop::Suspend).await;
    let (agent, backend, storage, _hold) = restore_holding(saved).await;
    let turn = agent.owed_turns().await.remove(0);
    let Resumed::Continued(_) = agent.resume_turn(&turn, resumer()).await.unwrap() else {
        panic!("a real SDK resume is admitted")
    };
    bounded(backend.dispatched.notified()).await;
    let snapshot = storage.snapshot();
    bounded(agent.close(actor())).await.unwrap();

    for mismatch in ["actor", "initiation", "request", "mode", "stop"] {
        let mut invalid = snapshot.clone();
        match mismatch {
            "actor" => {
                let successor = &mut invalid.invocations[1];
                let original_actor = successor.actor.clone();
                successor.actor = resumer();
                for edge in &mut successor.scheduling {
                    if edge.actor.as_ref() == Some(&original_actor) {
                        edge.actor = Some(resumer());
                    }
                }
                for entry in &mut invalid.queue_history {
                    if entry.mutation.id() == Some(&successor.request.execution_id)
                        && entry.actor.as_ref() == Some(&original_actor)
                    {
                        entry.actor = Some(resumer());
                    }
                }
            }
            "initiation" => {
                invalid.invocations[1].initiation =
                    Initiation::grant(GrantInitiator::new("different-grant", 1).unwrap());
            }
            "request" => invalid.invocations[1].request.estimated_input_tokens += 1,
            "mode" => invalid.invocations[1].submission = SubmissionMode::Immediate,
            "stop" => {
                let source = &mut invalid.invocations[0];
                source.scheduling.last_mut().unwrap().cause = SchedulingCause::SessionClosed;
                source.local_cancellation.as_mut().unwrap().cause = SchedulingCause::SessionClosed;
            }
            _ => unreachable!(),
        }
        assert_eq!(
            resumed_turn_id(&invalid.invocations, turn.execution_id()),
            None,
            "a contradictory {mismatch} cannot grant canonical continuation authority"
        );
        let saved = MemoryStorage::default();
        saved.0.lock().unwrap().snapshot = Some(invalid);
        let backend = workflow_backend();
        let result = Agent::prepare(
            Arc::new(WorkflowProvider(backend.clone())),
            saved.manager().await,
            backend.audit.clone(),
        )
        .await;
        assert!(
            matches!(result, Err(error) if matches!(error.cause(), AgentError::Storage(StorageError::Corrupt(_)))),
            "the SDK owner must reject a contradictory {mismatch} before restoring work"
        );
        assert!(backend.executions.lock().unwrap().is_empty());
        assert_eq!(saved.0.lock().unwrap().writes, 0);
    }
}

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

#[tokio::test]
async fn an_exact_queued_caller_input_after_a_recorded_stop_is_not_a_resume() {
    let (_, successor) = canonical_requests("stopped").await;
    let saved = stopped_storage(Stop::Suspend).await;
    let (agent, backend, storage) = workflow_from_storage(saved).await;
    let id = ExecutionId::new("stopped").unwrap();
    let (_release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let input = agent
        .enqueue(successor, Submitter::owner(actor()))
        .await
        .unwrap();
    bounded(backend.dispatched.notified()).await;
    let snapshot = storage.snapshot();
    assert!(snapshot.invocations[1].resumed_from.is_none());
    assert_eq!(resumed_turn_id(&snapshot.invocations, &id), None);
    assert!(owed_turn_ids(&snapshot.invocations).contains(&id));
    let saved = MemoryStorage::default();
    saved.0.lock().unwrap().snapshot = Some(snapshot);
    bounded(agent.close(actor())).await.unwrap();
    drop(input);
    let (restored, _, storage) = workflow_from_storage(saved).await;
    let before = storage.snapshot();
    assert_eq!(resumed_turn_id(&before.invocations, &id), None);
    let owed = restored
        .owed_turns()
        .await
        .into_iter()
        .find(|turn| turn.execution_id() == &id)
        .expect("ordinary input cannot resume the source");
    assert!(matches!(
        restored.resume_turn(&owed, resumer()).await,
        Err(AgentError::SubmissionConflict)
    ));
    assert_eq!(storage.snapshot().invocations, before.invocations);
    let original = &before.invocations[0];
    let retry = restored
        .enqueue(original.request.clone(), Submitter::owner(actor()))
        .await
        .unwrap();
    assert_eq!(
        bounded(retry.wait()).await,
        original.result.clone().unwrap_or(Err(AgentError::Closed))
    );
    assert_eq!(storage.snapshot().invocations, before.invocations);
    bounded(restored.close(actor())).await.unwrap();
}

#[tokio::test]
async fn a_real_resume_retains_its_admission_relationship_after_restore() {
    let saved = stopped_storage(Stop::Suspend).await;
    let (agent, backend, storage, _hold) = restore_holding(saved).await;
    let turn = agent.owed_turns().await.remove(0);
    let Resumed::Continued(next) = agent.resume_turn(&turn, resumer()).await.unwrap() else {
        panic!("a real SDK resume is admitted")
    };
    bounded(backend.dispatched.notified()).await;
    let snapshot = storage.snapshot();
    assert_eq!(
        snapshot.invocations[1].resumed_from.as_ref(),
        Some(turn.execution_id())
    );
    let mut unattested = snapshot.invocations.clone();
    unattested[1].resumed_from = None;
    assert_eq!(resumed_turn_id(&unattested, turn.execution_id()), None);
    assert_eq!(
        resumed_turn_id(&snapshot.invocations, turn.execution_id()),
        Some(next.id().clone())
    );
    assert!(!owed_turn_ids(&snapshot.invocations).contains(turn.execution_id()));
    let saved = MemoryStorage::default();
    saved.0.lock().unwrap().snapshot = Some(snapshot);
    bounded(agent.close(actor())).await.unwrap();
    let (restored, _, storage) = workflow_from_storage(saved).await;
    assert_eq!(
        resumed_turn_id(&storage.snapshot().invocations, turn.execution_id()),
        Some(next.id().clone())
    );
    assert!(!restored
        .owed_turns()
        .await
        .iter()
        .any(|owed| owed.execution_id() == turn.execution_id()));
    bounded(restored.close(actor())).await.unwrap();
}

#[tokio::test]
async fn a_saved_resume_origin_survives_a_later_source_checkpoint() {
    let saved = stopped_storage(Stop::Suspend).await;
    let (agent, backend, storage, _hold) = restore_holding(saved).await;
    let turn = agent.owed_turns().await.remove(0);
    let Resumed::Continued(next) = agent.resume_turn(&turn, resumer()).await.unwrap() else {
        panic!("a real SDK resume is admitted")
    };
    bounded(backend.dispatched.notified()).await;
    let mut snapshot = storage.snapshot();
    bounded(agent.close(actor())).await.unwrap();
    // A later final decision changes eligibility, not the already saved
    // admission fact. This does not depend on the best-effort resume audit.
    let source = &mut snapshot.invocations[0];
    let mut checkpoint = source.scheduling.last().unwrap().clone();
    checkpoint.before = Some(InvocationStage::Cancelled);
    checkpoint.cause = SchedulingCause::NotResumed;
    checkpoint.actor = Some(resumer());
    source.scheduling.push(checkpoint);
    assert_eq!(
        resumed_turn_id(&snapshot.invocations, turn.execution_id()),
        None
    );
    let saved = MemoryStorage::default();
    saved.0.lock().unwrap().snapshot = Some(snapshot);
    let (restored, _, storage) = workflow_from_storage(saved).await;
    let snapshot = storage.snapshot();
    assert_eq!(snapshot.invocations[1].request.execution_id, *next.id());
    assert_eq!(
        snapshot.invocations[1].resumed_from.as_ref(),
        Some(turn.execution_id())
    );
    assert!(!restored
        .owed_turns()
        .await
        .iter()
        .any(|owed| owed.execution_id() == turn.execution_id()));
    bounded(restored.close(actor())).await.unwrap();
}

/// Park is reached only after the current-thread runtime has drained runnable
/// tasks, so the supervised ordinary admission has registered its scheduler
/// waiter before the paused resume save is released. No sleep or production hook.
#[test]
fn ordinary_enqueue_winning_after_resume_settlement_keeps_its_origin() {
    let release_at_idle = Arc::new(Mutex::new(None::<oneshot::Sender<()>>));
    let checkpoint = release_at_idle.clone();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .on_thread_park(move || {
            if let Some(release) = checkpoint.lock().unwrap().take() {
                release.send(()).expect("resume is waiting on its save");
            }
        })
        .build()
        .unwrap()
        .block_on(async {
            let (_, successor) = canonical_requests("racing-source").await;
            let saved = cut_off_while_running("racing-source").await;
            let (agent, backend, storage, _hold) = restore_holding(saved).await;
            let owed = agent.owed_turns().await.remove(0);
            let (entered, release) = storage.pause_next_save();
            let resuming = tokio::spawn({
                let agent = agent.clone();
                let owed = owed.clone();
                async move { agent.resume_turn(&owed, resumer()).await }
            });
            bounded(entered).await.unwrap();
            let enqueuing = tokio::spawn({
                let agent = agent.clone();
                async move { agent.enqueue(successor, Submitter::owner(actor())).await }
            });
            *release_at_idle.lock().unwrap() = Some(release);
            let (resumed, queued) = bounded(async { tokio::join!(resuming, enqueuing) }).await;
            assert!(matches!(
                resumed.unwrap(),
                Err(AgentError::SubmissionConflict)
            ));
            let queued = queued.unwrap().unwrap();
            bounded(backend.dispatched.notified()).await;
            let history = storage.snapshot().invocations;
            assert!(history[1].resumed_from.is_none());
            assert_eq!(resumed_turn_id(&history, owed.execution_id()), None);
            assert!(owed_turn_ids(&history).contains(owed.execution_id()));
            bounded(agent.close(actor())).await.unwrap();
            drop(queued);
        });
}
