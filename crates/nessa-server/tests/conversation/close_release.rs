//! When a close that did not reach the agent may still let go of the
//! conversation's uploads: only when nothing saved can still ask for them.
use super::*;
use nessa_sdk::application::agent_execution::{
    permissions::ActionContext,
    providers::ProviderIdentity,
    sessions::{
        InvocationRecord, InvocationSchedulingEvent, SessionSnapshot, SubmissionAcknowledgement,
    },
};
use nessa_sdk::domain::agent_execution::{
    executions::{ExecutionId, ExecutionOutcome, InvocationKind, SchedulingCause, SubmissionMode},
    prompts::{PromptText, UserMessage},
    sessions::{ExecutionSessionId, ProviderContext},
};
use nessa_sdk::domain::common::value_objects::{ImageMediaType, Sha256Digest};

fn snapshot(invocations: Vec<InvocationRecord>) -> SessionSnapshot {
    SessionSnapshot {
        id: SessionId::new("00000000-0000-4000-8000-00000000000c").unwrap(),
        provider: ProviderIdentity::new("gateway-test", "test", "test").unwrap(),
        provider_context: ProviderContext::Recorded(ExecutionSessionId::new("provider-1").unwrap()),
        invocations,
        queue_history: Vec::new(),
    }
}

/// One saved turn: with or without images, at a stage, settled or not.
fn invocation(
    images: bool,
    stage: InvocationStage,
    result: Option<Result<ExecutionOutcome, AgentError>>,
) -> InvocationRecord {
    let message = if images {
        UserMessage::new(
            Some(PromptText::new("look").unwrap()),
            vec![
                ImageReference::new(Sha256Digest::from_bytes([7; 32]), ImageMediaType::Png, 1024)
                    .unwrap(),
            ],
            Vec::new(),
        )
        .unwrap()
    } else {
        UserMessage::text_only(PromptText::new("hello").unwrap())
    };
    InvocationRecord {
        target_event_offset: None,
        submission: SubmissionMode::Queued,
        request: ExecutionRequest {
            execution_id: ExecutionId::new("turn-1").unwrap(),
            user_message: message,
            estimated_input_tokens: 1,
            reserved_output_tokens: 1,
        },
        actor: ActionContext::new("person", "panel", "turn-1").unwrap(),
        acknowledgement: SubmissionAcknowledgement::Acknowledged,
        events: Vec::new(),
        scheduling: vec![InvocationSchedulingEvent {
            kind: InvocationKind::Queued,
            target: None,
            before: None,
            stage,
            cause: SchedulingCause::Submitted,
            actor: None,
        }],
        cancellation: None,
        provider_report: None,
        local_cancellation: None,
        local_outcome: None,
        result,
    }
}

#[test]
fn only_a_turn_that_names_images_and_has_not_settled_holds_its_uploads_back() {
    let settled = || Some(Ok(ExecutionOutcome::Completed));
    for (images, stage, result, awaits) in [
        // A queued or running turn with images is dispatched when the agent
        // next opens, and reads its bytes then.
        (true, InvocationStage::Queued, None, true),
        (true, InvocationStage::Running, None, true),
        // Every way of being over: nothing will ask for those bytes again.
        (true, InvocationStage::Settled, None, false),
        (true, InvocationStage::Cancelled, None, false),
        (true, InvocationStage::Injected, None, false),
        (true, InvocationStage::Queued, settled(), false),
        (true, InvocationStage::Running, settled(), false),
        // A turn that names no images never holds any back.
        (false, InvocationStage::Queued, None, false),
        (false, InvocationStage::Running, None, false),
    ] {
        assert_eq!(
            awaits_images(Some(&snapshot(vec![invocation(images, stage, result)]))),
            awaits,
            "{images} {stage:?}"
        );
    }
    // An empty session, and a session of settled turns, hold nothing back.
    assert!(!awaits_images(Some(&snapshot(Vec::new()))));
    assert!(!awaits_images(Some(&snapshot(vec![
        invocation(true, InvocationStage::Settled, None),
        invocation(false, InvocationStage::Queued, None),
    ]))));
    // One unsettled image turn among settled ones is enough.
    assert!(awaits_images(Some(&snapshot(vec![
        invocation(true, InvocationStage::Settled, None),
        invocation(true, InvocationStage::Queued, None),
    ]))));
    // No snapshot is not knowing there is nothing.
    assert!(awaits_images(None));
}
