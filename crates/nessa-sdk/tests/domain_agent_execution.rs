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

fn choice(id: &str, decision: PermissionDecision) -> PermissionOption {
    PermissionOption::new(
        PermissionOptionId::new(id).unwrap(),
        format!("Choice {id}"),
        decision,
    )
    .unwrap()
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
        PermissionOptions::new(
            vec![
                choice("allow", PermissionDecision::AllowOnce),
                choice("reject", PermissionDecision::RejectOnce),
            ],
            &PermissionConfig::once_only(),
        )
        .unwrap(),
    )
}

#[test]
fn permissions_are_scoped_and_resolved_once_without_consuming_invalid_answers() {
    for (id, decision) in [
        ("allow", PermissionDecision::AllowOnce),
        ("reject", PermissionDecision::RejectOnce),
    ] {
        let mut permission = request();
        assert_eq!(permission.id().as_str(), "permission");
        assert_eq!(permission.execution_id().as_str(), "execution");
        assert_eq!(permission.tool_id().as_str(), "tool");
        assert!(
            matches!(permission.input(), FileToolInput::Write { content, .. } if content.is_empty())
        );
        let option_id = PermissionOptionId::new(id).unwrap();
        assert_eq!(permission.options().choices().len(), 2);
        assert_eq!(permission.state(), &PermissionState::Pending);
        assert_eq!(
            permission.answer(&ExecutionId::new("other").unwrap(), &option_id),
            Err(ExecutionError::DifferentExecution)
        );
        let execution = permission.execution_id().clone();
        assert_eq!(
            permission.answer(&execution, &PermissionOptionId::new("unknown").unwrap()),
            Err(ExecutionError::UnknownPermissionOption)
        );
        assert_eq!(permission.state(), &PermissionState::Pending);
        assert_eq!(permission.answer(&execution, &option_id).unwrap(), decision);
        let expected = PermissionState::Answered {
            option_id: option_id.clone(),
            decision,
        };
        assert_eq!(permission.state(), &expected);
        assert_eq!(
            permission.answer(&execution, &option_id),
            Err(ExecutionError::PermissionResolved)
        );
        assert_eq!(permission.cancel(), Err(ExecutionError::PermissionResolved));
        assert_eq!(permission.state(), &expected);
    }
    let mut permission = request();
    permission.cancel().unwrap();
    assert_eq!(permission.state(), &PermissionState::Cancelled);
    assert_eq!(permission.cancel(), Err(ExecutionError::PermissionResolved));
    assert_eq!(
        permission.answer(
            &ExecutionId::new("execution").unwrap(),
            &PermissionOptionId::new("allow").unwrap()
        ),
        Err(ExecutionError::PermissionResolved)
    );
}

#[test]
fn permission_configuration_preserves_exact_choices_and_filters_disallowed_kinds() {
    assert!(PermissionOptionId::new(" ").is_err());
    let id = PermissionOptionId::new("option").unwrap();
    assert_eq!(id.as_str(), "option");
    assert!(PermissionOption::new(id, " ", PermissionDecision::AllowOnce).is_err());
    assert_eq!(
        PermissionConfig::new(vec![]),
        Err(ExecutionError::NoPermissionOptions)
    );
    assert_eq!(
        PermissionConfig::new(vec![PermissionDecision::AllowOnce; 2]),
        Err(ExecutionError::DuplicatePermissionDecision)
    );
    let config = PermissionConfig::new(vec![
        PermissionDecision::AllowOnce,
        PermissionDecision::RejectOnce,
        PermissionDecision::AllowAlways,
        PermissionDecision::RejectAlways,
    ])
    .unwrap();
    let options = vec![
        choice("once", PermissionDecision::AllowOnce),
        choice("file", PermissionDecision::AllowAlways),
        choice("directory", PermissionDecision::AllowAlways),
        choice("deny", PermissionDecision::RejectAlways),
    ];
    let all = PermissionOptions::new(options.clone(), &config).unwrap();
    assert_eq!(all.choices(), options);
    assert_eq!(
        all.find(&PermissionOptionId::new("directory").unwrap())
            .unwrap()
            .label(),
        "Choice directory"
    );
    let filtered = PermissionOptions::new(options.clone(), &PermissionConfig::once_only()).unwrap();
    assert_eq!(filtered.choices(), &options[..1]);
    assert!(filtered
        .find(&PermissionOptionId::new("file").unwrap())
        .is_none());
    assert_eq!(
        PermissionOptions::new(vec![], &config),
        Err(ExecutionError::NoPermissionOptions)
    );
    assert_eq!(
        PermissionOptions::new(
            vec![choice("always", PermissionDecision::AllowAlways)],
            &PermissionConfig::once_only()
        ),
        Err(ExecutionError::NoPermissionOptions)
    );
    assert_eq!(
        PermissionOptions::new(
            vec![
                choice("duplicate", PermissionDecision::AllowAlways),
                choice("duplicate", PermissionDecision::RejectAlways)
            ],
            &PermissionConfig::once_only()
        ),
        Err(ExecutionError::DuplicatePermissionOption)
    );
    for id in ["file", "directory", "deny"] {
        let mut permission = PermissionRequest::new(
            PermissionId::new("request").unwrap(),
            ExecutionId::new("execution").unwrap(),
            ToolCallId::new("tool").unwrap(),
            request().input().clone(),
            all.clone(),
        );
        let option_id = PermissionOptionId::new(id).unwrap();
        let expected = all.find(&option_id).unwrap().decision();
        assert_eq!(
            permission
                .answer(&ExecutionId::new("execution").unwrap(), &option_id)
                .unwrap(),
            expected
        );
        assert_eq!(
            permission.state(),
            &PermissionState::Answered {
                option_id,
                decision: expected
            }
        );
    }
}

#[test]
fn prompt_builder_composes_sources_in_order_and_validates_the_finished_content() {
    use nessa_sdk::domain::agent_execution::builders::PromptBuilder;
    for builder in [
        PromptBuilder::new(),
        PromptBuilder::default().text(" ").text("\n"),
    ] {
        assert_eq!(
            builder.build(),
            Err(ExecutionError::EmptyValue("prompt text"))
        );
    }
    let instructions = String::from("Instructions: α");
    let user = "User: inspect tools";
    let prompt = PromptBuilder::new()
        .text(&instructions)
        .text("\n\n")
        .text(user)
        .text("")
        .build()
        .unwrap();
    assert_eq!(
        prompt.text().as_str(),
        "Instructions: α\n\nUser: inspect tools"
    );
    assert_eq!(
        prompt,
        Prompt::new(PromptText::new("Instructions: α\n\nUser: inspect tools").unwrap())
    );
    assert_eq!(
        PromptBuilder::new()
            .text("independent")
            .build()
            .unwrap()
            .text()
            .as_str(),
        "independent"
    );
    assert_eq!(prompt.clone().text(), prompt.text());
}

#[test]
fn agent_turn_events_carry_domain_content_and_validated_execution_identity() {
    use nessa_sdk::domain::agent_execution::events::{AgentTurnEvent, AgentTurnUpdate};
    let id = ExecutionId::new("execution").unwrap();
    let update = AgentTurnUpdate::Message(MessageChunk::Text(" text ".into()));
    let event = AgentTurnEvent::new(id.clone(), update.clone());
    assert_eq!(event.execution_id(), &id);
    assert_eq!(event.update(), &update);
    assert_eq!(event.into_update(), update);
}
