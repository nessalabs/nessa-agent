//! A turn cut off by a restart, or stopped by the host's own shutdown, is
//! continued by a successor turn; a person's stop stays final.
use super::*;
use nessa_sdk::application::agent_execution::{
    agents::{
        owed_turn_ids, resumed_turn_id, AttachmentFailureCode, AttachmentPhase, OwedTurn, Resumed,
    },
    executions::TurnResumption,
};
use nessa_sdk::domain::agent_execution::executions::Resumption;

mod canonical_admission;

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
async fn a_decline_audits_its_applied_decision_despite_a_failed_terminal_save() {
    for reject_audit in [false, true] {
        let saved = cut_off_while_running("decline-save").await;
        let (agent, backend, storage, _hold) = restore_holding(saved).await;
        let turn = agent.owed_turns().await.remove(0);
        // Interrupted commits; the final NotResumed decision is retained but
        // its adapter rejects the write without committing it.
        let failed_write = storage.writes() + 2;
        storage.0.lock().unwrap().fail_write = Some(failed_write);
        backend.audit.reject.store(reject_audit, Ordering::SeqCst);
        let error = bounded(agent.decline_turn(&turn, resumer()))
            .await
            .unwrap_err();
        let storage_error = AgentError::Storage(StorageError::Io("fixture write failed".into()));
        assert_eq!(
            error,
            if reject_audit {
                AgentError::MultipleOperationFailures {
                    first_error: Box::new(storage_error),
                    subsequent_error: Box::new(AgentError::AuditFailure),
                }
            } else {
                storage_error
            }
        );
        assert_eq!(resumptions(&backend), [TurnResumption::Declined]);
        assert_eq!(
            storage.snapshot().invocations[0]
                .scheduling
                .last()
                .unwrap()
                .cause,
            SchedulingCause::Interrupted
        );
        assert!(
            agent.owed_turns().await.is_empty(),
            "the live decision is retained"
        );
        agent
            .session_manager()
            .save_retained_lease_records()
            .await
            .unwrap();
        assert_eq!(
            storage.snapshot().invocations[0]
                .scheduling
                .last()
                .unwrap()
                .cause,
            SchedulingCause::NotResumed
        );
        assert!(backend.executions.lock().unwrap().is_empty());
        backend.audit.reject.store(false, Ordering::SeqCst);
        bounded(agent.close(actor())).await.unwrap();
        assert_ends_after_restore(&storage).await;
    }
}

#[tokio::test]
async fn a_failed_interrupted_save_does_not_invent_a_decline_decision() {
    let saved = cut_off_while_running("interrupted-save").await;
    let (agent, backend, storage, _hold) = restore_holding(saved).await;
    let turn = agent.owed_turns().await.remove(0);
    storage.fail_next();
    assert!(matches!(
        bounded(agent.decline_turn(&turn, resumer())).await,
        Err(AgentError::Storage(_))
    ));
    assert!(resumptions(&backend).is_empty());
    assert_eq!(agent.owed_turns().await.len(), 1);
    bounded(agent.decline_turn(&turn, resumer())).await.unwrap();
    assert_eq!(resumptions(&backend), [TurnResumption::Declined]);
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
    let (agent, backend, storage, hold) = restore_holding(saved).await;
    let turn = agent.owed_turns().await.remove(0);
    backend.audit.reject.store(true, Ordering::SeqCst);
    let Resumed::ContinuedUnaudited { admission, error } =
        bounded(agent.resume_turn(&turn, resumer())).await.unwrap()
    else {
        panic!("the successor is queued whether or not its record saved")
    };
    assert_eq!(error, AgentError::AuditFailure);
    let history = storage.snapshot().invocations;
    assert_eq!(
        resumed_turn_id(&history, turn.execution_id()),
        Some(admission.id().clone())
    );
    assert!(!owed_turn_ids(&history).contains(turn.execution_id()));
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
    failed_named(error, "failed").await
}

