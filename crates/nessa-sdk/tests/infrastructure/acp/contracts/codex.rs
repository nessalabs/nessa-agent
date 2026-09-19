//! The Codex profile against a handler speaking Codex's own shapes: what it
//! selects, what it refuses to proceed without, and what survives translation.
use super::support::*;
use crate::domain::agent_execution::tools::ToolContent;

#[tokio::test]
async fn a_session_is_opened_configured_and_prompted_through_the_shared_runtime() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_codex_binding("echo", 16);
    let mut opened = binding.open(None).await.unwrap();
    assert_eq!(
        opened.session.capabilities().model().model_id(),
        "exact-fixture-model"
    );
    assert_eq!(
        opened
            .session
            .execute(prompt("first"))
            .await
            .into_result()
            .unwrap(),
        ExecutionOutcome::Completed
    );
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("first"))
    );
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Finished(ExecutionOutcome::Completed)
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn instructions_reach_codex_through_its_own_configuration() {
    let _process_slot = process_test_slot().await;
    let (root, config, model) = codex_configuration("instructions", 16);
    let prompt_text = SystemPromptBuilder::new()
        .text(
            PromptSource::new(PromptSourceKind::Core, "fixture/core").unwrap(),
            "Core instructions.",
        )
        .text(
            PromptSource::new(PromptSourceKind::Plugin, "fixture/plugin").unwrap(),
            "\nPlugin instructions.",
        )
        .build()
        .unwrap();
    let binding = CodexAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    assert!(binding.system_prompt().is_none());
    let binding = binding.with_system_prompt(prompt_text.clone());
    assert_eq!(binding.system_prompt(), Some(&prompt_text));
    // The launch configuration is asserted inside the handler; reaching a
    // completed prompt is how this side learns it was accepted.
    let opened = binding.open(None).await.unwrap();
    assert_eq!(
        opened
            .session
            .execute(prompt("hello"))
            .await
            .into_result()
            .unwrap(),
        ExecutionOutcome::Completed
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn a_shell_command_arrives_with_its_output_and_without_its_terminal_pointer() {
    let _process_slot = process_test_slot().await;
    let (_root, binding) = test_codex_binding("terminal-command", 16);
    let mut opened = binding.open(None).await.unwrap();
    let running = start(&opened, "run the tests").await;
    let ExecutionUpdate::Tool(started) = next(&mut opened).await else {
        panic!("expected the command");
    };
    assert_eq!(started.title().as_deref(), Some("npm test"));
    assert_eq!(started.content().as_deref(), None);
    // Each frame replaces the tool's content rather than adding to it, so every
    // frame has to carry the whole transcript so far. Carrying the delta alone
    // put one chunk of a command's output on screen and dropped the rest.
    for expected in ["compiling\n", "compiling\n2 passed\n"] {
        let ExecutionUpdate::Tool(streamed) = next(&mut opened).await else {
            panic!("expected streamed output");
        };
        assert_eq!(
            streamed.content().as_deref(),
            Some(&vec![ToolContent::text(String::from(expected))][..])
        );
    }
    let ExecutionUpdate::Tool(completed) = next(&mut opened).await else {
        panic!("expected the completion");
    };
    // The completion repeats the whole output, and replaces the accumulated
    // snapshot with it — the same text, shown once.
    assert_eq!(
        completed.content().as_deref(),
        Some(&vec![ToolContent::text(String::from("compiling\n2 passed\n"))][..])
    );
    assert_eq!(running.await.unwrap().unwrap(), ExecutionOutcome::Completed);
}

#[tokio::test]
async fn an_approval_with_no_arguments_still_reaches_the_host_with_what_codex_said() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_codex_binding("file-change-permission", 16);
    let mut opened = binding.open(None).await.unwrap();
    let running = start(&opened, "edit the config").await;
    let ExecutionUpdate::PermissionRequested {
        id, input, options, ..
    } = next(&mut opened).await
    else {
        panic!("expected permission");
    };
    assert_eq!(input.name, "edit");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&input.arguments_json).unwrap(),
        serde_json::json!({"params":{"itemId":"file-change-1","reason":"Modifying config file"}})
    );
    // Codex offers a persistent choice this binding cannot enforce; it is not
    // passed on to the host as one it may take.
    assert_eq!(options.choices().len(), 2);
    let allow = options
        .choices()
        .iter()
        .find(|option| {
            option.decision().clone()
                == PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request())
        })
        .unwrap()
        .id()
        .clone();
    opened
        .session
        .answer_permission(PermissionAnswer {
            attribution: attribution(),
            execution_id: ExecutionId::new("edit the config").unwrap(),
            id,
            option_id: allow.clone(),
        })
        .await
        .map_err(|failure| failure.into_error())
        .unwrap();
    assert_eq!(running.await.unwrap().unwrap(), ExecutionOutcome::Completed);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(root.path().join("permission-outcome")).unwrap()
        )
        .unwrap(),
        serde_json::json!({"outcome":"selected","optionId":allow.as_str()})
    );
}

