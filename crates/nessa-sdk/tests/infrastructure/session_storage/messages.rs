//! File decoding enforces the same actual message-byte limit as live admission.
use super::*;

#[tokio::test]
async fn file_decode_rejects_oversized_message_without_rewriting() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(directory.clone()).unwrap();
    let mut value = snapshot("message-limit");
    value.invocations[0].request.user_message =
        PromptText::new("é".repeat(ExecutionRequest::MAX_MESSAGE_BYTES / 2)).unwrap();
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value.clone()).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
    let path = journal_path(&directory, "message-limit");
    let mut invalid: serde_json::Value = snapshot_json(&std::fs::read(&path).unwrap()).unwrap();
    let request = invalid.pointer_mut("/invocations/0/user_message").unwrap();
    *request = format!("{}x", value.invocations[0].request.user_message.as_str()).into();
    let bytes = journal_bytes(&invalid).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[tokio::test]
async fn restored_message_chunks_share_live_limits_without_rewriting() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(directory.clone()).unwrap();
    for thought in [false, true] {
        let mut value = snapshot("chunk-limit");
        let text = "é".repeat(ExecutionEvent::MAX_MESSAGE_CHUNK_BYTES / 2);
        let chunk = if thought {
            MessageChunk::thought(text)
        } else {
            MessageChunk::text(text)
        };
        value.invocations[0].events = vec![ExecutionEvent::new(
            value.invocations[0].request.execution_id.clone(),
            ExecutionUpdate::Message(chunk),
        )];
        let lease = storage.open(value.id.clone()).await.unwrap();
        lease.save(value.clone()).await.unwrap();
        assert_same(&lease.load().await.unwrap().unwrap(), &value);
        let path = journal_path(&directory, "chunk-limit");
        let original_bytes = std::fs::read(&path).unwrap();
        let mut invalid: serde_json::Value = snapshot_json(&original_bytes).unwrap();
        let kind = if thought { "Thought" } else { "Text" };
        let field = invalid
            .pointer_mut(&format!("/invocations/0/events/0/update/{kind}"))
            .unwrap();
        *field = format!("{}x", field.as_str().unwrap()).into();
        let bytes = journal_bytes(&invalid).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        drop(lease);
        std::fs::write(&path, original_bytes).unwrap();
    }
}

#[tokio::test]
async fn custom_storage_cannot_restore_oversized_message_chunks() {
    for thought in [false, true] {
        let mut value = snapshot("chunk-limit");
        let text = "x".repeat(ExecutionEvent::MAX_MESSAGE_CHUNK_BYTES + 1);
        let chunk = if thought {
            MessageChunk::thought(text)
        } else {
            MessageChunk::text(text)
        };
        value.invocations[0].events = vec![ExecutionEvent::new(
            value.invocations[0].request.execution_id.clone(),
            ExecutionUpdate::Message(chunk),
        )];
        super::custom_storage::assert_custom_retention_admission(value, false).await;
    }
}

#[tokio::test]
async fn provider_message_identity_survives_file_storage_without_combining_fragments() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(directory).unwrap();
    let mut value = snapshot("message-identities");
    let execution = value.invocations[0].request.execution_id.clone();
    value.invocations[0].events = vec![
        ExecutionEvent::new(
            execution.clone(),
            ExecutionUpdate::Message(
                MessageChunk::text(" First ").with_message_id(MessageId::new("m1").unwrap()),
            ),
        ),
        ExecutionEvent::new(
            execution.clone(),
            ExecutionUpdate::Message(
                MessageChunk::text("answer.\n").with_message_id(MessageId::new("m1").unwrap()),
            ),
        ),
        ExecutionEvent::new(
            execution,
            ExecutionUpdate::Message(
                MessageChunk::text("Second answer.")
                    .with_message_id(MessageId::new("x".repeat(256)).unwrap()),
            ),
        ),
    ];
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value.clone()).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
}

#[tokio::test]
async fn empty_provider_message_identity_is_rejected_during_file_restoration() {
    assert!(MessageId::new("").is_err());
    let mut value = snapshot("empty-message-identity");
    let execution = value.invocations[0].request.execution_id.clone();
    value.invocations[0].events = vec![ExecutionEvent::new(
        execution,
        ExecutionUpdate::Message(
            MessageChunk::text("x").with_message_id(MessageId::new("valid").unwrap()),
        ),
    )];
    super::custom_storage::assert_custom_retention_admission(value.clone(), true).await;

    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(directory.clone()).unwrap();
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value).await.unwrap();
    let path = journal_path(&directory, "empty-message-identity");
    let original = std::fs::read(&path).unwrap();
    let mut invalid: serde_json::Value = snapshot_json(&original).unwrap();
    *invalid
        .pointer_mut("/invocations/0/events/0/message_id")
        .unwrap() = "".into();
    let bytes = journal_bytes(&invalid).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}
