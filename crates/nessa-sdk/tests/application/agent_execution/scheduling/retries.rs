//! Stable submission identities recover receipts without repeating provider effects.
use super::*;
use nessa_sdk::domain::common::value_objects::{ImageMediaType, Sha256Digest};

#[tokio::test]
async fn scheduling_concurrent_retries_share_dispatch_and_settlement() {
    let (agent, storage, _, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let agent = agent.clone();
        tasks.push(tokio::spawn(async move {
            agent.enqueue(request("same"), actor()).await.unwrap()
        }));
    }
    let mut receipts = Vec::new();
    for task in tasks {
        receipts.push(within(task).await.unwrap());
    }
    let running = started(&mut calls, "same").await;
    assert_eq!(storage.snapshot().invocations.len(), 1);
    complete(running);
    for receipt in receipts {
        assert_eq!(
            within(receipt.wait()).await,
            Ok(ExecutionOutcome::Completed)
        );
    }
    let retry = agent.enqueue(request("same"), actor()).await.unwrap();
    assert_eq!(within(retry.wait()).await, Ok(ExecutionOutcome::Completed));
    assert!(calls.try_recv().is_err());
    assert_eq!(record(&storage, "same").scheduling.len(), 3);
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn scheduling_cancelled_admission_wait_is_supervised_and_retry_joins_it() {
    let (agent, storage, _, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let (saving, release) = storage.pause_next_save();
    let caller = agent.clone();
    let task = tokio::spawn(async move { caller.enqueue(request("same"), actor()).await });
    within(saving).await.unwrap();
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    let retrying = agent.clone();
    let retry = tokio::spawn(async move { retrying.enqueue(request("same"), actor()).await });
    release.send(()).unwrap();
    let receipt = within(retry).await.unwrap().unwrap();
    complete(started(&mut calls, "same").await);
    assert_eq!(
        within(receipt.wait()).await,
        Ok(ExecutionOutcome::Completed)
    );
    assert_eq!(storage.snapshot().invocations.len(), 1);
    assert!(calls.try_recv().is_err());
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn scheduling_retry_rejects_changed_content_attribution_or_delivery_mode() {
    let (agent, storage, _, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let first = agent.enqueue(request("same"), actor()).await.unwrap();
    let running = started(&mut calls, "same").await;
    let mut different = request("same");
    different.user_message = UserMessage::text_only(PromptText::new("different message").unwrap());
    assert!(matches!(
        agent.enqueue(different, actor()).await,
        Err(AgentError::SubmissionConflict)
    ));
    // The same words with an image attached are a different message.
    let mut different = request("same");
    different.user_message = UserMessage::new(
        different.user_message.text().cloned(),
        vec![
            ImageReference::new(Sha256Digest::from_bytes([1; 32]), ImageMediaType::Png, 1).unwrap(),
        ],
        Vec::new(),
    )
    .unwrap();
    assert!(matches!(
        agent.enqueue(different, actor()).await,
        Err(AgentError::ImageInputRefused(ImageInputRefusal::NotOffered))
    ));
    let mut different = request("same");
    different.estimated_input_tokens += 1;
    assert!(matches!(
        agent.enqueue(different, actor()).await,
        Err(AgentError::SubmissionConflict)
    ));
    assert!(matches!(
        agent.enqueue(request("same"), close_action()).await,
        Err(AgentError::SubmissionConflict)
    ));
    assert!(matches!(
        agent.enqueue_steering(request("same"), actor()).await,
        Err(AgentError::SubmissionConflict)
    ));
    assert!(matches!(
        agent.steer(request("same"), actor()).await,
        Err(AgentError::SubmissionConflict)
    ));
    assert_eq!(storage.snapshot().invocations.len(), 1);
    complete(running);
    within(first.wait()).await.unwrap();
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn scheduling_native_retry_recovers_injection_and_ambiguous_error_without_redelivery() {
    for outcome in [Ok(SteeringOutcome::Injected), Err(AgentError::Deadline)] {
        let (agent, storage, provider, mut calls) = fixture(outcome.clone()).await;
        let active = agent.enqueue(request("active"), actor()).await.unwrap();
        let running = started(&mut calls, "active").await;
        for _ in 0..2 {
            match agent.steer(request("correction"), actor()).await {
                Ok(SteeringDelivery::Injected { target, .. }) => {
                    assert!(outcome.is_ok());
                    assert_eq!(target.as_str(), "active");
                }
                Err(error) => assert_eq!(Err(error), outcome),
                _ => panic!("native input must not be queued again"),
            }
        }
        assert_eq!(provider.steered.lock().unwrap().len(), 1);
        assert_eq!(storage.snapshot().invocations.len(), 2);
        complete(running);
        within(active.wait()).await.unwrap();
        agent.close(close_action()).await.unwrap();
    }
}

#[tokio::test]
async fn scheduling_restore_returns_saved_result_without_dispatching_again() {
    let (agent, storage, provider, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let receipt = agent.enqueue(request("saved"), actor()).await.unwrap();
    complete(started(&mut calls, "saved").await);
    within(receipt.wait()).await.unwrap();
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let restored = attached_agent(Arc::new(GatedFactory(provider)), storage.manager().await)
        .await
        .unwrap();
    let receipt = restored.enqueue(request("saved"), actor()).await.unwrap();
    assert_eq!(
        within(receipt.wait()).await,
        Ok(ExecutionOutcome::Completed)
    );
    assert!(calls.try_recv().is_err());
    assert_eq!(storage.snapshot().invocations.len(), 1);
    restored.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn restored_retry_retains_late_queue_and_native_persistence_failures() {
    for native in [false, true] {
        let (agent, storage, provider, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
        let receipt = agent.enqueue(request("active"), actor()).await.unwrap();
        let running = started(&mut calls, "active").await;
        let (native_evidence, queue_error) = if native {
            storage.fail_scheduling("correction", InvocationStage::Injected);
            let evidence = match agent.steer(request("correction"), actor()).await.unwrap() {
                SteeringDelivery::Injected { evidence, .. } => evidence,
                SteeringDelivery::Queued(_) => panic!("active steering must remain native"),
            };
            assert!(matches!(
                &evidence,
                SteeringEvidence::Failed(failure)
                    if matches!(failure.storage(), Some(StorageError::Io(_)))
            ));
            let _ = running.release.send(Ok(ExecutionOutcome::Completed));
            let active_result = within(receipt.wait()).await;
            assert_eq!(record(&storage, "active").result, Some(active_result));
            (Some(evidence), None)
        } else {
            storage.fail_scheduling("active", InvocationStage::Settled);
            complete(running);
            (None, Some(within(receipt.wait()).await.unwrap_err()))
        };
        agent.close(close_action()).await.unwrap();
        drop(agent);
        let restored = attached_agent(
            Arc::new(GatedFactory(provider.clone())),
            storage.manager().await,
        )
        .await
        .unwrap();
        if native {
            let restored_evidence = match restored.steer(request("correction"), actor()).await {
                Ok(SteeringDelivery::Injected { evidence, .. }) => evidence,
                Ok(SteeringDelivery::Queued(_)) => {
                    panic!("restored native receipt became queued")
                }
                Err(error) => panic!("restored native receipt failed: {error:?}"),
            };
            assert_eq!(Some(restored_evidence), native_evidence);
            assert_eq!(provider.steered.lock().unwrap().len(), 1);
        } else {
            let retry = restored.enqueue(request("active"), actor()).await.unwrap();
            assert_eq!(within(retry.wait()).await, Err(queue_error.unwrap()));
        }
        assert!(calls.try_recv().is_err());
        restored.close(close_action()).await.unwrap();
    }
}

#[tokio::test]
async fn restored_withdrawn_receipt_retains_its_audit_failure() {
    let (agent, storage, provider, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let active = agent.enqueue(request("active"), actor()).await.unwrap();
    let running = started(&mut calls, "active").await;
    let pending = agent.enqueue(request("removed"), actor()).await.unwrap();
    storage.fail_scheduling("removed", InvocationStage::Cancelled);
    assert!(agent
        .remove_queued(request("removed").execution_id, actor())
        .await
        .is_err());
    let original = within(pending.wait()).await.unwrap_err();
    complete(running);
    within(active.wait()).await.unwrap();
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let restored = attached_agent(Arc::new(GatedFactory(provider)), storage.manager().await)
        .await
        .unwrap();
    let retry = restored.enqueue(request("removed"), actor()).await.unwrap();
    assert_eq!(within(retry.wait()).await, Err(original));
    assert!(calls.try_recv().is_err());
    restored.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn scheduling_idle_steering_retry_preserves_intent_without_native_injection() {
    let (agent, storage, provider, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let SteeringDelivery::Queued(first) = agent.steer(request("idle"), actor()).await.unwrap()
    else {
        panic!("idle steering must queue without native injection")
    };
    let running = started(&mut calls, "idle").await;
    let SteeringDelivery::Queued(retry) = agent.steer(request("idle"), actor()).await.unwrap()
    else {
        panic!("retry must recover the queued receipt")
    };
    assert!(provider.steered.lock().unwrap().is_empty());
    complete(running);
    assert_eq!(within(first.wait()).await, Ok(ExecutionOutcome::Completed));
    assert_eq!(within(retry.wait()).await, Ok(ExecutionOutcome::Completed));
    let SteeringDelivery::Queued(settled_retry) =
        agent.steer(request("idle"), actor()).await.unwrap()
    else {
        panic!("settled retry must recover the original delivery")
    };
    assert_eq!(
        within(settled_retry.wait()).await,
        Ok(ExecutionOutcome::Completed)
    );
    let saved = record(&storage, "idle");
    assert_eq!(saved.submission, SubmissionMode::Steering);
    assert_eq!(saved.scheduling.len(), 3);
    assert!(saved.scheduling.iter().all(|edge| edge.target.is_none()));
    assert_eq!(
        saved.scheduling.last().unwrap().stage,
        InvocationStage::Settled
    );
    assert_eq!(storage.snapshot().invocations.len(), 1);
    assert!(provider.steered.lock().unwrap().is_empty());
    assert!(calls.try_recv().is_err());
    assert!(matches!(
        agent.enqueue_steering(request("idle"), actor()).await,
        Err(AgentError::SubmissionConflict)
    ));
    agent.close(close_action()).await.unwrap();
}
