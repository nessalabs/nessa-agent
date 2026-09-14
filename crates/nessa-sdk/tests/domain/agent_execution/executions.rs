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
