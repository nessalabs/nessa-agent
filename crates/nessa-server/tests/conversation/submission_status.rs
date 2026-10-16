//! Read-only submit answers and Stop classify the same committed turn status.
use super::*;

#[test]
fn submission_disposition_preserves_results_and_each_scheduling_stage() {
    for (stage, without_result) in [
        (None, ConversationDisposition::Queued),
        (
            Some(InvocationStage::Queued),
            ConversationDisposition::Queued,
        ),
        (
            Some(InvocationStage::Running),
            ConversationDisposition::Queued,
        ),
        (
            Some(InvocationStage::Injected),
            ConversationDisposition::Injected,
        ),
        (
            Some(InvocationStage::Settled),
            ConversationDisposition::Settled,
        ),
        (
            Some(InvocationStage::Cancelled),
            ConversationDisposition::Settled,
        ),
    ] {
        assert_eq!(submission_disposition(false, stage), without_result);
        assert_eq!(
            submission_disposition(true, stage),
            ConversationDisposition::Settled,
            "a committed result settles the submission before its scheduling edge: {stage:?}"
        );
    }
}

#[tokio::test]
async fn current_submission_rechecks_policy_before_returning_committed_status() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let policy = Arc::new(ObservedPolicy::default());
    let (service, storage, metadata) = fixture_admitted_by(
        &root,
        provider.clone(),
        crate::conversation::infrastructure::in_process_environment().into(),
        policy.clone(),
    );
    let target = id();
    opened(&service, &provider, &target).await;
    finish_turn(&service, &provider, &target, "done").await;
    assert_eq!(
        service
            .current_submission(target.clone(), caller("read"), "done".into())
            .await
            .unwrap()
            .disposition,
        ConversationDisposition::Settled
    );
    let asks = policy.asked();
    let rows = command_rows(&root);
    policy.undecide();
    assert!(matches!(
        service
            .current_submission(target, caller("read"), "done".into())
            .await,
        Err(ConversationError::Metadata)
    ));
    assert!(policy.asked() > asks);
    assert_eq!(command_rows(&root), rows);
    assert_eq!(provider.executions.lock().unwrap().as_slice(), ["done"]);
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn current_submission_and_stop_agree_on_running_queued_cancelled_and_finished_turns() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    assert!(matches!(
        service
            .current_submission(target.clone(), caller("read"), "missing".into())
            .await,
        Err(ConversationError::NotFound)
    ));
    assert!(matches!(
        service
            .current_submission(target.clone(), caller("read"), String::new())
            .await,
        Err(ConversationError::InvalidInput)
    ));
    let release_execution = hold_turn(&service, &provider, &target, "active").await;
    let active = ExecutionId::new("active").unwrap();
    let live = service.resolve(&target, &caller("read")).await.unwrap();
    let status = live
        .agent
        .session_manager()
        .committed_invocation_status(&active)
        .await
        .unwrap();
    assert_eq!(status.stage(), Some(InvocationStage::Running));
    assert!(!status.has_result());
    assert_eq!(turn_is_final(&live, &active).await, Some(false));
    assert_eq!(
        service
            .current_submission(target.clone(), caller("read"), "active".into())
            .await
            .unwrap()
            .disposition,
        ConversationDisposition::Queued
    );
    service
        .submit_command(
            storage.clone(),
            target.clone(),
            caller("submit-queued"),
            Initiation::owner(),
            true,
            "queued".into(),
            message("later"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .current_submission(target.clone(), caller("read"), "queued".into())
            .await
            .unwrap()
            .disposition,
        ConversationDisposition::Queued
    );
    let queued = ExecutionId::new("queued").unwrap();
    assert_eq!(turn_is_final(&live, &queued).await, Some(false));
    let withdrawn = service
        .stop_command(
            storage.clone(),
            target.clone(),
            caller("withdraw-queued"),
            "queued".into(),
        )
        .await
        .unwrap();
    assert_eq!(withdrawn.outcome(), Some(MutationOutcome::Withdrawn));
    assert_eq!(turn_is_final(&live, &queued).await, Some(true));
    assert_eq!(
        service
            .current_submission(target.clone(), caller("read"), "queued".into())
            .await
            .unwrap()
            .disposition,
        ConversationDisposition::Settled
    );
    release_execution.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while turn_is_final(&live, &active).await != Some(true) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        service
            .current_submission(target.clone(), caller("read"), "active".into())
            .await
            .unwrap()
            .disposition,
        ConversationDisposition::Settled
    );
    let final_stop = service
        .stop_command(
            storage.clone(),
            target.clone(),
            caller("stop-finished"),
            "active".into(),
        )
        .await
        .unwrap();
    assert_eq!(final_stop.outcome(), Some(MutationOutcome::AlreadyFinal));
    assert_eq!(provider.executions.lock().unwrap().as_slice(), ["active"]);
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        turn_is_final(&live, &ExecutionId::new("missing").unwrap()).await,
        None
    );
    drop(live);
    tombstone(&metadata, &target).await;
    assert!(matches!(
        service
            .current_submission(target, caller("read"), "active".into())
            .await,
        Err(ConversationError::Deleted)
    ));
    retire(service, storage, metadata).await;
}
