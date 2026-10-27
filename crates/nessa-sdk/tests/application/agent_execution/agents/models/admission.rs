//! Saved and selected admissions retain their original authority across controls.
use super::*;

#[derive(Clone, Copy, Debug)]
enum Narrowing {
    Context,
    Output,
    Images,
}
fn images() -> ImageInputLimitsDto {
    ImageInputLimitsDto {
        media_types: vec!["image/png".into()],
        max_encoded_bytes: 8,
        max_edge_px: 8000,
        many_images_max_edge_px: 2000,
        native_long_edge_px: 1568,
    }
}
async fn admit(
    agent: &Agent,
    mode: SubmissionMode,
    request: ExecutionRequest,
    submitter: Submitter,
) -> Result<QueueAdmission, AgentError> {
    match mode {
        SubmissionMode::Queued => agent.enqueue(request, submitter).await,
        SubmissionMode::BoundarySteering => agent.enqueue_steering(request, submitter).await,
        SubmissionMode::Steering => match agent.steer(request, submitter).await? {
            SteeringDelivery::Queued(receipt) => Ok(receipt),
            SteeringDelivery::Injected { .. } => {
                panic!("no active target can accept native steering")
            }
        },
        _ => panic!("this matrix covers recoverable queue/steering admissions"),
    }
}
async fn saved_retry(mode: SubmissionMode, narrowing: Narrowing) {
    let caps = capabilities_with_images("large", 1000, Some(images()));
    let (agent, prior, storage) =
        prepared_with_capabilities(None, Normalization::Exact, None, caps).await;
    let mut request = input("saved", 400);
    if matches!(narrowing, Narrowing::Images) {
        request.user_message = UserMessage::new(
            request.user_message.text().cloned(),
            vec![
                ImageReference::new(Sha256Digest::from_bytes([1; 32]), ImageMediaType::Png, 1)
                    .unwrap(),
            ],
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
    }
    let submitter = Submitter::owner(close_action());
    let receipt = admit(&agent, mode, request.clone(), submitter.clone())
        .await
        .unwrap();
    assert_eq!(receipt.wait().await, Ok(ExecutionOutcome::Completed));
    let chosen_caps = match narrowing {
        Narrowing::Context => capabilities_with_images("small", 200, Some(images())),
        Narrowing::Output => capabilities_with_limits("small", 1000, 5, Some(images())),
        Narrowing::Images => capabilities_for("small", 1000),
    };
    let chosen = Arc::new(Provider {
        model: "small".into(),
        capabilities: chosen_caps,
        ..(*prior).clone()
    });
    agent.set_model(chosen, close_action()).await.unwrap();
    let writes = storage.0.lock().unwrap().writes;
    let audit = prior.backend.audit.lock().unwrap().len();
    let dispatches = prior.backend.inner.prompts.load(Ordering::SeqCst);
    let retry = admit(&agent, mode, request.clone(), submitter.clone()).await;
    // Check the conflict and fresh boundaries even when the before-probe loses
    // the exact receipt, so the witness retains all three public outcomes.
    let mut changed = request.clone();
    changed.reserved_output_tokens += 1;
    let conflict = admit(&agent, mode, changed, submitter.clone()).await.err();
    let mut fresh = request;
    fresh.execution_id = ExecutionId::new("fresh").unwrap();
    let fresh_error = admit(&agent, mode, fresh, submitter).await.err();
    assert_eq!(storage.0.lock().unwrap().writes, writes);
    assert_eq!(prior.backend.audit.lock().unwrap().len(), audit);
    assert_eq!(
        prior.backend.inner.prompts.load(Ordering::SeqCst),
        dispatches
    );
    agent.close(close_action()).await.unwrap();
    let retry = match retry {
        Ok(receipt) => receipt,
        Err(error) => panic!(
            "{mode:?}/{narrowing:?}: exact saved receipt was rejected: {error:?}; conflict={conflict:?}; fresh={fresh_error:?}"
        ),
    };
    assert_eq!(retry.wait().await, Ok(ExecutionOutcome::Completed));
    assert_eq!(conflict, Some(AgentError::SubmissionConflict));
    assert!(
        matches!(
            fresh_error,
            Some(AgentError::InvalidInput(_) | AgentError::ImageInputRefused(_))
        ),
        "fresh invalid input: {fresh_error:?}"
    );
}

#[tokio::test]
async fn saved_queue_retry_after_context_narrowing() {
    saved_retry(SubmissionMode::Queued, Narrowing::Context).await;
}
#[tokio::test]
async fn saved_queue_retry_after_output_narrowing() {
    saved_retry(SubmissionMode::Queued, Narrowing::Output).await;
}
#[tokio::test]
async fn saved_queue_retry_after_image_removal() {
    saved_retry(SubmissionMode::Queued, Narrowing::Images).await;
}
#[tokio::test]
async fn saved_boundary_retry_after_context_narrowing() {
    saved_retry(SubmissionMode::BoundarySteering, Narrowing::Context).await;
}
#[tokio::test]
async fn saved_boundary_retry_after_output_narrowing() {
    saved_retry(SubmissionMode::BoundarySteering, Narrowing::Output).await;
}
#[tokio::test]
async fn saved_boundary_retry_after_image_removal() {
    saved_retry(SubmissionMode::BoundarySteering, Narrowing::Images).await;
}
#[tokio::test]
async fn saved_steer_retry_after_context_narrowing() {
    saved_retry(SubmissionMode::Steering, Narrowing::Context).await;
}
#[tokio::test]
async fn saved_steer_retry_after_output_narrowing() {
    saved_retry(SubmissionMode::Steering, Narrowing::Output).await;
}
#[tokio::test]
async fn saved_steer_retry_after_image_removal() {
    saved_retry(SubmissionMode::Steering, Narrowing::Images).await;
}

#[derive(Clone, Copy)]
enum SelectedControl {
    Model,
    Preset,
    UnchangedPreset,
}
async fn selected_preparation(control: SelectedControl) {
    let (agent, prior, _) = prepared(None).await;
    let (entered, gate) = oneshot::channel();
    let (release, hold) = oneshot::channel();
    *prior.backend.preparation_gate.lock().unwrap() = Some((entered, hold));
    let checked = Submitter::new(
        ActionContext::new("driver", "phone", "selected").unwrap(),
        Initiation::checked_grant(GrantInitiator::new("selected-driver", 1).unwrap()),
    );
    let receipt = agent.enqueue(input("selected", 1), checked).await.unwrap();
    timeout(Duration::from_secs(3), gate)
        .await
        .unwrap()
        .unwrap();
    assert!(agent.queued_ids().await.is_empty());
    assert!(agent.active_execution_id().is_none());
    let result = match control {
        SelectedControl::Model => {
            let mut chosen = (*target(&prior)).clone();
            chosen.mode = ApprovalMode::Auto;
            *prior.backend.model_mode.lock().unwrap() = ApprovalMode::Auto;
            agent
                .set_model(Arc::new(chosen), close_action())
                .await
                .map(|_| ())
        }
        SelectedControl::Preset => {
            agent
                .set_approval_mode(ApprovalMode::Full, close_action())
                .await
        }
        SelectedControl::UnchangedPreset => {
            agent
                .set_approval_mode(ApprovalMode::Ask, close_action())
                .await
        }
    };
    release.send(()).unwrap();
    let outcome = timeout(Duration::from_secs(3), receipt.wait())
        .await
        .unwrap();
    let stages = prior
        .backend
        .audit
        .lock()
        .unwrap()
        .iter()
        .filter_map(|record| match record {
            ExecutionAuditRecord::ConfigurationControlled(record) => Some(record.stage()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let native = (
        prior.backend.model_calls.load(Ordering::SeqCst),
        prior.backend.approval_calls.load(Ordering::SeqCst),
    );
    let selected_mode = agent.approval_mode();
    agent.close(close_action()).await.unwrap();
    assert_eq!(
        outcome,
        Ok(ExecutionOutcome::Completed),
        "selected admission lost: control={result:?}, preset={selected_mode:?}, native={native:?}"
    );
    if matches!(control, SelectedControl::UnchangedPreset) {
        result.unwrap();
        assert_eq!(native, (0, 1));
        assert_eq!(
            stages,
            vec![
                ConfigurationControlStage::Requested,
                ConfigurationControlStage::Acknowledged,
                ConfigurationControlStage::Applied
            ]
        );
    } else {
        let error =
            result.expect_err("selected work retains its configuration before native entry");
        assert_eq!(error.error(), &AgentError::Busy);
        assert_eq!(error.stage(), ProviderOperationStage::BeforeEffect);
        assert_eq!(error.session_state(), &ProviderSessionState::Usable);
        assert_eq!(native, (0, 0));
        assert_eq!(
            stages,
            vec![
                ConfigurationControlStage::Requested,
                ConfigurationControlStage::Refused
            ]
        );
    }
}
#[tokio::test]
async fn selected_model_retains_admission_authority() {
    selected_preparation(SelectedControl::Model).await;
}
#[tokio::test]
async fn selected_preset_retains_admission_authority() {
    selected_preparation(SelectedControl::Preset).await;
}
#[tokio::test]
async fn selected_unchanged_preset_remains_supported() {
    selected_preparation(SelectedControl::UnchangedPreset).await;
}