async fn failed_named(error: AgentError, id: &str) -> (Agent, Arc<WorkflowBackend>, MemoryStorage) {
    let (agent, backend, storage) = workflow().await;
    *backend.execution_fault.lock().unwrap() = Some(ProviderOperationFailure::new(
        error.clone(),
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    ));
    let (release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let failed = Mode::Queued.start(agent.clone(), request(id));
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
    later_person_message("next").await;
}

async fn later_person_message(next_id: &str) {
    let (agent, _backend, storage) = failed_with(AgentError::Provider {
        code: -32077,
        diagnostic: None,
    })
    .await;
    let next = agent
        .enqueue(request(next_id), Submitter::owner(actor()))
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
async fn a_message_sent_while_a_retry_waits_moves_past_the_retry_that_failed() {
    person_message_while_retry_waits("next").await;
}

async fn person_message_while_retry_waits(next_id: &str) {
    let error = AgentError::Transport("pipe".into());
    let (agent, backend, _storage) = failed_with(error.clone()).await;
    *backend.execution_fault.lock().unwrap() = Some(ProviderOperationFailure::new(
        error.clone(),
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    ));
    let (release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let mut successors = Vec::new();
    for turn in &agent.owed_turns().await {
        let Resumed::Continued(admission) =
            bounded(agent.resume_turn(turn, resumer())).await.unwrap()
        else {
            panic!("a failed turn is continued")
        };
        successors.push(admission);
    }
    bounded(backend.dispatched.notified()).await;
    // The person saw the failure and wrote again; it waits behind the retry.
    let next = agent
        .enqueue(request(next_id), Submitter::owner(actor()))
        .await
        .unwrap();
    release.send(()).unwrap();
    for admission in successors {
        let _ = bounded(admission.wait()).await;
    }
    let _ = bounded(next.wait()).await;
    *backend.execution_fault.lock().unwrap() = None;
    assert_eq!(
        owed_ids(&agent.owed_turns().await),
        [next_id],
        "the retries the person wrote past are not sent again, so the next message is not held behind them"
    );
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

#[tokio::test]
async fn continuation_read_waits_for_the_exact_original_successor_and_keeps_person_stop_final() {
    let saved = cut_off_while_running("report-continuation").await;
    let original = ExecutionId::new("report-continuation").unwrap();
    assert!(owed_turn_ids(&saved.snapshot().invocations).contains(&original));
    assert_eq!(
        resumed_turn_id(&saved.snapshot().invocations, &original),
        None
    );
    let (agent, backend, storage, _hold) = restore_holding(saved).await;
    let turn = agent.owed_turns().await.remove(0);
    let Resumed::Continued(admission) = agent.resume_turn(&turn, resumer()).await.unwrap() else {
        panic!("continued")
    };
    bounded(backend.dispatched.notified()).await;
    let snapshot = storage.snapshot();
    assert_eq!(
        resumed_turn_id(&snapshot.invocations, &original),
        Some(admission.id().clone())
    );
    let mut unrelated = snapshot.invocations.clone();
    unrelated[1].actor = ActionContext::new("other", "panel", "forged-successor").unwrap();
    assert_eq!(resumed_turn_id(&unrelated, &original), None);
    let mut unrelated = snapshot.invocations;
    let mut wrong_request = request("unrelated");
    wrong_request.execution_id = admission.id().clone();
    unrelated[1].request = wrong_request;
    assert_eq!(resumed_turn_id(&unrelated, &original), None);
    bounded(agent.close(actor())).await.unwrap();
    assert!(!owed_turn_ids(&storage.snapshot().invocations).contains(admission.id()));
    assert_eq!(
        resumed_turn_id(&storage.snapshot().invocations, admission.id()),
        None
    );
}

struct HoldBeforeDispatch {
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}
impl InvocationHook for HoldBeforeDispatch {
    fn before_invocation(&self, context: &InvocationContext<'_>) -> Result<(), HookError> {
        if context.request.execution_id.as_str() == "handoff" {
            if let Some(entered) = self.entered.lock().unwrap().take() {
                entered.send(()).unwrap();
                self.release
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
            }
        }
        Ok(())
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsettled_selection_includes_dequeued_work_before_active_provider_dispatch() {
    let (agent, _, _) = workflow().await;
    let (entered, waiting) = oneshot::channel();
    let (release, gate) = std::sync::mpsc::channel();
    agent.add_invocation_hook(Arc::new(HoldBeforeDispatch {
        entered: Mutex::new(Some(entered)),
        release: Mutex::new(Some(gate)),
    }));
    let admission = agent
        .enqueue(request("handoff"), Submitter::owner(actor()))
        .await
        .unwrap();
    bounded(waiting).await.unwrap();
    assert!(agent.active_execution_id().is_none());
    assert!(
        agent.queued_ids().await.is_empty(),
        "the runner already dequeued this work"
    );
    assert_eq!(
        agent.unsettled_execution_ids().await,
        [ExecutionId::new("handoff").unwrap()]
    );
    release.send(()).unwrap();
    bounded(admission.wait()).await.unwrap();
    assert!(agent.unsettled_execution_ids().await.is_empty());
    bounded(agent.close(actor())).await.unwrap();
}

#[tokio::test]
async fn a_saved_failed_turn_successor_is_still_followed_after_later_person_work() {
    let (agent, _backend, storage) = failed_with(AgentError::Transport("pipe".into())).await;
    let owed = agent.owed_turns().await;
    let Resumed::Continued(successor) = agent.resume_turn(&owed[0], resumer()).await.unwrap()
    else {
        panic!("failed work continues")
    };
    let successor_id = successor.id().clone();
    assert_eq!(
        bounded(successor.wait()).await,
        Ok(ExecutionOutcome::Completed)
    );
    let later = agent
        .enqueue(request("later-person-work"), Submitter::owner(actor()))
        .await
        .unwrap();
    bounded(later.wait()).await.unwrap();
    assert_eq!(
        resumed_turn_id(
            &storage.snapshot().invocations,
            &ExecutionId::new("failed").unwrap()
        ),
        Some(successor_id)
    );
    bounded(agent.close(actor())).await.unwrap();
}

#[tokio::test]
async fn historical_continuation_agrees_when_a_person_message_supersedes_a_failed_successor() {
    let error = AgentError::Transport("pipe".into());
    let (agent, backend, storage) = failed_with(error.clone()).await;
    *backend.execution_fault.lock().unwrap() = Some(ProviderOperationFailure::new(
        error,
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    ));
    let (release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let mut successors = Vec::new();
    for turn in &agent.owed_turns().await {
        let Resumed::Continued(admission) = agent.resume_turn(turn, resumer()).await.unwrap()
        else {
            panic!("owed successor")
        };
        successors.push(admission);
    }
    bounded(backend.dispatched.notified()).await;
    let next = agent
        .enqueue(request("next"), Submitter::owner(actor()))
        .await
        .unwrap();
    release.send(()).unwrap();
    let mut ids = Vec::new();
    for admission in successors {
        ids.push(admission.id().clone());
        let _ = bounded(admission.wait()).await;
    }
    let _ = bounded(next.wait()).await;
    *backend.execution_fault.lock().unwrap() = None;
    let owed = agent.owed_turns().await;
    let history = storage.snapshot().invocations;
    for id in ids {
        assert!(!owed.iter().any(|turn| turn.execution_id() == &id));
        assert!(!owed_turn_ids(&history).contains(&id));
        assert_eq!(resumed_turn_id(&history, &id), None);
    }
    bounded(agent.close(actor())).await.unwrap();
}

fn ordinary_resume_shaped_id() -> ExecutionId {
    Resumption::of(&ExecutionId::new("ordinary").unwrap())
        .successor()
        .unwrap()
}

#[tokio::test]
async fn a_failed_ordinary_resume_shaped_id_is_not_a_retry() {
    let id = ordinary_resume_shaped_id();
    let (agent, _, _) = failed_named(AgentError::Transport("pipe".into()), id.as_str()).await;
    assert_eq!(owed_ids(&agent.owed_turns().await), [id.as_str(), "behind"]);
    bounded(agent.close(actor())).await.unwrap();
}

#[tokio::test]
async fn a_completed_ordinary_resume_shaped_id_is_a_person_intervention() {
    later_person_message(ordinary_resume_shaped_id().as_str()).await;
}

#[tokio::test]
async fn an_ordinary_resume_shaped_id_moves_past_a_failed_retry() {
    person_message_while_retry_waits(ordinary_resume_shaped_id().as_str()).await;
}
