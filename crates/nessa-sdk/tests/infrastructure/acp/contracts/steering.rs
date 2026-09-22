use super::support::*;
use crate::application::agent_execution::providers::{
    OperationCapabilities, PermissionDeferralCapability, PermissionDenialCapability,
};

fn assert_capabilities(actual: OperationCapabilities, native_steering: bool, session_resume: bool) {
    assert!(actual.negotiated());
    assert_eq!(actual.native_steering(), native_steering);
    assert_eq!(actual.session_resume(), session_resume);
    assert!(!actual.image_input());
    assert_eq!(
        actual.permission_denial(),
        PermissionDenialCapability::SupportedForOfferedPermissionReviews
    );
    assert_eq!(
        actual.permission_deferral(),
        PermissionDeferralCapability::UnsupportedNotImplemented
    );
}

async fn steering_session(mode: &str) -> (TempDir, OpenedProviderSession) {
    let (root, mut config, model) = test_acp_configuration(mode, 32);
    config.execution_timeout = None;
    let provider = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    (root, provider.open(None).await.unwrap())
}

#[tokio::test]
async fn steering_injects_into_the_target_and_keeps_output_with_its_invocation() {
    let _slot = process_test_slot().await;
    let (root, mut opened) = steering_session("steering-injected").await;
    let active = start(&opened, "first").await;
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("running:first"))
    );
    assert_eq!(
        opened
            .session
            .steer(ExecutionId::new("first").unwrap(), prompt("adjust course"))
            .await
            .map_err(|failure| failure.into_error())
            .unwrap(),
        SteeringOutcome::Injected
    );
    let event = opened
        .events
        .next()
        .await
        .map_err(|failure| failure.into_error())
        .unwrap()
        .unwrap();
    assert_eq!(event.execution_id().as_str(), "first");
    assert_eq!(
        event.update(),
        &ExecutionUpdate::Message(MessageChunk::text("steered:adjust course"))
    );
    assert_eq!(active.await.unwrap().unwrap(), ExecutionOutcome::Completed);
    let observed: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.path().join("steering-observed")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        observed,
        serde_json::json!([{"type":"text","text":"adjust course"}])
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn idle_race_returns_prompt_required_without_a_detached_invocation() {
    let _slot = process_test_slot().await;
    let (root, mut opened) = steering_session("steering-required").await;
    let active = start(&opened, "first").await;
    next(&mut opened).await;
    assert_eq!(
        opened
            .session
            .steer(ExecutionId::new("first").unwrap(), prompt("later"))
            .await
            .map_err(|failure| failure.into_error())
            .unwrap(),
        SteeringOutcome::PromptRequired
    );
    assert_eq!(active.await.unwrap().unwrap(), ExecutionOutcome::Completed);
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.path().join("saved-session")).unwrap())
            .unwrap();
    assert_eq!(saved["history"], serde_json::json!(["first"]));
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
}

#[tokio::test]
async fn stale_target_and_unsupported_steering_never_send_the_message() {
    let _slot = process_test_slot().await;
    for mode in ["steering-injected", "steering-unsupported"] {
        let (root, mut opened) = steering_session(mode).await;
        let active = start(&opened, "newer").await;
        next(&mut opened).await;
        let result = opened
            .session
            .steer(ExecutionId::new("old").unwrap(), prompt("must not arrive"))
            .await
            .map_err(|failure| failure.into_error());
        if mode == "steering-unsupported" {
            assert!(matches!(result, Err(AgentError::Unsupported(_))));
        } else {
            assert_eq!(result.unwrap(), SteeringOutcome::PromptRequired);
        }
        assert!(!root.path().join("steering-observed").exists());
        let mut invalid = prompt("invalid");
        invalid.estimated_input_tokens = u64::MAX;
        assert!(matches!(
            opened
                .session
                .steer(ExecutionId::new("newer").unwrap(), invalid)
                .await
                .map_err(|failure| failure.into_error()),
            Err(AgentError::InvalidInput(_))
        ));
        assert!(!root.path().join("steering-observed").exists());
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
        assert_eq!(active.await.unwrap().unwrap(), ExecutionOutcome::Cancelled);
    }
}

