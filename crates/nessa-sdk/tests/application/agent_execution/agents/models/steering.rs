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

async fn approval_switch(
    timing: ModelChangeTiming,
    outcome: SteeringOutcome,
) -> (
    Agent,
    Arc<Provider>,
    MemoryStorage,
    QueueAdmission,
    oneshot::Sender<()>,
) {
    let (release, hold) = oneshot::channel();
    let (agent, prior, storage) = prepared(Some(hold)).await;
    *prior.backend.model_timing.lock().unwrap() = timing;
    *prior.backend.model_mode.lock().unwrap() = ApprovalMode::Full;
    *prior.backend.model_effort.lock().unwrap() = Some(EffortLevel::new("high".into()).unwrap());
    *prior.backend.steering.lock().unwrap() = Some(outcome);
    let active = agent
        .enqueue(
            input("approval-native-owner", 50),
            Submitter::owner(close_action()),
        )
        .await
        .unwrap();
    timeout(Duration::from_secs(3), prior.backend.started.notified())
        .await
        .unwrap();
    let chosen = target(&prior);
    agent.set_model(chosen, close_action()).await.unwrap();
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Full));
    (agent, prior, storage, active, release)
}

async fn native_approval_pin(timing: ModelChangeTiming, pin: ApprovalMode, admitted: bool) {
    let (agent, prior, storage, active, release) =
        approval_switch(timing, SteeringOutcome::Injected).await;
    let actor =
        ActionContext::new("steering-person", "steering-window", "native-approval").unwrap();
    let submitter = Submitter::owner(actor.clone()).with_approval_mode(pin);
    let request = input("approval-native-correction", 50);
    let writes_before = storage.0.lock().unwrap().writes;
    let result = agent.steer(request.clone(), submitter.clone()).await;
    let calls = prior.backend.steered.lock().unwrap().len();
    let writes_after = storage.0.lock().unwrap().writes;
    if admitted && matches!(result, Ok(SteeringDelivery::Injected { .. })) {
        // Current selection is Full, but retry retrieves the original receipt,
        // including the NextTurn Ask-pinned injection, without another save/call.
        assert!(matches!(
            agent.steer(request.clone(), submitter).await,
            Ok(SteeringDelivery::Injected { .. })
        ));
        assert_eq!(storage.0.lock().unwrap().writes, writes_after);
        assert_eq!(prior.backend.steered.lock().unwrap().len(), 1);
        let audit = prior.backend.audit.lock().unwrap();
        assert!(audit.iter().any(|record| matches!(record, ExecutionAuditRecord::SteeringAcknowledged(record) if record.execution_id() == &request.execution_id && record.actor() == &actor)));
    }
    release.send(()).unwrap();
    assert_eq!(active.wait().await, Ok(ExecutionOutcome::Completed));
    agent.close(close_action()).await.unwrap();
    if admitted {
        assert!(
            matches!(result, Ok(SteeringDelivery::Injected { .. })),
            "active binding pin refused"
        );
        assert_eq!(calls, 1);
    } else {
        assert_eq!(result.err(), Some(AgentError::ApprovalModeChanged));
        assert_eq!(calls, 0);
        assert_eq!(writes_before, writes_after);
        assert!(!prior.backend.audit.lock().unwrap().iter().any(|record| matches!(record, ExecutionAuditRecord::SteeringAcknowledged(record) if record.execution_id() == &request.execution_id)));
    }
}

#[tokio::test]
async fn native_approval_next_turn_accepts_active_pin_and_recovers_retry() {
    native_approval_pin(ModelChangeTiming::NextTurn, ApprovalMode::Ask, true).await;
}
#[tokio::test]
async fn native_approval_next_turn_refuses_selected_only_pin() {
    native_approval_pin(ModelChangeTiming::NextTurn, ApprovalMode::Full, false).await;
}
#[tokio::test]
async fn native_approval_subsequent_responses_uses_selected_pin() {
    native_approval_pin(
        ModelChangeTiming::SubsequentResponses,
        ApprovalMode::Full,
        true,
    )
    .await;
}
#[tokio::test]
async fn native_approval_subsequent_responses_refuses_prior_pin() {
    native_approval_pin(
        ModelChangeTiming::SubsequentResponses,
        ApprovalMode::Ask,
        false,
    )
    .await;
}

