//! Custom native steering follows the acknowledged response/turn ownership boundary.
use super::*;

async fn active_switch(
    timing: ModelChangeTiming,
    widening: bool,
    outcome: SteeringOutcome,
) -> (
    Agent,
    Arc<Provider>,
    MemoryStorage,
    QueueAdmission,
    oneshot::Sender<()>,
) {
    let (release, hold) = oneshot::channel();
    let maximum = if widening { 200 } else { 1000 };
    let (agent, prior, storage) = prepared_with_capabilities(
        Some(hold),
        Normalization::Exact,
        None,
        capabilities_for("large", maximum),
    )
    .await;
    *prior.backend.model_timing.lock().unwrap() = timing;
    *prior.backend.steering.lock().unwrap() = Some(outcome);
    let active = agent
        .enqueue(input("native-owner", 50), Submitter::owner(close_action()))
        .await
        .unwrap();
    timeout(Duration::from_secs(3), prior.backend.started.notified())
        .await
        .unwrap();
    let chosen = Arc::new(Provider {
        model: "small".into(),
        capabilities: capabilities_for("small", if widening { 1000 } else { 200 }),
        ..(*prior).clone()
    });
    let effect = agent
        .set_model(chosen.clone(), close_action())
        .await
        .unwrap();
    assert_eq!(effect.timing(), timing);
    assert_eq!(agent.provider_identity(), chosen.identity());
    (agent, prior, storage, active, release)
}

async fn native_binding(timing: ModelChangeTiming, widening: bool) {
    let (agent, prior, storage, active, release) =
        active_switch(timing, widening, SteeringOutcome::Injected).await;
    let request = input("native-correction", 400);
    let submitter = Submitter::owner(close_action());
    let writes_before = storage.0.lock().unwrap().writes;
    let result = agent.steer(request.clone(), submitter.clone()).await;
    let expected_injection = match timing {
        ModelChangeTiming::NextTurn => !widening,
        ModelChangeTiming::SubsequentResponses => widening,
    };
    let calls = prior.backend.steered.lock().unwrap().clone();
    if expected_injection && matches!(result, Ok(SteeringDelivery::Injected { .. })) {
        let writes = storage.0.lock().unwrap().writes;
        assert!(matches!(
            agent.steer(request.clone(), submitter.clone()).await,
            Ok(SteeringDelivery::Injected { .. })
        ));
        let mut changed = request;
        changed.reserved_output_tokens += 1;
        assert_eq!(
            agent.steer(changed, submitter).await.err(),
            Some(AgentError::SubmissionConflict)
        );
        assert_eq!(storage.0.lock().unwrap().writes, writes);
        assert_eq!(prior.backend.steered.lock().unwrap().len(), 1);
    }
    let writes_after = storage.0.lock().unwrap().writes;
    release.send(()).unwrap();
    assert_eq!(active.wait().await, Ok(ExecutionOutcome::Completed));
    agent.close(close_action()).await.unwrap();
    if expected_injection {
        assert!(
            matches!(result, Ok(SteeringDelivery::Injected { .. })),
            "valid native input was refused: {:?}",
            result.err()
        );
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, ExecutionId::new("native-owner").unwrap());
    } else {
        assert!(
            matches!(result, Err(AgentError::InvalidInput(_))),
            "invalid native input was admitted"
        );
        assert!(calls.is_empty());
        assert_eq!(writes_after, writes_before);
    }
}

#[tokio::test]
async fn native_next_turn_widening_keeps_active_limits() {
    native_binding(ModelChangeTiming::NextTurn, true).await;
}
#[tokio::test]
async fn native_next_turn_narrowing_keeps_active_limits() {
    native_binding(ModelChangeTiming::NextTurn, false).await;
}
#[tokio::test]
async fn native_subsequent_responses_widening_uses_acknowledged_limits() {
    native_binding(ModelChangeTiming::SubsequentResponses, true).await;
}
#[tokio::test]
async fn native_subsequent_responses_narrowing_uses_acknowledged_limits() {
    native_binding(ModelChangeTiming::SubsequentResponses, false).await;
}

#[tokio::test]
async fn native_prompt_required_revalidates_current_queue_binding() {
    let (agent, prior, _, active, release) = active_switch(
        ModelChangeTiming::NextTurn,
        false,
        SteeringOutcome::PromptRequired,
    )
    .await;
    let result = agent
        .steer(
            input("queue-fallback", 400),
            Submitter::owner(close_action()),
        )
        .await;
    let calls = prior.backend.steered.lock().unwrap().len();
    let queued = agent.queued_ids().await;
    release.send(()).unwrap();
    assert_eq!(active.wait().await, Ok(ExecutionOutcome::Completed));
    agent.close(close_action()).await.unwrap();
    assert!(matches!(result, Err(AgentError::InvalidInput(_))));
    assert_eq!(
        calls, 1,
        "valid A-native input must reach explicit nonconsumption before B queue validation"
    );
    assert!(queued.is_empty());
}