#[tokio::test]
async fn ambiguous_steering_errors_are_terminal_and_never_replayed() {
    let _slot = process_test_slot().await;
    for mode in [
        "steering-malformed",
        "steering-detached",
        "steering-wrong-id",
        "steering-error",
        "steering-transport",
    ] {
        let (root, mut opened) = steering_session(mode).await;
        let active = start(&opened, "first").await;
        next(&mut opened).await;
        let result = timeout(
            Duration::from_secs(5),
            opened
                .session
                .steer(ExecutionId::new("first").unwrap(), prompt("ambiguous")),
        )
        .await
        .unwrap()
        .map_err(|failure| failure.into_error());
        assert!(result.is_err(), "{mode}");
        if mode == "steering-error" {
            assert_eq!(result, Err(AgentError::Provider { code: -32001 }));
        }
        assert!(active.await.unwrap().is_err());
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("steering-count")).unwrap(),
            "1"
        );
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn pending_steering_blocks_new_prompts_and_close_settles_the_reply() {
    let _slot = process_test_slot().await;
    let (root, mut opened) = steering_session("steering-stall").await;
    let active = start(&opened, "first").await;
    next(&mut opened).await;
    let session = opened.session.clone();
    let steering = tokio::spawn(async move {
        session
            .steer(ExecutionId::new("first").unwrap(), prompt("pending"))
            .await
            .map_err(|failure| failure.into_error())
    });
    wait_for_file(&root, "steering-observed").await;
    assert_eq!(
        opened.session.execute(prompt("newer")).await.into_result(),
        Err(AgentError::Busy)
    );
    assert_eq!(
        opened
            .session
            .steer(ExecutionId::new("first").unwrap(), prompt("another"))
            .await
            .map_err(|failure| failure.into_error()),
        Err(AgentError::Busy)
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_eq!(steering.await.unwrap(), Err(AgentError::Closed));
    assert_eq!(active.await.unwrap().unwrap(), ExecutionOutcome::Cancelled);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn steering_has_a_deadline_even_when_execution_has_none() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, mut config, model) = test_acp_configuration("steering-stall", 16);
    config.execution_timeout = None;
    let binding = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        audit.clone(),
    )
    .unwrap();
    let mut opened = binding.open(None).await.unwrap();
    let active = start(&opened, "first").await;
    next(&mut opened).await;
    let session = opened.session.clone();
    let steering = tokio::spawn(async move {
        session
            .steer(ExecutionId::new("first").unwrap(), prompt("pending"))
            .await
            .map_err(|failure| failure.into_error())
    });
    wait_for_file(&root, "steering-observed").await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(6)).await;
    tokio::time::resume();
    assert_eq!(
        timeout(Duration::from_secs(5), steering)
            .await
            .unwrap()
            .unwrap(),
        Err(AgentError::Deadline)
    );
    assert_eq!(active.await.unwrap(), Err(AgentError::Deadline));
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    let closures = audit.closures.lock().unwrap();
    let finishes = audit.finishes.lock().unwrap();
    assert_eq!(closures.len(), 1);
    assert_eq!(finishes.len(), 1);
    assert_eq!(
        closures[0].closure().reason(),
        &PermissionCancellationReason::deadline_exceeded()
    );
    assert_eq!(
        finishes[0].result(),
        &Err(PermissionCancellationReason::deadline_exceeded())
    );
    assert_eq!(
        closures[0].closure().execution_id(),
        Some(finishes[0].execution_id())
    );
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn a_completed_target_cannot_steer_the_next_running_invocation() {
    let _slot = process_test_slot().await;
    let (root, mut opened) = steering_session("steering-injected").await;
    let first = start(&opened, "first").await;
    next(&mut opened).await;
    assert_eq!(
        opened
            .session
            .steer(ExecutionId::new("first").unwrap(), prompt("finish first"))
            .await
            .map_err(|failure| failure.into_error())
            .unwrap(),
        SteeringOutcome::Injected
    );
    next(&mut opened).await;
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Finished(ExecutionOutcome::Completed)
    );
    assert_eq!(first.await.unwrap().unwrap(), ExecutionOutcome::Completed);
    let second = start(&opened, "second").await;
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("running:second"))
    );
    assert_eq!(
        opened
            .session
            .steer(ExecutionId::new("first").unwrap(), prompt("stale"))
            .await
            .map_err(|failure| failure.into_error())
            .unwrap(),
        SteeringOutcome::PromptRequired
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("steering-count")).unwrap(),
        "1"
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_eq!(second.await.unwrap().unwrap(), ExecutionOutcome::Cancelled);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn operation_capabilities_follow_successful_negotiation_and_restoration() {
    let _slot = process_test_slot().await;
    for mode in ["steering-unsupported", "resume-unsupported"] {
        let (_root, opened) = steering_session(mode).await;
        assert_capabilities(
            opened.session.operation_capabilities(),
            false,
            mode != "resume-unsupported",
        );
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
    }
    let (_root, mut opened) = steering_session("steering-capabilities-change").await;
    assert_capabilities(opened.session.operation_capabilities(), true, true);
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    // A provider's resume receipt precedes configuration and reader publication.
    // Await preparation, which owns that entire readiness boundary.
    opened.session.prepare_invocation().await.unwrap();
    assert_capabilities(opened.session.operation_capabilities(), false, true);
    let active = start(&opened, "restored").await;
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("running:restored"))
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_eq!(active.await.unwrap().unwrap(), ExecutionOutcome::Cancelled);
}

