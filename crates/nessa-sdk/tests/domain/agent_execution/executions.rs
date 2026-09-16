use super::support::*;

#[test]
fn identities_and_prompt_text_preserve_meaning_and_reject_blank_values() {
    for blank in ["", " \n\t"] {
        assert_eq!(
            ExecutionId::new(blank),
            Err(ExecutionError::EmptyValue("execution ID"))
        );
        assert_eq!(
            ExecutionId::new(blank.to_owned()),
            Err(ExecutionError::EmptyValue("execution ID"))
        );
        assert_eq!(
            ToolCallId::new(blank),
            Err(ExecutionError::EmptyValue("tool call ID"))
        );
        assert_eq!(
            PermissionId::new(blank),
            Err(ExecutionError::EmptyValue("permission ID"))
        );
        assert_eq!(
            PromptText::new(blank),
            Err(ExecutionError::EmptyValue("prompt text"))
        );
    }
    let value = " exact value \n";
    assert_eq!(ExecutionId::new(value).unwrap().as_str(), value);
    assert_eq!(ToolCallId::new(value).unwrap().as_str(), value);
    assert_eq!(PermissionId::new(value).unwrap().as_str(), value);
    assert_eq!(PromptText::new(value).unwrap().as_str(), value);
    assert_eq!(
        ExecutionId::new("x".repeat(ExecutionId::MAX_BYTES))
            .unwrap()
            .as_str()
            .len(),
        ExecutionId::MAX_BYTES
    );
    assert_eq!(
        ExecutionId::new("é".repeat(ExecutionId::MAX_BYTES / 2))
            .unwrap()
            .as_str()
            .len(),
        ExecutionId::MAX_BYTES
    );
    for oversized in [
        "x".repeat(ExecutionId::MAX_BYTES + 1),
        "é".repeat(ExecutionId::MAX_BYTES / 2 + 1),
    ] {
        assert_eq!(
            ExecutionId::new(oversized.as_str()),
            Err(ExecutionError::ValueTooLong {
                field: "execution ID",
                max_bytes: ExecutionId::MAX_BYTES
            })
        );
        assert_eq!(
            ExecutionId::new(oversized),
            Err(ExecutionError::ValueTooLong {
                field: "execution ID",
                max_bytes: ExecutionId::MAX_BYTES
            })
        );
    }
    assert_eq!(
        ExecutionError::InvalidPath.to_string(),
        "agent execution: InvalidPath"
    );
    let error: &dyn std::error::Error = &ExecutionError::InvalidPath;
    assert!(error.source().is_none());
}

#[test]
fn message_identity_preserves_exact_utf8_at_its_validated_byte_boundaries() {
    assert_eq!(
        MessageId::new(""),
        Err(ExecutionError::EmptyValue("message ID"))
    );
    assert_eq!(
        MessageId::new("x".repeat(MessageId::MAX_BYTES + 1)),
        Err(ExecutionError::ValueTooLong {
            field: "message ID",
            max_bytes: MessageId::MAX_BYTES,
        })
    );

    let exact = "é".repeat(MessageId::MAX_BYTES / 2);
    let id = MessageId::new(exact.clone()).unwrap();
    assert_eq!(id.as_str(), exact);
    let chunk = MessageChunk::text("payload").with_message_id(id);
    assert_eq!(chunk.message_id(), Some(exact.as_str()));
    assert_eq!(chunk.payload_bytes(), exact.len() + "payload".len());
}
