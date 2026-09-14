use super::support::*;
use nessa_sdk::application::agent_execution::executions::{
    ExecutionController, ExecutionEvent, ExecutionUpdate,
};

#[test]
fn agent_turn_events_carry_domain_content_and_validated_execution_identity() {
    let id = ExecutionId::new("execution").unwrap();
    let update = ExecutionUpdate::Message(MessageChunk::text(" text "));
    let event = ExecutionEvent::new(id.clone(), update.clone());
    assert_eq!(event.execution_id(), &id);
    assert_eq!(event.update(), &update);
    assert_eq!(event.into_update(), update);
}

#[test]
fn message_chunk_compaction_preserves_admission_and_payload_limits() {
    let mut controller = ExecutionController::new(ExecutionSessionId::new("chunks").unwrap());
    controller
        .begin_execution(ExecutionId::new("chunk").unwrap())
        .unwrap();
    for thought in [false, true] {
        let mut text = String::with_capacity(ExecutionEvent::MAX_MESSAGE_CHUNK_BYTES + 1);
        text.push('x');
        let chunk = if thought {
            MessageChunk::thought(text)
        } else {
            MessageChunk::text(text)
        };
        assert_eq!(chunk.payload_bytes(), 1);
        assert!(controller
            .message_event(&ExecutionId::new("chunk").unwrap(), chunk)
            .is_ok());
        let oversized = "x".repeat(ExecutionEvent::MAX_MESSAGE_CHUNK_BYTES + 1);
        let chunk = if thought {
            MessageChunk::thought(oversized)
        } else {
            MessageChunk::text(oversized)
        };
        assert!(matches!(
            controller.message_event(&ExecutionId::new("chunk").unwrap(), chunk),
            Err(AgentError::Protocol(_))
        ));
    }
    assert!(controller
        .message_event(
            &ExecutionId::new("chunk").unwrap(),
            MessageChunk::text("valid")
        )
        .is_ok());
}