#[tokio::test]
async fn failed_restoration_clears_previously_negotiated_operation_support() {
    let _slot = process_test_slot().await;
    let (_root, opened) = steering_session("steering-resume-removed").await;
    assert!(opened.session.operation_capabilities().native_steering());
    assert!(opened.session.operation_capabilities().session_resume());
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert!(matches!(
        opened
            .session
            .execute(prompt("restore"))
            .await
            .into_result(),
        Err(AgentError::Unsupported(_))
    ));
    assert_eq!(
        opened.session.operation_capabilities(),
        OperationCapabilities::default()
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
}

#[tokio::test]
async fn oversized_steering_is_rejected_without_interrupting_the_active_prompt() {
    let _slot = process_test_slot().await;
    let (root, mut opened) = steering_session("steering-injected").await;
    let active = start(&opened, "active-survives").await;
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("running:active-survives"))
    );
    let oversized = ExecutionRequest {
        user_message: UserMessage::text_only(PromptText::new("\u{0}".repeat(2048)).unwrap()),
        ..prompt("oversized-steering")
    };
    assert!(matches!(
        opened
            .session
            .steer(ExecutionId::new("active-survives").unwrap(), oversized)
            .await
            .map_err(|failure| failure.into_error()),
        Err(AgentError::MessageTooLarge { .. })
    ));
    assert!(!active.is_finished());
    assert!(!root.path().join("cancel-observed").exists());
    assert!(!root.path().join("steering-observed").exists());
    assert_eq!(
        opened
            .session
            .steer(
                ExecutionId::new("active-survives").unwrap(),
                prompt("valid-followup")
            )
            .await
            .map_err(|failure| failure.into_error())
            .unwrap(),
        SteeringOutcome::Injected
    );
    assert_eq!(active.await.unwrap().unwrap(), ExecutionOutcome::Completed);
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("steered:valid-followup"))
    );
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Finished(ExecutionOutcome::Completed)
    );
    let launches: Vec<u32> =
        serde_json::from_slice(&std::fs::read(root.path().join("launches")).unwrap()).unwrap();
    assert_eq!(launches.len(), 1);
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}
