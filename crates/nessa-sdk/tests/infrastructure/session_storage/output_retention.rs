//! Restored output uses the same per-invocation budget without limiting conversations.
use super::custom_storage::assert_moved_retention_admission;
use super::*;
use std::mem::size_of;

const BYTES: usize = 128 * 1024 * 1024;
const EVENTS: usize = 262_144;

fn output(bytes: usize) -> SessionSnapshot {
    let mut value = snapshot("output-budget");
    let id = value.invocations[0].request.execution_id.clone();
    value.invocations[0].events.clear();
    let overhead = size_of::<ExecutionEvent>() + id.as_str().len();
    let mut remaining = bytes;
    while remaining > 0 {
        let payload = (remaining - overhead).min(ExecutionEvent::MAX_MESSAGE_CHUNK_BYTES);
        value.invocations[0].events.push(ExecutionEvent::new(
            id.clone(),
            ExecutionUpdate::Message(MessageChunk::thought("x".repeat(payload))),
        ));
        remaining -= payload + overhead;
    }
    value
}

#[tokio::test]
async fn custom_storage_output_bytes_accept_exact_limit_and_reject_one_extra() {
    let mut conversation = output(BYTES);
    let mut next = snapshot("another").invocations.remove(0);
    next.request.execution_id = ExecutionId::new("second-output").unwrap();
    next.events = vec![ExecutionEvent::new(
        next.request.execution_id.clone(),
        ExecutionUpdate::Message(MessageChunk::text("another invocation")),
    )];
    conversation.invocations.push(next);
    assert_moved_retention_admission(conversation, true).await;
    assert_moved_retention_admission(output(BYTES + 1), false).await;
    let mut spare = snapshot("spare-output");
    spare.invocations[0].events = Vec::with_capacity(EVENTS + 1);
    assert_moved_retention_admission(spare, false).await;
}

#[tokio::test]
async fn restored_output_count_is_per_invocation_and_file_rejection_does_not_rewrite() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(directory.clone()).unwrap();
    let mut value = snapshot("output-count");
    let id = value.invocations[0].request.execution_id.clone();
    value.invocations[0].events = (0..EVENTS)
        .map(|_| ExecutionEvent::new(id.clone(), ExecutionUpdate::Message(MessageChunk::text(""))))
        .collect();
    assert_moved_retention_admission(value.clone(), true).await;
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value).await.unwrap();
    let path = journal_path(&directory, "output-count");
    let mut wire: serde_json::Value = snapshot_json(&std::fs::read(&path).unwrap()).unwrap();
    let events = wire
        .pointer_mut("/invocations/0/events")
        .unwrap()
        .as_array_mut()
        .unwrap();
    events.push(events[0].clone());
    let bytes = journal_bytes(&wire).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[tokio::test]
async fn retained_output_limit_is_a_typed_persisted_failure() {
    let storage = InMemoryStorage::new();
    let mut value = snapshot("output-error");
    value.invocations[0].result = Some(Err(AgentError::OutputRetentionLimit));
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value.clone()).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(directory).unwrap();
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value.clone()).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
}
