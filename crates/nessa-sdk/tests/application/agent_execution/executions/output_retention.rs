use super::*;
use crate::application::agent_execution::executions::ExecutionUpdate;
use crate::domain::agent_execution::executions::ExecutionId;

fn message(text: String) -> ExecutionEvent {
    ExecutionEvent::new(
        ExecutionId::new("output").unwrap(),
        ExecutionUpdate::Message(MessageChunk::thought(text)),
    )
}

#[test]
fn retained_output_counts_exact_utf8_bytes_and_empty_event_slots() {
    let event = message("é".repeat(MAX_MESSAGE_CHUNK_BYTES / 2));
    let mut usage = ObservationUsage::default();
    while usage.bytes + event.retained_bytes() <= MAX_RETAINED_OUTPUT_BYTES {
        usage = usage.with_event(&event).unwrap();
    }
    let overhead = message(String::new()).retained_bytes();
    let remaining = MAX_RETAINED_OUTPUT_BYTES - usage.bytes - overhead;
    let exact = message("x".repeat(remaining));
    usage = usage.with_event(&exact).unwrap();
    assert_eq!(usage.bytes, MAX_RETAINED_OUTPUT_BYTES);
    assert!(matches!(
        usage.with_event(&message(String::new())),
        Err(AgentError::OutputRetentionLimit)
    ));
    let empty = message(String::new());
    let mut usage = ObservationUsage::default();
    for _ in 0..MAX_RETAINED_OUTPUT_EVENTS {
        usage = usage.with_event(&empty).unwrap();
    }
    assert_eq!(usage.count, MAX_RETAINED_OUTPUT_EVENTS);
    assert!(matches!(
        usage.with_event(&empty),
        Err(AgentError::OutputRetentionLimit)
    ));
}

#[test]
fn compacted_snapshot_output_grows_without_exceeding_the_slot_budget() {
    let event = message(String::new());
    // WorkStatus clones existing snapshots, producing exact non-power-of-two capacity.
    let mut events = vec![event.clone(); MAX_RETAINED_OUTPUT_EVENTS / 2 + 1].clone();
    assert_eq!(events.capacity(), events.len());
    reserve_observation_slot(&mut events);
    events.push(event);
    assert_eq!(events.len(), MAX_RETAINED_OUTPUT_EVENTS / 2 + 2);
    assert!(events.capacity() <= MAX_RETAINED_OUTPUT_EVENTS);
    assert!(ObservationUsage::from_events(&events).is_ok());
}