#[tokio::test]
async fn native_foreign_initiator_queues_under_current_binding() {
    let (agent, prior, _, active, release) =
        active_switch(ModelChangeTiming::NextTurn, true, SteeringOutcome::Injected).await;
    let foreign = Submitter::new(
        close_action(),
        Initiation::checked_grant(GrantInitiator::new("other-owner", 1).unwrap()),
    );
    let result = agent.steer(input("foreign-correction", 400), foreign).await;
    assert!(prior.backend.steered.lock().unwrap().is_empty());
    release.send(()).unwrap();
    assert_eq!(active.wait().await, Ok(ExecutionOutcome::Completed));
    let receipt = match result {
        Ok(SteeringDelivery::Queued(receipt)) => receipt,
        _ => panic!("foreign input must use its own B queue binding"),
    };
    assert_eq!(receipt.wait().await, Ok(ExecutionOutcome::Completed));
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn native_idle_queues_under_current_binding() {
    let (agent, prior, _, active, release) =
        active_switch(ModelChangeTiming::NextTurn, true, SteeringOutcome::Injected).await;
    release.send(()).unwrap();
    assert_eq!(active.wait().await, Ok(ExecutionOutcome::Completed));
    let result = agent
        .steer(
            input("idle-correction", 400),
            Submitter::owner(close_action()),
        )
        .await;
    assert!(prior.backend.steered.lock().unwrap().is_empty());
    let receipt = match result {
        Ok(SteeringDelivery::Queued(receipt)) => receipt,
        _ => panic!("idle input must use B queue binding"),
    };
    assert_eq!(receipt.wait().await, Ok(ExecutionOutcome::Completed));
    agent.close(close_action()).await.unwrap();
}

struct FinishTarget {
    agent: Agent,
    release: Mutex<Option<oneshot::Sender<()>>>,
    ran: AtomicUsize,
}
impl AfterSaveCheck for FinishTarget {
    fn after_save<'a>(
        &'a self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), AfterSaveRefusal>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.ran.fetch_add(1, Ordering::SeqCst);
            self.release
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(())
                .unwrap();
            timeout(Duration::from_secs(3), async {
                while self.agent.active_execution_id().is_some() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            Ok(())
        })
    }
}

#[tokio::test]
async fn native_finished_target_revalidates_current_queue_binding() {
    let (agent, prior, _, active, release) = active_switch(
        ModelChangeTiming::NextTurn,
        false,
        SteeringOutcome::Injected,
    )
    .await;
    let check = Arc::new(FinishTarget {
        agent: agent.clone(),
        release: Mutex::new(Some(release)),
        ran: AtomicUsize::new(0),
    });
    let result = agent
        .steer(
            input("finished-target", 400),
            Submitter::owner(close_action()).with_after_save(check.clone()),
        )
        .await;
    // Release the original even when the before implementation refuses input
    // before reaching the saved hook; cleanup cannot conceal that ordering.
    if let Some(release) = check.release.lock().unwrap().take() {
        release.send(()).unwrap();
    }
    assert_eq!(active.wait().await, Ok(ExecutionOutcome::Completed));
    let queued = agent.queued_ids().await;
    let calls = prior.backend.steered.lock().unwrap().len();
    agent.close(close_action()).await.unwrap();
    assert!(matches!(result, Err(AgentError::InvalidInput(_))));
    assert_eq!(
        check.ran.load(Ordering::SeqCst),
        1,
        "valid A input must be saved before its target finishes"
    );
    assert_eq!(calls, 0);
    assert!(queued.is_empty());
}

async fn fallback_valid_for_current_binding(target_finishes: bool) {
    let (agent, prior, _, active, release) = active_switch(
        ModelChangeTiming::NextTurn,
        false,
        SteeringOutcome::PromptRequired,
    )
    .await;
    let check = Arc::new(FinishTarget {
        agent: agent.clone(),
        release: Mutex::new(Some(release)),
        ran: AtomicUsize::new(0),
    });
    let request = input("valid-queue-fallback", 100);
    let submitter = Submitter::owner(close_action());
    let submitter = if target_finishes {
        submitter.with_after_save(check.clone())
    } else {
        submitter
    };
    let result = agent.steer(request.clone(), submitter.clone()).await;
    if let Some(release) = check.release.lock().unwrap().take() {
        release.send(()).unwrap();
    }
    assert_eq!(active.wait().await, Ok(ExecutionOutcome::Completed));
    let receipt = match result {
        Ok(SteeringDelivery::Queued(receipt)) => receipt,
        _ => panic!("valid B input must retain its original queued receipt"),
    };
    assert_eq!(receipt.wait().await, Ok(ExecutionOutcome::Completed));
    assert_eq!(
        prior.backend.steered.lock().unwrap().len(),
        usize::from(!target_finishes)
    );
    assert_eq!(prior.backend.inner.prompts.load(Ordering::SeqCst), 2);
    let snapshot = agent.session_manager().snapshot().await.unwrap();
    assert_eq!(
        snapshot
            .invocations
            .iter()
            .find(|record| record.request.execution_id == request.execution_id)
            .unwrap()
            .request,
        request
    );
    let expected_model = agent.provider_identity();
    {
        let audit = prior.backend.audit.lock().unwrap();
        let admission = audit
            .iter()
            .find_map(|record| match record {
                ExecutionAuditRecord::QueueAdmitted(record)
                    if record.execution_id() == &request.execution_id =>
                {
                    Some(record)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(admission.model(), Some(&expected_model));
    }
    agent.close(close_action()).await.unwrap();
}
#[tokio::test]
async fn native_prompt_required_valid_current_binding_queues_once() {
    fallback_valid_for_current_binding(false).await;
}
#[tokio::test]
async fn native_finished_target_valid_current_binding_queues_once() {
    fallback_valid_for_current_binding(true).await;
}
