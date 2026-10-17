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

/// Once successor admission has reached its audit, losing the public caller
/// must not discard the owned handoff to the runner or its decision audit.
#[tokio::test]
async fn a_resumption_keeps_its_successor_after_its_caller_is_lost_during_admission_audit() {
    let saved = cut_off_while_running("lost-resumer").await;
    let (agent, backend, storage, hold) = restore_holding(saved).await;
    let turn = agent.owed_turns().await.remove(0);
    let (entered, waiting) = oneshot::channel();
    let (release, gate) = oneshot::channel();
    *backend.audit.pause.lock().unwrap() = Some((entered, gate));
    let resuming = {
        let agent = agent.clone();
        tokio::spawn(async move { agent.resume_turn(&turn, resumer()).await })
    };
    bounded(waiting).await.unwrap();
    let successor = {
        let records = backend.audit.records.lock().unwrap();
        match records.last().unwrap() {
            ExecutionAuditRecord::QueueAdmitted(record) => record.execution_id().clone(),
            _ => panic!("the successor's queue admission is the held audit"),
        }
    };
    assert_ne!(successor.as_str(), "lost-resumer");
    assert_eq!(storage.snapshot().invocations.len(), 2);
    assert!(backend.executions.lock().unwrap().is_empty());
    resuming.abort();
    match resuming.await {
        Err(error) => assert!(error.is_cancelled()),
        Ok(_) => panic!("the response caller was not cancelled"),
    }
    release
        .send(())
        .expect("the admitted resumption retains its audit after caller loss");
    // No enqueue, manual runner wake, or second resume starts this successor.
    bounded(backend.dispatched.notified()).await;
    assert_eq!(*backend.executions.lock().unwrap(), vec![successor.clone()]);
    let retry = bounded(agent.enqueue(request("lost-resumer"), Submitter::owner(actor())))
        .await
        .unwrap();
    hold.send(()).unwrap();
    assert_eq!(bounded(retry.wait()).await, Ok(ExecutionOutcome::Completed));
    assert_eq!(storage.snapshot().invocations.len(), 2);
    assert_eq!(
        resumptions(&backend),
        [TurnResumption::Continued { successor }]
    );
    bounded(agent.close(actor())).await.unwrap();
}

