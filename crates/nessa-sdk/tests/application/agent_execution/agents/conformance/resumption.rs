//! A turn cut off by a restart, or stopped by the host's own shutdown, is
//! continued by a successor turn; a person's stop stays final.
use super::*;
use nessa_sdk::application::agent_execution::{
    agents::{AttachmentFailureCode, AttachmentPhase, OwedTurn, Resumed},
    executions::TurnResumption,
};

fn resumer() -> ActionContext {
    ActionContext::new("gateway", "resume_after_restart", "restart").unwrap()
}

/// Run `id` until the provider has it, then copy saved history as a process
/// that ended there would leave it.
async fn cut_off_while_running(id: &str) -> MemoryStorage {
    let (agent, backend, storage) = workflow().await;
    let (_release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let running = Mode::Queued.start(agent.clone(), request(id));
    bounded(backend.dispatched.notified()).await;
    let restored = MemoryStorage::default();
    restored.0.lock().unwrap().snapshot = Some(storage.snapshot());
    bounded(agent.close(actor())).await.unwrap();
    let _ = bounded(running).await;
    restored
}

/// Restore `storage` into a fresh process whose provider holds every turn.
async fn restore_holding(
    storage: MemoryStorage,
) -> (
    Agent,
    Arc<WorkflowBackend>,
    MemoryStorage,
    oneshot::Sender<()>,
) {
    let (agent, backend, storage) = workflow_from_storage(storage).await;
    let (release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    (agent, backend, storage, release)
}

fn successor_text(storage: &MemoryStorage, id: &ExecutionId) -> String {
    storage
        .snapshot()
        .invocations
        .iter()
        .find(|record| &record.request.execution_id == id)
        .expect("successor saved")
        .request
        .user_message
        .text_str()
        .to_owned()
}

fn resumptions(backend: &WorkflowBackend) -> Vec<TurnResumption> {
    backend
        .audit
        .records
        .lock()
        .unwrap()
        .iter()
        .filter_map(|record| match record {
            ExecutionAuditRecord::TurnResumed(record) => Some(record.resumption().clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_turn_cut_off_mid_run_is_continued_by_one_successor() {
    let saved = cut_off_while_running("cut").await;
    let (agent, backend, storage, _hold) = restore_holding(saved).await;
    let owed = agent.owed_turns().await;
    assert_eq!(owed.len(), 1);
    let turn = &owed[0];
    assert_eq!(turn.execution_id().as_str(), "cut");
    assert!(turn.dispatched());
    assert_eq!(turn.submitted_by(), &actor());

    let Resumed::Continued(admission) = bounded(agent.resume_turn(turn, resumer())).await.unwrap()
    else {
        panic!("a cut-off turn is continued")
    };
    bounded(backend.dispatched.notified()).await;
    let successor = admission.id().clone();
    assert_ne!(successor.as_str(), "cut");
    assert_eq!(*backend.executions.lock().unwrap(), vec![successor.clone()]);

    let snapshot = storage.snapshot();
    let original = &snapshot.invocations[0];
    let last = original.scheduling.last().unwrap();
    assert_eq!(last.stage, InvocationStage::Cancelled);
    assert_eq!(last.cause, SchedulingCause::Interrupted);
    let text = successor_text(&storage, &successor);
    assert!(
        text.starts_with("[Nessa restarted while you were working"),
        "{text}"
    );
    assert!(text.ends_with("message cut"), "{text}");
    assert_eq!(
        resumptions(&backend),
        [TurnResumption::Continued {
            successor: successor.clone()
        }]
    );
    assert!(agent.owed_turns().await.is_empty());
    bounded(agent.close(actor())).await.unwrap();
}

#[tokio::test]
async fn resume_twice_queues_one_successor() {
    let saved = cut_off_while_running("twice").await;
    let (agent, backend, storage, _hold) = restore_holding(saved).await;
    let turn: OwedTurn = agent.owed_turns().await.remove(0);
    let first = bounded(agent.resume_turn(&turn, resumer())).await.unwrap();
    assert!(matches!(first, Resumed::Continued(_)));
    let second = bounded(agent.resume_turn(&turn, resumer())).await.unwrap();
    assert!(matches!(second, Resumed::NotOwed));
    assert_eq!(storage.snapshot().invocations.len(), 2);
    assert_eq!(resumptions(&backend).len(), 1);
    bounded(agent.close(actor())).await.unwrap();
}

#[tokio::test]
async fn a_turn_the_provider_never_had_is_sent_again_whole() {
    let (agent, backend, storage) = workflow().await;
    let (_release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let running = Mode::Queued.start(agent.clone(), request("first"));
    bounded(backend.dispatched.notified()).await;
    let waiting = agent
        .enqueue(request("waiting"), Submitter::owner(actor()))
        .await
        .unwrap();
    let saved = MemoryStorage::default();
    saved.0.lock().unwrap().snapshot = Some(storage.snapshot());
    bounded(agent.close(actor())).await.unwrap();
    let _ = bounded(running).await;
    drop(waiting);

    let (agent, _backend, storage, _hold) = restore_holding(saved).await;
    let owed = agent.owed_turns().await;
    let ids: Vec<_> = owed
        .iter()
        .map(|turn| turn.execution_id().as_str())
        .collect();
    assert_eq!(ids, ["first", "waiting"]);
    assert!(owed[0].dispatched());
    assert!(!owed[1].dispatched());
    let Resumed::Continued(admission) = bounded(agent.resume_turn(&owed[1], resumer()))
        .await
        .unwrap()
    else {
        panic!("a queued turn is continued")
    };
    let text = successor_text(&storage, admission.id());
    assert!(
        text.starts_with("[Nessa restarted before this message reached you"),
        "{text}"
    );
    assert!(text.ends_with("message waiting"), "{text}");
    bounded(agent.close(actor())).await.unwrap();
}

#[tokio::test]
async fn a_turn_cut_off_on_every_start_is_resumed_at_most_three_times() {
    let mut saved = cut_off_while_running("looping").await;
    for attempt in 1..=4 {
        let (agent, backend, storage, _hold) = restore_holding(saved).await;
        let owed = agent.owed_turns().await;
        assert_eq!(owed.len(), 1, "attempt {attempt}");
        let resumed = bounded(agent.resume_turn(&owed[0], resumer()))
            .await
            .unwrap();
        if attempt <= 3 {
            assert!(
                matches!(resumed, Resumed::Continued(_)),
                "attempt {attempt}"
            );
            bounded(backend.dispatched.notified()).await;
        } else {
            assert!(matches!(resumed, Resumed::Exhausted));
            assert_eq!(resumptions(&backend), [TurnResumption::Exhausted]);
            assert!(backend.executions.lock().unwrap().is_empty());
            assert!(agent.owed_turns().await.is_empty());
        }
        saved = MemoryStorage::default();
        saved.0.lock().unwrap().snapshot = Some(storage.snapshot());
        bounded(agent.close(actor())).await.unwrap();
    }
    let (agent, _backend, _storage, _hold) = restore_holding(saved).await;
    assert!(
        agent.owed_turns().await.is_empty(),
        "a spent chain stays ended"
    );
    bounded(agent.close(actor())).await.unwrap();
}

#[tokio::test]
async fn a_retry_of_a_resumed_turn_answers_as_its_successor() {
    let saved = cut_off_while_running("retried").await;
    let (agent, backend, _storage, hold) = restore_holding(saved).await;
    let turn = agent.owed_turns().await.remove(0);
    let Resumed::Continued(successor) = bounded(agent.resume_turn(&turn, resumer())).await.unwrap()
    else {
        panic!("a cut-off turn is continued")
    };
    bounded(backend.dispatched.notified()).await;
    let retry = bounded(agent.enqueue(request("retried"), Submitter::owner(actor())))
        .await
        .unwrap();
    assert_eq!(retry.id().as_str(), "retried");
    hold.send(()).unwrap();
    assert_eq!(bounded(retry.wait()).await, Ok(ExecutionOutcome::Completed));
    assert_eq!(
        bounded(successor.wait()).await,
        Ok(ExecutionOutcome::Completed)
    );
    assert_eq!(
        backend.executions.lock().unwrap().len(),
        1,
        "nothing ran twice"
    );
    bounded(agent.close(actor())).await.unwrap();
}

#[derive(Clone, Copy)]
enum Stop {
    Close,
    Suspend,
    CloseThenSuspend,
}

/// Stop a running turn with `stop`, and copy what it saved.
async fn stopped_storage(stop: Stop) -> MemoryStorage {
    let (agent, backend, storage) = workflow().await;
    *backend.execution_report.lock().unwrap() = Some(ExecutionReport::cancelled_locally(
        CleanupReport::confirmed(CloseOutcome { forced: false }),
    ));
    let (_release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let running = Mode::Queued.start(agent.clone(), request("stopped"));
    bounded(backend.dispatched.notified()).await;
    match stop {
        Stop::Close => {
            bounded(agent.close(actor())).await.unwrap();
        }
        Stop::Suspend => {
            bounded(agent.suspend(resumer())).await.unwrap();
        }
        Stop::CloseThenSuspend => {
            bounded(agent.close(actor())).await.unwrap();
            bounded(agent.suspend(resumer())).await.unwrap();
        }
    }
    assert_eq!(
        bounded(running).await.unwrap(),
        Ok(ExecutionOutcome::Cancelled)
    );
    let saved = MemoryStorage::default();
    saved.0.lock().unwrap().snapshot = Some(storage.snapshot());
    saved
}

/// Stop a running turn with `stop`, then restore what it saved.
async fn stopped_while_running(stop: Stop) -> Vec<OwedTurn> {
    let (agent, _backend, _storage, _hold) = restore_holding(stopped_storage(stop).await).await;
    let owed = agent.owed_turns().await;
    bounded(agent.close(actor())).await.unwrap();
    owed
}

#[tokio::test]
async fn a_suspended_turn_is_owed_and_a_persons_close_is_final() {
    let owed = stopped_while_running(Stop::Suspend).await;
    assert_eq!(owed.len(), 1);
    assert_eq!(owed[0].execution_id().as_str(), "stopped");
    assert!(stopped_while_running(Stop::Close).await.is_empty());
}

#[tokio::test]
async fn a_suspend_after_a_persons_close_keeps_the_close_final() {
    assert!(stopped_while_running(Stop::CloseThenSuspend)
        .await
        .is_empty());
}

#[tokio::test]
async fn a_declined_cut_off_turn_is_recorded_ended_and_no_longer_owed() {
    let saved = cut_off_while_running("declined").await;
    let (agent, backend, storage, _hold) = restore_holding(saved).await;
    let turn = agent.owed_turns().await.remove(0);
    bounded(agent.decline_turn(&turn, resumer())).await.unwrap();
    let snapshot = storage.snapshot();
    assert_eq!(
        last_causes(&snapshot.invocations[0]),
        [SchedulingCause::Interrupted, SchedulingCause::NotResumed]
    );
    let last = snapshot.invocations[0].scheduling.last().unwrap();
    assert_eq!(last.before, Some(InvocationStage::Cancelled));
    assert_eq!(last.actor, Some(resumer()));
    assert_eq!(snapshot.invocations.len(), 1, "nothing was queued");
    assert_eq!(resumptions(&backend), [TurnResumption::Declined]);
    assert!(agent.owed_turns().await.is_empty());
    assert!(backend.executions.lock().unwrap().is_empty());
    bounded(agent.close(actor())).await.unwrap();
    assert_ends_after_restore(&storage).await;
}

#[tokio::test]
async fn a_declined_suspended_turn_is_decided_once() {
    let (agent, backend, storage, _hold) =
        restore_holding(stopped_storage(Stop::Suspend).await).await;
    let turn = agent.owed_turns().await.remove(0);
    bounded(agent.decline_turn(&turn, resumer())).await.unwrap();
    bounded(agent.decline_turn(&turn, resumer())).await.unwrap();
    assert_eq!(
        last_causes(&storage.snapshot().invocations[0]),
        [SchedulingCause::Suspended, SchedulingCause::NotResumed]
    );
    assert_eq!(resumptions(&backend), [TurnResumption::Declined]);
    assert!(agent.owed_turns().await.is_empty());
    bounded(agent.close(actor())).await.unwrap();
    assert_ends_after_restore(&storage).await;
}

#[tokio::test]
async fn a_spent_chain_of_suspended_turns_is_recorded_ended() {
    let mut saved = stopped_storage(Stop::Suspend).await;
    // The original and its three successors are each stopped by a shutdown.
    for attempt in 1..=4 {
        let (agent, backend, storage, _hold) = restore_holding(saved).await;
        *backend.execution_report.lock().unwrap() = Some(ExecutionReport::cancelled_locally(
            CleanupReport::confirmed(CloseOutcome { forced: false }),
        ));
        let owed = agent.owed_turns().await;
        assert_eq!(owed.len(), 1, "attempt {attempt}");
        let turn = &owed[0];
        let resumed = bounded(agent.resume_turn(turn, resumer())).await.unwrap();
        if attempt <= 3 {
            assert!(
                matches!(resumed, Resumed::Continued(_)),
                "attempt {attempt}"
            );
            bounded(backend.dispatched.notified()).await;
            bounded(agent.suspend(resumer())).await.unwrap();
        } else {
            assert!(matches!(resumed, Resumed::Exhausted));
            assert_eq!(resumptions(&backend), [TurnResumption::Exhausted]);
            let snapshot = storage.snapshot();
            let spent = snapshot
                .invocations
                .iter()
                .find(|record| &record.request.execution_id == turn.execution_id())
                .unwrap();
            assert_eq!(
                last_causes(spent),
                [SchedulingCause::Suspended, SchedulingCause::NotResumed]
            );
            assert!(agent.owed_turns().await.is_empty());
            bounded(agent.close(actor())).await.unwrap();
            assert_ends_after_restore(&storage).await;
            return;
        }
        saved = MemoryStorage::default();
        saved.0.lock().unwrap().snapshot = Some(storage.snapshot());
    }
}

fn last_causes(record: &InvocationRecord) -> Vec<SchedulingCause> {
    let causes: Vec<_> = record.scheduling.iter().map(|edge| edge.cause).collect();
    causes[causes.len() - 2..].to_vec()
}

/// A decided turn stays ended in the next process too.
async fn assert_ends_after_restore(storage: &MemoryStorage) {
    let saved = MemoryStorage::default();
    saved.0.lock().unwrap().snapshot = Some(storage.snapshot());
    let (agent, _backend, _storage, _hold) = restore_holding(saved).await;
    assert!(
        agent.owed_turns().await.is_empty(),
        "decided once, for good"
    );
    bounded(agent.close(actor())).await.unwrap();
}

#[tokio::test]
async fn a_successor_whose_audit_record_fails_still_runs_and_says_so() {
    let saved = cut_off_while_running("unaudited").await;
    let (agent, backend, _storage, hold) = restore_holding(saved).await;
    let turn = agent.owed_turns().await.remove(0);
    backend.audit.reject.store(true, Ordering::SeqCst);
    let Resumed::ContinuedUnaudited { admission, error } =
        bounded(agent.resume_turn(&turn, resumer())).await.unwrap()
    else {
        panic!("the successor is queued whether or not its record saved")
    };
    assert_eq!(error, AgentError::AuditFailure);
    backend.audit.reject.store(false, Ordering::SeqCst);
    // The host still has the receipt to watch. What the turn then comes to
    // is the agent's own audit rule (the same sink also refused the
    // successor's admission record); the original is not owed again, and
    // a successor that failed this way is owed in its place.
    let successor = admission.id().clone();
    let _ = hold.send(());
    let _ = bounded(admission.wait()).await;
    let owed = agent.owed_turns().await;
    assert!(
        owed.iter().all(|turn| turn.execution_id() == &successor),
        "{:?}",
        owed_ids(&owed)
    );
    bounded(agent.close(actor())).await.unwrap();
}

/// Run `failed` until the provider fails it with `error`, with `behind`
/// queued after it, and wait for both receipts.
async fn failed_with(error: AgentError) -> (Agent, Arc<WorkflowBackend>, MemoryStorage) {
    let (agent, backend, storage) = workflow().await;
    *backend.execution_fault.lock().unwrap() = Some(ProviderOperationFailure::new(
        error.clone(),
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    ));
    let (release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let failed = Mode::Queued.start(agent.clone(), request("failed"));
    bounded(backend.dispatched.notified()).await;
    let behind = agent
        .enqueue(request("behind"), Submitter::owner(actor()))
        .await
        .unwrap();
    release.send(()).unwrap();
    assert_eq!(bounded(failed).await.unwrap(), Err(error));
    let _ = bounded(behind.wait()).await;
    *backend.execution_fault.lock().unwrap() = None;
    (agent, backend, storage)
}

fn owed_ids(owed: &[OwedTurn]) -> Vec<&str> {
    owed.iter()
        .map(|turn| turn.execution_id().as_str())
        .collect()
}

#[tokio::test]
async fn a_turn_the_provider_failed_and_the_turn_behind_it_are_sent_again() {
    let (agent, backend, storage) = failed_with(AgentError::Provider {
        code: -32077,
        diagnostic: None,
    })
    .await;
    let owed = agent.owed_turns().await;
    assert_eq!(owed_ids(&owed), ["failed", "behind"]);
    assert!(owed[0].dispatched());
    assert!(!owed[1].dispatched());
    let mut successors = Vec::new();
    for turn in &owed {
        let Resumed::Continued(admission) =
            bounded(agent.resume_turn(turn, resumer())).await.unwrap()
        else {
            panic!("a failed turn is continued")
        };
        successors.push(admission);
    }
    let texts: Vec<_> = successors
        .iter()
        .map(|admission| successor_text(&storage, admission.id()))
        .collect();
    assert!(
        texts[0].starts_with("[Your last turn on this request ended early"),
        "{}",
        texts[0]
    );
    assert!(texts[0].ends_with("message failed"), "{}", texts[0]);
    assert!(
        texts[1].starts_with("[This message did not reach you"),
        "{}",
        texts[1]
    );
    for admission in successors {
        assert_eq!(
            bounded(admission.wait()).await,
            Ok(ExecutionOutcome::Completed)
        );
    }
    assert!(agent.owed_turns().await.is_empty());
    assert_eq!(resumptions(&backend).len(), 2);
    bounded(agent.close(actor())).await.unwrap();
}

#[tokio::test]
async fn a_failed_turn_is_owed_after_a_restart_too() {
    let (agent, _backend, storage) = failed_with(AgentError::Transport("pipe".into())).await;
    bounded(agent.close(actor())).await.unwrap();
    let saved = MemoryStorage::default();
    saved.0.lock().unwrap().snapshot = Some(storage.snapshot());
    let (agent, _backend, _storage, _hold) = restore_holding(saved).await;
    assert_eq!(owed_ids(&agent.owed_turns().await), ["failed", "behind"]);
    bounded(agent.close(actor())).await.unwrap();
}

#[tokio::test]
async fn a_failure_a_retry_cannot_fix_ends_the_turn() {
    let (agent, _backend, _storage) =
        failed_with(AgentError::AuthenticationRequired { diagnostic: None }).await;
    assert_eq!(
        owed_ids(&agent.owed_turns().await),
        ["behind"],
        "only the turn the failure kept from the provider is owed"
    );
    bounded(agent.close(actor())).await.unwrap();
}

#[tokio::test]
async fn a_declined_failed_turn_is_recorded_ended() {
    let (agent, backend, storage) = failed_with(AgentError::Deadline).await;
    let owed = agent.owed_turns().await;
    for turn in &owed {
        bounded(agent.decline_turn(turn, resumer())).await.unwrap();
    }
    let snapshot = storage.snapshot();
    assert_eq!(
        last_causes(&snapshot.invocations[0]),
        [
            SchedulingCause::ExecutionFailed,
            SchedulingCause::NotResumed
        ]
    );
    assert_eq!(
        last_causes(&snapshot.invocations[1]),
        [SchedulingCause::RunnerStopped, SchedulingCause::NotResumed]
    );
    assert_eq!(
        resumptions(&backend),
        [TurnResumption::Declined, TurnResumption::Declined]
    );
    assert!(agent.owed_turns().await.is_empty());
    bounded(agent.close(actor())).await.unwrap();
    assert_ends_after_restore(&storage).await;
}

#[tokio::test]
async fn a_failed_turn_a_later_message_moved_past_is_not_sent_again() {
    let (agent, _backend, storage) = failed_with(AgentError::Provider {
        code: -32077,
        diagnostic: None,
    })
    .await;
    let next = agent
        .enqueue(request("next"), Submitter::owner(actor()))
        .await
        .unwrap();
    assert_eq!(bounded(next.wait()).await, Ok(ExecutionOutcome::Completed));
    assert!(
        agent.owed_turns().await.is_empty(),
        "the person moved on: neither the failed turn nor the one behind it is sent again"
    );
    bounded(agent.close(actor())).await.unwrap();
    let saved = MemoryStorage::default();
    saved.0.lock().unwrap().snapshot = Some(storage.snapshot());
    let (agent, _backend, _storage, _hold) = restore_holding(saved).await;
    assert!(agent.owed_turns().await.is_empty(), "nor after a restart");
    bounded(agent.close(actor())).await.unwrap();
}

#[tokio::test]
async fn a_retry_of_a_failed_turn_answers_as_its_successor() {
    let (agent, _backend, _storage) = failed_with(AgentError::Transport("pipe".into())).await;
    let owed = agent.owed_turns().await;
    let Resumed::Continued(successor) = bounded(agent.resume_turn(&owed[0], resumer()))
        .await
        .unwrap()
    else {
        panic!("a failed turn is continued")
    };
    let retry = bounded(agent.enqueue(request("failed"), Submitter::owner(actor())))
        .await
        .unwrap();
    assert_eq!(retry.id().as_str(), "failed");
    assert_eq!(
        bounded(retry.wait()).await,
        Ok(ExecutionOutcome::Completed),
        "not the original's failure"
    );
    assert_eq!(
        bounded(successor.wait()).await,
        Ok(ExecutionOutcome::Completed)
    );
    bounded(agent.close(actor())).await.unwrap();
}

#[test]
fn a_failure_that_lost_its_evidence_is_not_retried() {
    let provider = || AgentError::Provider {
        code: -32077,
        diagnostic: None,
    };
    assert!(AgentError::AttachmentUnavailable(AttachmentPhase::Failed(
        AttachmentFailureCode::Provider
    ))
    .leaves_work_owed());
    for code in [AttachmentFailureCode::Audit, AttachmentFailureCode::Cleanup] {
        assert!(
            !AgentError::AttachmentUnavailable(AttachmentPhase::Failed(code)).leaves_work_owed()
        );
    }
    assert!(AgentError::MultipleOperationFailures {
        first_error: Box::new(provider()),
        subsequent_error: Box::new(AgentError::Deadline),
    }
    .leaves_work_owed());
    assert!(!AgentError::MultipleOperationFailures {
        first_error: Box::new(provider()),
        subsequent_error: Box::new(AgentError::AuditFailure),
    }
    .leaves_work_owed());
    assert!(!AgentError::OperationAndCleanupFailure {
        operation_error: Box::new(provider()),
        cleanup_error: Box::new(AgentError::CleanupUncertain),
    }
    .leaves_work_owed());
}

#[tokio::test]
async fn a_later_message_that_also_failed_still_moves_past_the_first() {
    let error = AgentError::Provider {
        code: -32077,
        diagnostic: None,
    };
    let (agent, backend, _storage) = failed_with(error.clone()).await;
    *backend.execution_fault.lock().unwrap() = Some(ProviderOperationFailure::new(
        error.clone(),
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    ));
    let next = agent
        .enqueue(request("next"), Submitter::owner(actor()))
        .await
        .unwrap();
    assert_eq!(bounded(next.wait()).await, Err(error));
    *backend.execution_fault.lock().unwrap() = None;
    assert_eq!(
        owed_ids(&agent.owed_turns().await),
        ["next"],
        "only the request the person moved on to is sent again"
    );
    bounded(agent.close(actor())).await.unwrap();
}

#[tokio::test]
async fn a_retry_answers_as_the_successor_that_ended_the_chain() {
    let (agent, backend, _storage) = failed_with(AgentError::Transport("pipe".into())).await;
    let owed = agent.owed_turns().await;
    let refused = AgentError::AuthenticationRequired { diagnostic: None };
    *backend.execution_fault.lock().unwrap() = Some(ProviderOperationFailure::new(
        refused.clone(),
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    ));
    let Resumed::Continued(successor) = bounded(agent.resume_turn(&owed[0], resumer()))
        .await
        .unwrap()
    else {
        panic!("a failed turn is continued")
    };
    assert_eq!(bounded(successor.wait()).await, Err(refused.clone()));
    *backend.execution_fault.lock().unwrap() = None;
    let retry = bounded(agent.enqueue(request("failed"), Submitter::owner(actor())))
        .await
        .unwrap();
    assert_eq!(
        bounded(retry.wait()).await,
        Err(refused),
        "the successor's ending, not the original's"
    );
    bounded(agent.close(actor())).await.unwrap();
}