async fn approval_fallback(pinned: bool) {
    let (agent, prior, _, active, release) =
        approval_switch(ModelChangeTiming::NextTurn, SteeringOutcome::PromptRequired).await;
    let actor =
        ActionContext::new("fallback-person", "fallback-window", "approval-fallback").unwrap();
    let submitter = Submitter::owner(actor.clone());
    let submitter = if pinned {
        submitter.with_approval_mode(ApprovalMode::Ask)
    } else {
        submitter
    };
    let request = input("approval-queue-fallback", 50);
    let result = agent.steer(request.clone(), submitter).await;
    let calls = prior.backend.steered.lock().unwrap().len();
    let queued = agent.queued_ids().await;
    let snapshot = agent.session_manager().snapshot().await.unwrap();
    let retained = snapshot
        .invocations
        .iter()
        .find(|record| record.request.execution_id == request.execution_id)
        .map(|record| record.request.clone());
    let admission = prior
        .backend
        .audit
        .lock()
        .unwrap()
        .iter()
        .find_map(|record| match record {
            ExecutionAuditRecord::QueueAdmitted(record)
                if record.execution_id() == &request.execution_id =>
            {
                Some(record.clone())
            }
            _ => None,
        });
    release.send(()).unwrap();
    assert_eq!(active.wait().await, Ok(ExecutionOutcome::Completed));
    if pinned {
        assert_eq!(result.err(), Some(AgentError::ApprovalModeChanged));
        assert!(queued.is_empty());
        assert!(admission.is_none());
        assert_eq!(prior.backend.inner.prompts.load(Ordering::SeqCst), 1);
    } else {
        let receipt = match result {
            Ok(SteeringDelivery::Queued(receipt)) => receipt,
            _ => panic!("unpinned fallback must queue under B"),
        };
        assert_eq!(receipt.wait().await, Ok(ExecutionOutcome::Completed));
        let admission = admission.unwrap();
        assert_eq!(admission.actor(), &actor);
        assert_eq!(admission.approval_mode(), Some(ApprovalMode::Full));
        assert_eq!(
            admission.effort_level(),
            Some(&EffortLevel::new("high".into()).unwrap())
        );
        assert_eq!(admission.model(), Some(&agent.provider_identity()));
        assert_eq!(prior.backend.inner.prompts.load(Ordering::SeqCst), 2);
    }
    agent.close(close_action()).await.unwrap();
    assert_eq!(
        calls, 1,
        "A-valid input must reach native nonconsumption first"
    );
    assert_eq!(
        retained,
        Some(request),
        "retain exact attempted steering, without queue ownership on refusal"
    );
}
#[tokio::test]
async fn native_approval_prompt_required_refuses_active_only_pin() {
    approval_fallback(true).await;
}
#[tokio::test]
async fn native_approval_prompt_required_records_current_queue_binding() {
    approval_fallback(false).await;
}

#[tokio::test]
async fn native_approval_foreign_initiator_queues_at_selected_mode() {
    let (agent, prior, _, active, release) =
        approval_switch(ModelChangeTiming::NextTurn, SteeringOutcome::Injected).await;
    agent
        .set_approval_mode(ApprovalMode::Ask, close_action())
        .await
        .unwrap();
    let actor = ActionContext::new("foreign-person", "foreign-window", "foreign-approval").unwrap();
    let request = input("foreign-approval-correction", 50);
    let submitter = Submitter::new(
        actor.clone(),
        Initiation::checked_grant(GrantInitiator::new("foreign-checked", 1).unwrap()),
    )
    .with_approval_mode(ApprovalMode::Ask);
    let result = agent.steer(request.clone(), submitter).await;
    let calls = prior.backend.steered.lock().unwrap().len();
    release.send(()).unwrap();
    assert_eq!(active.wait().await, Ok(ExecutionOutcome::Completed));
    let receipt = match result {
        Ok(SteeringDelivery::Queued(receipt)) => receipt,
        _ => panic!("foreign input belongs to selected B"),
    };
    assert_eq!(receipt.wait().await, Ok(ExecutionOutcome::Completed));
    agent.close(close_action()).await.unwrap();
    assert_eq!(calls, 0);
    let audit = prior.backend.audit.lock().unwrap();
    let record = audit
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
    assert_eq!(record.approval_mode(), Some(ApprovalMode::Ask));
    assert_eq!(record.actor(), &actor);
}

