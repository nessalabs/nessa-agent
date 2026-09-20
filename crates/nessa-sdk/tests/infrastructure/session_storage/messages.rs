//! File decoding enforces the same actual message-byte limit as live admission.
use super::*;

#[tokio::test]
async fn file_decode_rejects_oversized_message_without_rewriting() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(directory.clone()).unwrap();
    let mut value = snapshot("message-limit");
    value.invocations[0].request.user_message = UserMessage::text_only(
        PromptText::new("é".repeat(ExecutionRequest::MAX_MESSAGE_BYTES / 2)).unwrap(),
    );
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value.clone()).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
    let path = journal_path(&directory, "message-limit");
    let mut invalid: serde_json::Value = snapshot_json(&std::fs::read(&path).unwrap()).unwrap();
    let request = invalid.pointer_mut("/invocations/0/user_message").unwrap();
    *request = format!("{}x", value.invocations[0].request.user_message.text_str()).into();
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

fn image(seed: u8, media_type: ImageMediaType, size: u64) -> ImageReference {
    ImageReference::new(Sha256Digest::from_bytes([seed; 32]), media_type, size).unwrap()
}

#[tokio::test]
async fn image_references_survive_file_storage_in_order_with_and_without_text() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(directory.clone()).unwrap();
    let images = vec![
        image(2, ImageMediaType::Webp, ImageReference::MAX_BYTES),
        image(1, ImageMediaType::Png, 1),
        image(3, ImageMediaType::Gif, 1024),
        image(1, ImageMediaType::Png, 1),
    ];
    for (id, text) in [("with-text", Some(" look \n")), ("images-alone", None)] {
        let mut value = snapshot(id);
        value.invocations[0].request.user_message = UserMessage::new(
            text.map(|text| PromptText::new(text).unwrap()),
            images.clone(),
        )
        .unwrap();
        let lease = storage.open(value.id.clone()).await.unwrap();
        lease.save(value.clone()).await.unwrap();
        let restored = lease.load().await.unwrap().unwrap();
        assert_same(&restored, &value);
        assert_eq!(
            restored.invocations[0].request.user_message.images(),
            images
        );
        assert_eq!(
            restored.invocations[0]
                .request
                .user_message
                .text()
                .is_some(),
            text.is_some()
        );
        // Only references are saved: never a byte of any image.
        let saved = snapshot_json(&std::fs::read(journal_path(&directory, id)).unwrap()).unwrap();
        assert_eq!(
            saved.pointer("/invocations/0/user_images/1").unwrap(),
            &serde_json::json!({
                "digest": format!("sha256:{}", "01".repeat(32)),
                "media_type": "image/png",
                "size": 1,
            })
        );
    }
}

#[tokio::test]
async fn saved_image_references_are_restored_through_the_message_rules() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(directory.clone()).unwrap();
    let mut value = snapshot("image-rules");
    value.invocations[0].request.user_message =
        UserMessage::new(None, vec![image(1, ImageMediaType::Png, 1)]).unwrap();
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value.clone()).await.unwrap();
    let path = journal_path(&directory, "image-rules");
    let original = std::fs::read(&path).unwrap();
    let valid: serde_json::Value = snapshot_json(&original).unwrap();
    let one = valid
        .pointer("/invocations/0/user_images/0")
        .unwrap()
        .clone();

    let corruptions: Vec<(&str, serde_json::Value)> = vec![
        ("/invocations/0/user_images/0/digest", "sha256:00".into()),
        (
            "/invocations/0/user_images/0/digest",
            format!("sha256:{}", "AB".repeat(32)).into(),
        ),
        (
            "/invocations/0/user_images/0/media_type",
            "image/svg+xml".into(),
        ),
        ("/invocations/0/user_images/0/size", 0.into()),
        (
            "/invocations/0/user_images/0/size",
            (ImageReference::MAX_BYTES + 1).into(),
        ),
        // Neither text nor an image is not a message.
        ("/invocations/0/user_images", serde_json::json!([])),
        (
            "/invocations/0/user_images",
            vec![one; UserMessage::MAX_IMAGES + 1].into(),
        ),
    ];
    for (pointer, replacement) in corruptions {
        let mut invalid = valid.clone();
        *invalid.pointer_mut(pointer).unwrap() = replacement;
        let bytes = journal_bytes(&invalid).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(
            matches!(lease.load().await, Err(StorageError::Corrupt(_))),
            "{pointer}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "{pointer}");
    }
    std::fs::write(&path, original).unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
}
