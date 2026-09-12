use nessa_sdk::domain::agent_execution::{entities::*, value_objects::*, ExecutionError};

#[test]
fn identities_and_prompt_text_preserve_meaning_and_reject_blank_values() {
    for blank in ["", " \n\t"] {
        assert_eq!(
            ExecutionId::new(blank),
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
    // No ACP-specific ID length restriction is imposed on other consumers.
    assert!(ExecutionId::new("x".repeat(300)).is_ok());
    assert_eq!(
        ExecutionError::InvalidPath.to_string(),
        "agent execution: InvalidPath"
    );
    let error: &dyn std::error::Error = &ExecutionError::InvalidPath;
    assert!(error.source().is_none());
}

#[test]
fn paths_are_descriptions_not_filesystem_authority() {
    for invalid in ["", "a\0b"] {
        assert_eq!(FilePath::new(invalid), Err(ExecutionError::InvalidPath));
    }
    for valid in ["../outside", "/not-a-local-file", "C:\\remote\\file", " "] {
        assert_eq!(FilePath::new(valid).unwrap().as_str(), valid);
    }
}

fn update(id: &str) -> ToolCallUpdate {
    ToolCallUpdate::new(ToolCallId::new(id).unwrap(), None, None, None, None, None)
}

#[test]
fn tool_observations_merge_without_losing_omitted_fields_or_crossing_identity() {
    let execution = ExecutionId::new("execution").unwrap();
    let original = ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        Some("Read file".into()),
        Some(ToolKind::Read),
        Some(ToolStatus::Running),
        Some(vec![FileLocation::new(
            FilePath::new("a").unwrap(),
            Some(0),
        )]),
        Some(vec![ToolContent::Text("old output".into())]),
    );
    let mut tool = ToolCall::new(execution.clone(), original.clone());
    assert_eq!(tool.execution_id(), &execution);
    assert_eq!(tool.observation(), &original);
    let location = &tool.observation().locations().as_ref().unwrap()[0];
    assert_eq!(location.path().as_str(), "a");
    assert_eq!(location.line(), Some(0));
    tool.apply(&execution, update("tool")).unwrap();
    assert_eq!(tool.observation(), &original);
    let before = tool.clone();
    assert_eq!(
        tool.apply(&ExecutionId::new("other").unwrap(), update("tool")),
        Err(ExecutionError::DifferentExecution)
    );
    assert_eq!(
        tool.apply(&execution, update("other")),
        Err(ExecutionError::DifferentTool)
    );
    assert_eq!(tool, before);
    let cleared = ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        Some(String::new()),
        Some(ToolKind::Other),
        Some(ToolStatus::Completed),
        Some(vec![]),
        Some(vec![]),
    );
    tool.apply(&execution, cleared.clone()).unwrap();
    assert_eq!(tool.observation(), &cleared);
}

fn request() -> PermissionRequest {
    PermissionRequest::new(
        PermissionId::new("permission").unwrap(),
        ExecutionId::new("execution").unwrap(),
        ToolCallId::new("tool").unwrap(),
        FileToolInput::Write {
            path: FilePath::new("a").unwrap(),
            content: String::new(),
        },
    )
}

#[test]
fn permissions_are_scoped_and_resolved_once_without_consuming_invalid_answers() {
    for decision in [
        PermissionDecision::AllowOnce,
        PermissionDecision::RejectOnce,
    ] {
        let mut permission = request();
        assert_eq!(permission.id().as_str(), "permission");
        assert_eq!(permission.execution_id().as_str(), "execution");
        assert_eq!(permission.tool_id().as_str(), "tool");
        assert!(
            matches!(permission.input(), FileToolInput::Write { content, .. } if content.is_empty())
        );
        assert_eq!(permission.state(), PermissionState::Pending);
        assert_eq!(
            permission.answer(&ExecutionId::new("other").unwrap(), decision),
            Err(ExecutionError::DifferentExecution)
        );
        assert_eq!(permission.state(), PermissionState::Pending);
        let execution = permission.execution_id().clone();
        permission.answer(&execution, decision).unwrap();
        assert_eq!(permission.state(), PermissionState::Answered(decision));
        assert_eq!(
            permission.answer(&execution, decision),
            Err(ExecutionError::PermissionResolved)
        );
        assert_eq!(permission.cancel(), Err(ExecutionError::PermissionResolved));
        assert_eq!(permission.state(), PermissionState::Answered(decision));
    }
    let mut permission = request();
    permission.cancel().unwrap();
    assert_eq!(permission.state(), PermissionState::Cancelled);
    assert_eq!(permission.cancel(), Err(ExecutionError::PermissionResolved));
    assert_eq!(
        permission.answer(
            &ExecutionId::new("execution").unwrap(),
            PermissionDecision::AllowOnce
        ),
        Err(ExecutionError::PermissionResolved)
    );
}