#[tokio::test]
async fn native_approval_finished_target_refuses_active_only_pin() {
    let (agent, prior, _, active, release) =
        approval_switch(ModelChangeTiming::NextTurn, SteeringOutcome::Injected).await;
    let check = Arc::new(FinishTarget {
        agent: agent.clone(),
        release: Mutex::new(Some(release)),
        ran: AtomicUsize::new(0),
    });
    let request = input("finished-approval-correction", 50);
    let result = agent
        .steer(
            request.clone(),
            Submitter::owner(close_action())
                .with_approval_mode(ApprovalMode::Ask)
                .with_after_save(check.clone()),
        )
        .await;
    if let Some(release) = check.release.lock().unwrap().take() {
        release.send(()).unwrap();
    }
    assert_eq!(active.wait().await, Ok(ExecutionOutcome::Completed));
    let queued = agent.queued_ids().await;
    agent.close(close_action()).await.unwrap();
    assert_eq!(result.err(), Some(AgentError::ApprovalModeChanged));
    assert_eq!(check.ran.load(Ordering::SeqCst), 1);
    assert!(prior.backend.steered.lock().unwrap().is_empty());
    assert!(queued.is_empty());
    assert!(!prior.backend.audit.lock().unwrap().iter().any(|record| matches!(record, ExecutionAuditRecord::QueueAdmitted(record) if record.execution_id() == &request.execution_id)));
}

async fn direct_approval_native_pin(after_next_turn: bool, admitted: bool) {
    let (agent, prior, _, active, release) = if after_next_turn {
        approval_switch(ModelChangeTiming::NextTurn, SteeringOutcome::Injected).await
    } else {
        let (release, hold) = oneshot::channel();
        let (agent, prior, storage) = prepared(Some(hold)).await;
        *prior.backend.steering.lock().unwrap() = Some(SteeringOutcome::Injected);
        let active = agent
            .enqueue(
                input("direct-approval-owner", 50),
                Submitter::owner(close_action()),
            )
            .await
            .unwrap();
        timeout(Duration::from_secs(3), prior.backend.started.notified())
            .await
            .unwrap();
        (agent, prior, storage, active, release)
    };
    let mode = if after_next_turn {
        ApprovalMode::Auto
    } else {
        ApprovalMode::Full
    };
    agent.set_approval_mode(mode, close_action()).await.unwrap();
    let pin = if admitted { mode } else { ApprovalMode::Ask };
    let request = input(
        "direct-approval-native",
        if after_next_turn { 400 } else { 50 },
    );
    let result = agent
        .steer(
            request,
            Submitter::owner(close_action()).with_approval_mode(pin),
        )
        .await;
    let calls = prior.backend.steered.lock().unwrap().len();
    release.send(()).unwrap();
    assert_eq!(active.wait().await, Ok(ExecutionOutcome::Completed));
    agent.close(close_action()).await.unwrap();
    if admitted {
        assert!(matches!(result, Ok(SteeringDelivery::Injected { .. })), "direct ACK mode must govern active native admission without replacing its model limits");
        assert_eq!(calls, 1);
    } else {
        assert_eq!(result.err(), Some(AgentError::ApprovalModeChanged));
        assert_eq!(calls, 0);
    }
}
#[tokio::test]
async fn native_direct_approval_ack_accepts_new_pin() {
    direct_approval_native_pin(false, true).await;
}
#[tokio::test]
async fn native_direct_approval_ack_refuses_old_pin() {
    direct_approval_native_pin(false, false).await;
}
#[tokio::test]
async fn native_direct_approval_after_next_turn_preserves_active_model() {
    direct_approval_native_pin(true, true).await;
}
#[tokio::test]
async fn native_direct_approval_after_next_turn_refuses_old_pin() {
    direct_approval_native_pin(true, false).await;
}