#[tokio::test]
async fn a_session_is_refused_rather_than_run_half_configured() {
    let _process_slot = process_test_slot().await;
    for (mode, expected) in [
        ("wrong-harness", "requires Codex ACP 1.12.0"),
        ("wrong-version", "requires Codex ACP 1.12.0"),
        (
            "model-not-offered",
            "provider does not offer the configured model",
        ),
        (
            "mode-refused",
            "provider is not in the configured approval mode",
        ),
    ] {
        let _process_slot = process_test_slot().await;
        let (root, binding) = test_codex_binding(mode, 16);
        assert_eq!(
            binding
                .open(None)
                .await
                .err()
                .map(|failure| failure.cause().clone()),
            Some(AgentError::Protocol(expected.into())),
            "{mode}"
        );
        wait_until_gone(&root, "pid").await;
    }
}

#[tokio::test]
async fn a_refused_model_stops_before_the_approval_mode_is_touched() {
    let _process_slot = process_test_slot().await;
    // The handler asserts the ordering itself: reaching the mode step without
    // having settled the model fails the fixture rather than this assertion.
    let (root, binding) = test_codex_binding("model-refused", 16);
    assert_eq!(
        binding
            .open(None)
            .await
            .err()
            .map(|failure| failure.cause().clone())
            .unwrap(),
        AgentError::Provider { code: -32042 }
    );
    wait_until_gone(&root, "pid").await;
}

#[tokio::test]
async fn a_binding_codex_cannot_honour_is_refused_before_a_process_starts() {
    let (_root, config, model) = codex_configuration("echo", 16);
    let limits = TokenLimits::new(900, 100).unwrap();
    let audit = Arc::new(RecordingAudit::default());
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let anthropic = ModelMetadata::try_from(ModelMetadataDto {
        provider: "anthropic".into(),
        model_id: "exact-fixture-model".into(),
        display_name: "Fixture".into(),
        input: text,
        output: text,
        tool_use: true,
        reasoning: true,
        max_context_window_tokens: 1000,
        max_output_tokens: 200,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
    })
    .unwrap();
    assert_eq!(
        CodexAcpProvider::new(config.clone(), &anthropic, limits, audit.clone()).err(),
        Some(AgentError::Configuration(
            "Codex requires an OpenAI model".into()
        ))
    );
    // Codex brings its own tools and cannot be asked to run without them.
    let text_only = AcpConfig {
        tools_enabled: false,
        mcp_servers: Vec::new(),
        ..config.clone()
    };
    assert_eq!(
        CodexAcpProvider::new(text_only, &model, limits, audit.clone()).err(),
        Some(AgentError::Unsupported(
            "Codex always runs its own tools; a text-only Codex binding cannot be configured"
                .into()
        ))
    );
    assert!(CodexAcpProvider::new(config, &model, limits, audit).is_ok());
}

#[tokio::test]
async fn codex_reporting_its_configuration_while_it_is_being_configured_is_not_a_failure() {
    // This profile applies its selections in order — model, then mode — and a
    // provider is free to report its options between the two. Held to the
    // finished state, that notification says the model is not the configured
    // one, because the request that selects it is the one still in flight, and
    // the session dies during startup over a provider telling the truth.
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_codex_binding("startup-update-configuring", 16);
    let opened = binding.open(None).await.unwrap();
    assert_eq!(
        opened
            .session
            .execute(prompt("after the update"))
            .await
            .into_result()
            .unwrap(),
        ExecutionOutcome::Completed
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}