/// Queue the second caller on the real scheduler while the first caller's
/// interrupted-edge save is held. The scheduler's FIFO handoff exposes a gap
/// between deciding the old turn and admitting its successor.
#[tokio::test]
async fn concurrent_resumptions_of_one_owed_turn_make_one_decision() {
    let saved = cut_off_while_running("racing-resumers").await;
    let (agent, backend, storage, hold) = restore_holding(saved).await;
    let turn = agent.owed_turns().await.remove(0);
    let (saving, release) = storage.pause_next_save();
    let first = {
        let agent = agent.clone();
        let turn = turn.clone();
        tokio::spawn(async move { agent.resume_turn(&turn, resumer()).await })
    };
    bounded(saving).await.unwrap();
    let mut second = Box::pin(agent.resume_turn(&turn, resumer()));
    poll_fn(|cx| {
        assert!(second.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    release.send(()).unwrap();
    let (first, second) = bounded(async { tokio::join!(first, second) }).await;
    let first = first.unwrap().unwrap();
    let second = second.unwrap();
    let decisions = [first, second];
    assert_eq!(
        decisions
            .iter()
            .filter(|result| matches!(result, Resumed::Continued(_)))
            .count(),
        1
    );
    assert_eq!(
        decisions
            .iter()
            .filter(|result| matches!(result, Resumed::NotOwed))
            .count(),
        1
    );
    assert_eq!(storage.snapshot().invocations.len(), 2);
    assert_eq!(resumptions(&backend).len(), 1);
    bounded(backend.dispatched.notified()).await;
    assert_eq!(backend.executions.lock().unwrap().len(), 1);
    hold.send(()).unwrap();
    for decision in decisions {
        if let Resumed::Continued(admission) = decision {
            assert_eq!(
                bounded(admission.wait()).await,
                Ok(ExecutionOutcome::Completed)
            );
        }
    }
    bounded(agent.close(actor())).await.unwrap();
}

/// A decline queued behind an already-started resumption cannot slip between
/// its Interrupted decision and successor admission and make two decisions.
#[tokio::test]
async fn a_decline_racing_a_resumption_does_not_decide_the_turn_twice() {
    let saved = cut_off_while_running("resume-or-decline").await;
    let (agent, backend, storage, hold) = restore_holding(saved).await;
    let turn = agent.owed_turns().await.remove(0);
    let (saving, release) = storage.pause_next_save();
    let resuming = {
        let agent = agent.clone();
        let turn = turn.clone();
        tokio::spawn(async move { agent.resume_turn(&turn, resumer()).await })
    };
    bounded(saving).await.unwrap();
    let mut declining = Box::pin(agent.decline_turn(&turn, resumer()));
    poll_fn(|cx| {
        assert!(declining.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    release.send(()).unwrap();
    let (resumed, declined) = bounded(async { tokio::join!(resuming, declining) }).await;
    let Resumed::Continued(admission) = resumed.unwrap().unwrap() else {
        panic!("the first decision continues the turn");
    };
    declined.unwrap();
    let successor = admission.id().clone();
    assert_eq!(
        resumptions(&backend),
        [TurnResumption::Continued {
            successor: successor.clone()
        }]
    );
    let snapshot = storage.snapshot();
    assert_eq!(snapshot.invocations.len(), 2);
    assert_eq!(
        snapshot.invocations[0].scheduling.last().unwrap().cause,
        SchedulingCause::Interrupted
    );
    bounded(backend.dispatched.notified()).await;
    assert_eq!(*backend.executions.lock().unwrap(), vec![successor]);
    hold.send(()).unwrap();
    assert_eq!(
        bounded(admission.wait()).await,
        Ok(ExecutionOutcome::Completed)
    );
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
    bounded(agent.suspend(resumer())).await.unwrap();
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

/// An identity that merely looks derived belongs to its own saved submission.
#[tokio::test]
async fn an_unrelated_derived_identity_does_not_own_a_cut_off_turn() {
    let root = ExecutionId::new("occupied-root").unwrap();
    let occupied = Resumption::of(&root).successor().unwrap();
    let (agent, backend, storage) = workflow().await;
    let stranger = ActionContext::new("other-person", "test", "preoccupy").unwrap();
    let foreign = agent
        .enqueue(
            request(occupied.as_str()),
            Submitter::owner(stranger.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        bounded(foreign.wait()).await,
        Ok(ExecutionOutcome::Completed)
    );
    bounded(backend.dispatched.notified()).await;
    let (_release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let running = Mode::Queued.start(agent.clone(), request(root.as_str()));
    bounded(backend.dispatched.notified()).await;
    let saved = MemoryStorage::default();
    saved.0.lock().unwrap().snapshot = Some(storage.snapshot());
    bounded(agent.close(actor())).await.unwrap();
    let _ = bounded(running).await;

    let (agent, backend, storage, _hold) = restore_holding(saved).await;
    let turn = agent
        .owed_turns()
        .await
        .into_iter()
        .find(|turn| turn.execution_id() == &root)
        .expect("the unrelated occupied identity cannot suppress the original owed turn");
    assert!(matches!(
        bounded(agent.resume_turn(&turn, resumer())).await,
        Err(AgentError::SubmissionConflict)
    ));
    assert!(agent
        .owed_turns()
        .await
        .iter()
        .any(|turn| turn.execution_id() == &root));
    let retry =
        match bounded(agent.enqueue(request(root.as_str()), Submitter::owner(actor()))).await {
            Ok(admission) => bounded(admission.wait()).await,
            Err(error) => Err(error),
        };
    assert_eq!(
        retry,
        Err(AgentError::Closed),
        "the original retry cannot inherit the stranger's completed receipt"
    );
    assert!(backend.executions.lock().unwrap().is_empty());
    let snapshot = storage.snapshot();
    let foreign = snapshot
        .invocations
        .iter()
        .find(|record| record.request.execution_id == occupied)
        .unwrap();
    assert_eq!(foreign.actor, stranger);
    assert_eq!(foreign.request, request(occupied.as_str()));
    bounded(agent.close(actor())).await.unwrap();
}

#[tokio::test]
async fn a_canonical_resume_shaped_root_still_gets_its_own_successor() {
    let root = format!("resume-3-{}", "a".repeat(64));
    let saved = cut_off_while_running(&root).await;
    let (agent, backend, storage, _hold) = restore_holding(saved).await;
    let turn = agent.owed_turns().await.remove(0);
    let Resumed::Continued(admission) = bounded(agent.resume_turn(&turn, resumer())).await.unwrap()
    else {
        panic!("an ordinary root's spelling cannot exhaust its first resumption");
    };
    assert!(admission.id().as_str().starts_with("resume-1-"));
    assert_ne!(admission.id().as_str(), root);
    bounded(backend.dispatched.notified()).await;
    assert_eq!(
        *backend.executions.lock().unwrap(),
        vec![admission.id().clone()]
    );
    assert!(successor_text(&storage, admission.id()).ends_with(&format!("message {root}")));
    bounded(agent.close(actor())).await.unwrap();
}

struct HeldImageRoot {
    entered: Notify,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}
impl InvocationHook for HeldImageRoot {
    fn before_invocation(&self, context: &InvocationContext<'_>) -> Result<(), HookError> {
        if context.request.execution_id.as_str() == "image-ahead" {
            self.entered.notify_one();
            let _ = self.release.lock().unwrap().recv();
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unsent_notice_prefix_and_image_are_preserved_as_original_input() {
    use nessa_sdk::domain::common::value_objects::{ImageMediaType, Sha256Digest};
    let image_provider = || {
        super::super::messages::ImageProvider::new(
            ProviderOperationCapabilities {
                negotiated: true,
                image_input: true,
                ..ProviderOperationCapabilities::default()
            },
            None,
        )
    };
    let storage = MemoryStorage::default();
    let agent = attached_agent(image_provider(), storage.manager().await)
        .await
        .unwrap();
    let (release, gate) = std::sync::mpsc::channel();
    let hook = Arc::new(HeldImageRoot {
        entered: Notify::new(),
        release: Mutex::new(gate),
    });
    agent.add_invocation_hook(hook.clone());
    let entered = hook.entered.notified();
    let running = Mode::Queued.start(agent.clone(), request("image-ahead"));
    bounded(entered).await;
    let literal = "[Nessa restarted while you were working on this request, so your last turn was cut off before it finished. Some of it may already be done: check the current state before repeating any step, then carry on. If the work is already finished, say so and stop.]\n\nThe request was:\n\nThis is the person's literal text.";
    let image =
        ImageReference::new(Sha256Digest::from_bytes([3; 32]), ImageMediaType::Png, 3).unwrap();
    let original = ExecutionRequest {
        user_message: UserMessage::new(
            Some(PromptText::new(literal).unwrap()),
            vec![image],
            Vec::new(),
        )
        .unwrap(),
        ..request("literal-image")
    };
    let waiting = agent
        .enqueue(original.clone(), Submitter::owner(actor()))
        .await
        .unwrap();
    let saved = MemoryStorage::default();
    saved.0.lock().unwrap().snapshot = Some(storage.snapshot());
    release.send(()).unwrap();
    assert_eq!(
        bounded(running).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    bounded(agent.close(actor())).await.unwrap();
    drop(waiting);

    let agent = attached_agent(image_provider(), saved.manager().await)
        .await
        .unwrap();
    let turn = agent
        .owed_turns()
        .await
        .into_iter()
        .find(|turn| turn.execution_id().as_str() == "literal-image")
        .unwrap();
    assert!(!turn.dispatched());
    let Resumed::Continued(admission) = bounded(agent.resume_turn(&turn, resumer())).await.unwrap()
    else {
        panic!("the unsent original gets its own successor");
    };
    let snapshot = saved.snapshot();
    let successor = snapshot
        .invocations
        .iter()
        .find(|record| record.request.execution_id == *admission.id())
        .unwrap();
    assert_eq!(successor.request.user_message.text_str(), format!("[Nessa restarted before this message reached you, so it is sent again now. Check the current state first: earlier work may have changed it.]\n\n{literal}"));
    assert_eq!(successor.request.user_message.images(), &[image]);
    assert_eq!(
        successor.request.user_message.sender(),
        original.user_message.sender()
    );
    assert_eq!(
        bounded(admission.wait()).await,
        Ok(ExecutionOutcome::Completed)
    );
    bounded(agent.close(actor())).await.unwrap();
}

/// Preserve the real failed-save uncertainty: only the reconciliation read
/// after a delegated write error is refused; all actual storage stays owned.
#[derive(Clone)]
struct FailedReconciliationStorage(MemoryStorage);
struct FailedReconciliationLease {
    inner: Box<dyn SessionStorageLease>,
    fail_load: std::sync::atomic::AtomicBool,
}
impl SessionStorage for FailedReconciliationStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(FailedReconciliationLease {
                inner: self.0.open(id).await?,
                fail_load: std::sync::atomic::AtomicBool::new(false),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for FailedReconciliationLease {
    fn load(&self) -> StorageFuture<'_, SessionLoad> {
        Box::pin(async move {
            if self.fail_load.swap(false, Ordering::SeqCst) {
                Err(StorageError::Io(
                    "fixture reconciliation read failed".into(),
                ))
            } else {
                self.inner.load().await
            }
        })
    }
    fn save_changes(
        &self,
        binding: SessionSaveGeneration,
        snapshot: SessionSnapshot,
        units: Vec<SessionSaveUnit>,
    ) -> StorageFuture<'_, SessionSaveReceipt> {
        Box::pin(async move {
            let result = self.inner.save_changes(binding, snapshot, units).await;
            if result.is_err() {
                self.fail_load.store(true, Ordering::SeqCst);
            }
            result
        })
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.inner.erase()
    }
}

#[tokio::test]
async fn an_unacknowledged_successor_does_not_end_the_committed_roots_work() {
    let storage = cut_off_while_running("unacknowledged-root").await;
    let manager = SessionManager::open(
        Some(SessionId::new("conversation").unwrap()),
        Arc::new(FailedReconciliationStorage(storage.clone())),
        Arc::new(nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new()),
    )
    .await
    .unwrap();
    let backend = workflow_backend();
    let agent = attached_agent(Arc::new(WorkflowProvider(backend.clone())), manager)
        .await
        .unwrap();
    let turn = agent.owed_turns().await.remove(0);
    let root = turn.execution_id().clone();
    let successor = Resumption::of(&root).successor().unwrap();
    let (saving, release) = storage.pause_invocation_save(successor.as_str());
    let resuming = {
        let agent = agent.clone();
        tokio::spawn(async move { agent.resume_turn(&turn, resumer()).await })
    };
    bounded(saving).await.unwrap();
    storage.fail_next();
    release.send(()).unwrap();
    assert!(matches!(
        bounded(resuming).await.unwrap(),
        Err(AgentError::Storage(StorageError::Io(_)))
    ));
    let frame = bounded(agent.resumption_history()).await.unwrap();
    assert!(
        frame.is_owed(&root),
        "the committed predecessor still owes its work"
    );
    assert!(
        frame.has_observed(&successor),
        "the real failed save retains the observed successor while reconciliation is unavailable"
    );
    assert!(!frame
        .invocations()
        .iter()
        .any(|record| record.request.execution_id == successor));
    assert!(
        backend.executions.lock().unwrap().is_empty(),
        "unacknowledged admission never dispatched"
    );
    bounded(agent.close(actor())).await.unwrap();
}

#[tokio::test]
async fn a_persons_close_ends_failed_work_in_history_and_after_restore() {
    let (agent, backend, storage) =
        failed_with(AgentError::Transport("retryable failure".into())).await;
    assert_eq!(owed_ids(&agent.owed_turns().await), ["failed", "behind"]);
    let executions = backend.executions.lock().unwrap().clone();
    assert_eq!(
        bounded(agent.close(actor())).await,
        Ok(CloseOutcome { forced: false })
    );
    assert!(agent.owed_turns().await.is_empty());
    let snapshot = storage.snapshot();
    assert_eq!(snapshot.invocations.len(), 2);
    for record in &snapshot.invocations {
        let final_edge = record.scheduling.last().unwrap();
        assert_eq!(final_edge.cause, SchedulingCause::NotResumed);
        assert_eq!(final_edge.actor, Some(actor()));
    }
    assert_eq!(*backend.executions.lock().unwrap(), executions);
    let restored = MemoryStorage::default();
    restored.0.lock().unwrap().snapshot = Some(snapshot);
    let (restored, backend, _storage, _hold) = restore_holding(restored).await;
    assert!(restored.owed_turns().await.is_empty());
    assert!(backend.executions.lock().unwrap().is_empty());
    bounded(restored.close(actor())).await.unwrap();
}

#[tokio::test]
async fn a_failed_close_keeps_its_first_failure_when_storage_stays_unavailable() {
    for unconfirmed in [false, true] {
        let (agent, backend, storage) = workflow().await;
        if unconfirmed {
            *backend.cleanup.lock().unwrap() =
                CleanupReport::unconfirmed(AgentError::CleanupUncertain);
        }
        let (release, gate) = oneshot::channel();
        *backend.execution_gate.lock().unwrap() = Some(gate);
        let running = Mode::Queued.start(agent.clone(), request("running"));
        bounded(backend.dispatched.notified()).await;
        let behind = bounded(agent.enqueue(request("behind"), Submitter::owner(actor())))
            .await
            .unwrap();
        let writes = storage.0.lock().unwrap().writes;
        storage.fail_writes(true);
        let closing_agent = agent.clone();
        let closing = tokio::spawn(async move { closing_agent.close(actor()).await });
        // Wait for the queued cancellation's real failed save before allowing
        // the provider to finish. The queued turn cannot race into dispatch.
        bounded(async {
            while storage.0.lock().unwrap().writes == writes {
                tokio::task::yield_now().await;
            }
        })
        .await;
        let _ = release.send(());
        let first_close = bounded(closing).await.unwrap();
        let _ = bounded(running).await.unwrap();
        let _ = bounded(behind.wait()).await;
        assert_eq!(
            backend.executions.lock().unwrap().as_slice(),
            [ExecutionId::new("running").unwrap()]
        );
        assert!(!backend.shutdowns.lock().unwrap().is_empty());

        // Restore storage and confirm the physical owner before inspecting the
        // first error, so the red regression also leaves no held resources.
        storage.fail_writes(false);
        *backend.cleanup.lock().unwrap() = CleanupReport::confirmed(CloseOutcome { forced: false });
        assert_eq!(
            bounded(agent.close(actor())).await,
            Ok(CloseOutcome { forced: false })
        );
        assert!(agent.owed_turns().await.is_empty());
        assert_eq!(
            backend.executions.lock().unwrap().as_slice(),
            [ExecutionId::new("running").unwrap()]
        );
        let snapshot = storage.snapshot();
        assert_eq!(snapshot.invocations.len(), 2);
        assert!(snapshot
            .invocations
            .iter()
            .all(|record| record.result.is_some()));
        let restored = MemoryStorage::default();
        restored.0.lock().unwrap().snapshot = Some(snapshot);
        let (restored, restored_backend, _storage, _hold) = restore_holding(restored).await;
        assert!(restored.owed_turns().await.is_empty());
        assert!(restored_backend.executions.lock().unwrap().is_empty());
        bounded(restored.close(actor())).await.unwrap();

        // Cancellation already returned both failed writes. A later finality
        // attempt must not reclassify that operation error as physical cleanup.
        assert_eq!(
            first_close,
            Err(AgentError::MultipleOperationFailures {
                first_error: Box::new(AgentError::Storage(StorageError::Io(
                    "fixture write failed".into(),
                ))),
                subsequent_error: Box::new(AgentError::Storage(StorageError::Io(
                    "fixture write failed".into(),
                ))),
            }),
            "unconfirmed={unconfirmed}"
        );
    }
}

#[tokio::test]
async fn a_failed_final_decline_keeps_cleanup_and_a_close_retry_saves_finality() {
    let (agent, backend, storage) =
        failed_with(AgentError::Transport("retryable failure".into())).await;
    assert_eq!(owed_ids(&agent.owed_turns().await), ["failed", "behind"]);
    let executions = backend.executions.lock().unwrap().clone();
    storage.fail_scheduling_cause("behind", SchedulingCause::NotResumed);
    let error = bounded(agent.close(actor())).await.unwrap_err();
    let AgentError::StorageDuringClose {
        error,
        cleanup_result,
    } = error
    else {
        panic!("failed finality storage must retain its separate cleanup result");
    };
    assert!(matches!(error, StorageError::Io(_)));
    assert_eq!(*cleanup_result, Ok(CloseOutcome { forced: false }));
    assert_ne!(
        storage.snapshot().invocations[1]
            .scheduling
            .last()
            .unwrap()
            .cause,
        SchedulingCause::NotResumed
    );
    assert_eq!(
        storage.snapshot().invocations[0]
            .scheduling
            .last()
            .unwrap()
            .cause,
        SchedulingCause::NotResumed
    );
    let frame = bounded(agent.resumption_history()).await.unwrap();
    assert!(!frame.is_owed(&ExecutionId::new("failed").unwrap()));
    assert!(
        frame.is_owed(&ExecutionId::new("behind").unwrap()),
        "unacknowledged finality is not committed finality"
    );
    assert_eq!(
        bounded(agent.close(actor())).await,
        Ok(CloseOutcome { forced: false })
    );
    assert!(agent.owed_turns().await.is_empty());
    let snapshot = storage.snapshot();
    for record in &snapshot.invocations {
        assert_eq!(
            record.scheduling.last().unwrap().cause,
            SchedulingCause::NotResumed
        );
    }
    assert_eq!(*backend.executions.lock().unwrap(), executions);
    let restored = MemoryStorage::default();
    restored.0.lock().unwrap().snapshot = Some(snapshot);
    let (restored, backend, _storage, _hold) = restore_holding(restored).await;
    assert!(restored.owed_turns().await.is_empty());
    assert!(backend.executions.lock().unwrap().is_empty());
    bounded(restored.close(actor())).await.unwrap();
}

#[tokio::test]
async fn a_later_decline_keeps_its_actor_separate_from_the_original_suspension() {
    let saved = stopped_storage(Stop::Suspend).await;
    let (agent, backend, storage, _hold) = restore_holding(saved).await;
    let turn = agent.owed_turns().await.remove(0);
    let decision = actor();
    assert_ne!(decision, resumer());
    bounded(agent.decline_turn(&turn, decision.clone()))
        .await
        .unwrap();
    let snapshot = storage.snapshot();
    let record = &snapshot.invocations[0];
    let stopped = record.local_cancellation.as_ref().unwrap();
    assert_eq!(stopped.cause, SchedulingCause::Suspended);
    assert_eq!(stopped.actor, Some(resumer()));
    let original = record
        .scheduling
        .iter()
        .find(|edge| edge.cause == SchedulingCause::Suspended)
        .unwrap();
    assert_eq!(original.actor, Some(resumer()));
    let declined = record.scheduling.last().unwrap();
    assert_eq!(declined.cause, SchedulingCause::NotResumed);
    assert_eq!(declined.before, Some(InvocationStage::Cancelled));
    assert_eq!(declined.actor, Some(decision.clone()));
    assert!(agent.owed_turns().await.is_empty());
    assert_eq!(resumptions(&backend), [TurnResumption::Declined]);
    assert!(backend.executions.lock().unwrap().is_empty());
    bounded(agent.close(actor())).await.unwrap();

    let saved = MemoryStorage::default();
    saved.0.lock().unwrap().snapshot = Some(storage.snapshot());
    let (agent, backend, storage, _hold) = restore_holding(saved).await;
    assert!(agent.owed_turns().await.is_empty());
    assert!(backend.executions.lock().unwrap().is_empty());
    let restored = storage.snapshot();
    let record = &restored.invocations[0];
    assert_eq!(
        record.local_cancellation.as_ref().unwrap().actor,
        Some(resumer())
    );
    assert_eq!(
        record
            .scheduling
            .iter()
            .find(|edge| edge.cause == SchedulingCause::Suspended)
            .unwrap()
            .actor,
        Some(resumer())
    );
    assert_eq!(record.scheduling.last().unwrap().actor, Some(decision));
    bounded(agent.close(actor())).await.unwrap();
}

/// A later saved candidate must match every inherited submission fact, not
/// merely the derived identity and saved order. The actual resume API supplies
/// the otherwise authentic request used by each independent variant.
#[tokio::test]
async fn a_later_successor_candidate_must_match_text_actor_initiation_and_mode() {
    let original = stopped_storage(Stop::Suspend).await.snapshot();
    let root = original.invocations[0].request.execution_id.clone();
    let original_request = original.invocations[0].request.clone();
    let original_actor = original.invocations[0].actor.clone();
    let original_initiation = original.invocations[0].initiation.clone();
    let reference = MemoryStorage::default();
    reference.0.lock().unwrap().snapshot = Some(original.clone());
    let (reference_agent, reference_backend, reference, reference_hold) =
        restore_holding(reference).await;
    let turn = reference_agent.owed_turns().await.remove(0);
    let Resumed::Continued(admission) = bounded(reference_agent.resume_turn(&turn, resumer()))
        .await
        .unwrap()
    else {
        panic!("the actual resume API supplies the authentic successor reference");
    };
    let reference_snapshot = reference.snapshot();
    let reference_record = reference_snapshot
        .invocations
        .iter()
        .find(|record| record.request.execution_id == *admission.id())
        .unwrap();
    let candidate_request = reference_record.request.clone();
    assert_eq!(reference_record.actor, original_actor);
    assert_eq!(reference_record.initiation, original_initiation);
    assert_eq!(reference_record.submission, SubmissionMode::Queued);
    // Establish dispatch before releasing the fixture's provider gate, then
    // join its receipt. Closing before that checkpoint can race the fixture's
    // subscription to its close notification while this gate is still held.
    timeout(
        Duration::from_secs(2),
        reference_backend.dispatched.notified(),
    )
    .await
    .expect("authentic reference successor reached the provider dispatch checkpoint");
    reference_hold.send(()).unwrap();
    assert_eq!(
        bounded(admission.wait()).await,
        Ok(ExecutionOutcome::Completed)
    );
    bounded(reference_agent.close(actor())).await.unwrap();

    for variant in ["text", "actor", "initiation", "mode"] {
        let saved = MemoryStorage::default();
        saved.0.lock().unwrap().snapshot = Some(original.clone());
        let (agent, backend, _storage, hold) = restore_holding(saved).await;
        let mut input = candidate_request.clone();
        let mut caller = original_actor.clone();
        let mut initiation = original_initiation.clone();
        match variant {
            "text" => {
                input.user_message = UserMessage::text_only(
                    PromptText::new(format!("{} changed", input.user_message.text_str())).unwrap(),
                )
            }
            "actor" => caller = ActionContext::new("foreign", "test", "candidate").unwrap(),
            "initiation" => {
                initiation = Initiation::checked_grant(GrantInitiator::new("foreign", 7).unwrap())
            }
            "mode" => {}
            _ => unreachable!(),
        }
        let submitter = Submitter::new(caller, initiation);
        let candidate = if variant == "mode" {
            bounded(agent.enqueue_steering(input, submitter))
                .await
                .unwrap()
        } else {
            bounded(agent.enqueue(input, submitter)).await.unwrap()
        };
        if variant == "initiation" {
            // This fixture has no Ask-mode attachment for checked tools. The
            // foreign initiation is admitted and saved, then refused by the
            // actual dispatch policy; its receipt is not a completed turn.
            assert!(matches!(
                bounded(candidate.wait()).await,
                Err(AgentError::Unsupported(_))
            ));
            assert!(backend.executions.lock().unwrap().is_empty());
            drop(hold);
        } else {
            timeout(Duration::from_secs(2), backend.dispatched.notified())
                .await
                .unwrap_or_else(|_| panic!("{variant}: manually enqueued candidate reached no provider dispatch checkpoint"));
            hold.send(()).unwrap();
            assert_eq!(
                bounded(candidate.wait()).await,
                Ok(ExecutionOutcome::Completed),
                "{variant}"
            );
        }
        let executions = backend.executions.lock().unwrap().clone();
        let turn = agent
            .owed_turns()
            .await
            .into_iter()
            .find(|turn| turn.execution_id() == &root)
            .unwrap_or_else(|| {
                panic!("{variant}: a foreign candidate cannot discharge the original work")
            });
        let frame = bounded(agent.resumption_history()).await.unwrap();
        assert_eq!(frame.successor_of(&root), None, "{variant}");
        assert!(
            matches!(
                bounded(agent.resume_turn(&turn, resumer())).await,
                Err(AgentError::SubmissionConflict)
            ),
            "{variant}"
        );
        let retry = bounded(agent.enqueue(
            original_request.clone(),
            Submitter::new(original_actor.clone(), original_initiation.clone()),
        ))
        .await
        .unwrap();
        assert_eq!(bounded(retry.wait()).await, Ok(ExecutionOutcome::Cancelled), "{variant}: retry must answer from the suspended original, not the foreign candidate receipt");
        assert_eq!(
            *backend.executions.lock().unwrap(),
            executions,
            "{variant}: retry cannot dispatch extra work"
        );
        bounded(agent.close(actor())).await.unwrap();
    }
}

#[tokio::test]
async fn an_ordinary_suspension_cannot_name_a_different_local_stop_actor() {
    let mut snapshot = stopped_storage(Stop::Suspend).await.snapshot();
    let record = &mut snapshot.invocations[0];
    assert_eq!(
        record.local_cancellation.as_ref().unwrap().actor,
        Some(resumer())
    );
    let suspension = record
        .scheduling
        .iter_mut()
        .find(|edge| edge.cause == SchedulingCause::Suspended)
        .unwrap();
    assert_eq!(suspension.actor, Some(resumer()));
    assert_ne!(actor(), resumer());
    suspension.actor = Some(actor());
    let storage = MemoryStorage::default();
    storage.0.lock().unwrap().snapshot = Some(snapshot);
    let manager = SessionManager::open(
        Some(SessionId::new("conversation").unwrap()),
        Arc::new(storage),
        Arc::new(nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new()),
    )
    .await
    .unwrap();
    let backend = workflow_backend();
    let restored = attached_agent(Arc::new(WorkflowProvider(backend.clone())), manager).await;
    assert!(matches!(
        restored,
        Err(AgentError::Storage(StorageError::Corrupt(_)))
    ));
    assert!(backend.executions.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_decline_keeps_its_final_save_after_its_caller_is_lost() {
    let saved = stopped_storage(Stop::Suspend).await;
    let (agent, backend, storage, _hold) = restore_holding(saved).await;
    let turn = agent.owed_turns().await.remove(0);
    let (saving, release) = storage.pause_next_save();
    let declining = {
        let agent = agent.clone();
        let turn = turn.clone();
        tokio::spawn(async move { agent.decline_turn(&turn, actor()).await })
    };
    bounded(saving).await.unwrap();
    declining.abort();
    assert!(bounded(declining).await.unwrap_err().is_cancelled());
    release
        .send(())
        .expect("the SDK retains the final decision after caller loss");
    bounded(async {
        while resumptions(&backend).is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(agent.owed_turns().await.is_empty());
    assert_eq!(
        storage.snapshot().invocations[0]
            .scheduling
            .last()
            .unwrap()
            .cause,
        SchedulingCause::NotResumed
    );
    bounded(agent.decline_turn(&turn, actor())).await.unwrap();
    assert_eq!(resumptions(&backend), [TurnResumption::Declined]);
    assert!(backend.executions.lock().unwrap().is_empty());
    bounded(agent.close(actor())).await.unwrap();
}

/// Capture the same second turn first queued and then running, using actual
/// admissions. Give the two valid process histories distinct session owners.
async fn differently_dispatched_session_histories() -> (SessionSnapshot, SessionSnapshot) {
    let (agent, backend, storage) = workflow().await;
    let (release_ahead, ahead_gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(ahead_gate);
    let ahead = Mode::Queued.start(agent.clone(), request("ahead"));
    bounded(backend.dispatched.notified()).await;
    let shared = bounded(agent.enqueue(request("shared"), Submitter::owner(actor())))
        .await
        .unwrap();
    let mut queued = storage.snapshot();
    let (_release_shared, shared_gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(shared_gate);
    release_ahead.send(()).unwrap();
    assert_eq!(
        bounded(ahead).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    bounded(backend.dispatched.notified()).await;
    let mut running = storage.snapshot();
    bounded(agent.close(actor())).await.unwrap();
    let _ = bounded(shared.wait()).await;
    queued.id = SessionId::new("queued-session").unwrap();
    running.id = SessionId::new("running-session").unwrap();
    (queued, running)
}

async fn open_named_resumption_history(
    snapshot: SessionSnapshot,
) -> (Agent, Arc<WorkflowBackend>, MemoryStorage) {
    let storage = MemoryStorage::default();
    let id = snapshot.id.clone();
    storage.0.lock().unwrap().snapshot = Some(snapshot);
    open_named_resumption_storage(storage, id).await
}

async fn open_named_resumption_storage(
    storage: MemoryStorage,
    id: SessionId,
) -> (Agent, Arc<WorkflowBackend>, MemoryStorage) {
    let manager = SessionManager::open(
        Some(id),
        Arc::new(storage.clone()),
        Arc::new(nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new()),
    )
    .await
    .unwrap();
    let backend = workflow_backend();
    let agent = Agent::prepare(
        Arc::new(WorkflowProvider(backend.clone())),
        manager,
        backend.audit.clone(),
    )
    .await
    .unwrap();
    reattach_after_explicit_close(&agent).await;
    (agent, backend, storage)
}

#[tokio::test]
async fn a_foreign_session_owed_turn_cannot_resume_or_decline_a_matching_local_turn() {
    let (queued, running) = differently_dispatched_session_histories().await;
    for resume in [false, true] {
        for reverse in [false, true] {
            let (source, target) = if reverse {
                (queued.clone(), running.clone())
            } else {
                (running.clone(), queued.clone())
            };
            let (source_agent, _, _) = open_named_resumption_history(source).await;
            let token = source_agent
                .owed_turns()
                .await
                .into_iter()
                .find(|turn| turn.execution_id().as_str() == "shared")
                .unwrap();
            let (target_agent, backend, storage) = open_named_resumption_history(target).await;
            let local = target_agent
                .owed_turns()
                .await
                .into_iter()
                .find(|turn| turn.execution_id().as_str() == "shared")
                .unwrap();
            assert_ne!(token.dispatched(), local.dispatched());
            let before = storage.snapshot();
            let outcome = if resume {
                matches!(
                    bounded(target_agent.resume_turn(&token, resumer())).await,
                    Ok(Resumed::NotOwed)
                )
            } else {
                bounded(target_agent.decline_turn(&token, resumer())).await == Ok(())
            };
            let unchanged = storage.snapshot() == before;
            let decision_audits = backend
                .audit
                .records
                .lock()
                .unwrap()
                .iter()
                .filter_map(|record| match record {
                    ExecutionAuditRecord::TurnResumed(record) => Some(record.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let no_audit = decision_audits.is_empty();
            // Join physical owners before the regression assertions, including
            // a mistakenly admitted successor on the unfixed implementation.
            bounded(target_agent.close(actor())).await.unwrap();
            bounded(source_agent.close(actor())).await.unwrap();
            let no_dispatch = backend.executions.lock().unwrap().is_empty();
            assert!(outcome, "resume={resume} reverse={reverse}");
            assert!(
                unchanged,
                "foreign token changed local history: {decision_audits:?}"
            );
            assert!(
                no_audit,
                "foreign token generated a local decision audit: {decision_audits:?}"
            );
            assert!(no_dispatch, "foreign token dispatched local work");
        }
    }
}

#[tokio::test]
async fn an_owed_turn_token_remains_usable_after_its_own_session_is_reopened() {
    let (snapshot, _) = differently_dispatched_session_histories().await;
    let id = snapshot.id.clone();
    let (original, original_backend, storage) = open_named_resumption_history(snapshot).await;
    let token = original
        .owed_turns()
        .await
        .into_iter()
        .find(|turn| turn.execution_id().as_str() == "shared")
        .unwrap();
    bounded(original.suspend(resumer())).await.unwrap();
    drop(original);
    drop(original_backend);
    // Reacquire the same storage lease after suspension and drop. No snapshot
    // is copied or replaced between the two owners of this persistent session.
    let (reopened, backend, _) = open_named_resumption_storage(storage, id).await;
    let result = bounded(reopened.resume_turn(&token, resumer()))
        .await
        .unwrap();
    let Resumed::Continued(admission) = result else {
        panic!("a token belongs to the persistent session, not its Agent instance");
    };
    assert_eq!(
        bounded(admission.wait()).await,
        Ok(ExecutionOutcome::Completed)
    );
    let audits = resumptions(&backend);
    bounded(reopened.close(actor())).await.unwrap();
    assert_eq!(audits.len(), 1);
    assert!(matches!(audits[0], TurnResumption::Continued { .. }));
}

#[tokio::test]
async fn a_resumption_audit_uses_the_selected_sessions_current_dispatch_evidence() {
    let (queued, mut running) = differently_dispatched_session_histories().await;
    running.id = queued.id.clone();
    // Independent valid restored frames model a stale token from this same
    // persistent session. These are separate test stores, not concurrent writers.
    for resume in [false, true] {
        for reverse in [false, true] {
            let (source, target) = if reverse {
                (queued.clone(), running.clone())
            } else {
                (running.clone(), queued.clone())
            };
            let (source_agent, _, _) = open_named_resumption_history(source).await;
            let token = source_agent
                .owed_turns()
                .await
                .into_iter()
                .find(|turn| turn.execution_id().as_str() == "shared")
                .unwrap();
            let (target_agent, backend, storage) = open_named_resumption_history(target).await;
            let local = target_agent
                .owed_turns()
                .await
                .into_iter()
                .find(|turn| turn.execution_id().as_str() == "shared")
                .unwrap();
            assert_ne!(token.dispatched(), local.dispatched());
            let mut notice_matches = true;
            if resume {
                let result = bounded(target_agent.resume_turn(&token, resumer()))
                    .await
                    .unwrap();
                let Resumed::Continued(admission) = result else {
                    panic!("same-session stale hints still select currently owed work");
                };
                let text = successor_text(&storage, admission.id());
                notice_matches = text.starts_with(if local.dispatched() {
                    "[Nessa restarted while you were working"
                } else {
                    "[Nessa restarted before this message reached you"
                });
                assert_eq!(
                    bounded(admission.wait()).await,
                    Ok(ExecutionOutcome::Completed)
                );
            } else {
                bounded(target_agent.decline_turn(&token, resumer()))
                    .await
                    .unwrap();
            }
            let audits = backend
                .audit
                .records
                .lock()
                .unwrap()
                .iter()
                .filter_map(|record| match record {
                    ExecutionAuditRecord::TurnResumed(record) => Some(record.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            bounded(target_agent.close(actor())).await.unwrap();
            bounded(source_agent.close(actor())).await.unwrap();
            assert!(notice_matches);
            assert_eq!(audits.len(), 1);
            let audit = &audits[0];
            assert_eq!(audit.session_id(), &storage.snapshot().id);
            assert_eq!(audit.execution_id(), local.execution_id());
            assert_eq!(audit.cause(), SchedulingCause::Interrupted);
            assert_eq!(audit.submitted_by(), local.submitted_by());
            assert_eq!(audit.resumed_by(), &resumer());
            assert_eq!(
                audit.dispatched(),
                local.dispatched(),
                "resume={resume} reverse={reverse}"
            );
        }
    }
}

mod result_prefixes;
