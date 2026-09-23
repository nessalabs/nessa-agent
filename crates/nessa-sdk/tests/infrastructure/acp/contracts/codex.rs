//! The Codex profile against a handler speaking Codex's own shapes: what it
//! selects, what it refuses to proceed without, and what survives translation.
use super::support::*;
use crate::domain::agent_execution::tools::ToolContent;

#[tokio::test]
async fn a_session_is_opened_configured_and_prompted_through_the_shared_runtime() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_codex_binding("echo", 16);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
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
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
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
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
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
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
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
                .open(ProviderOpenRequest::without_startup_control(None))
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
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
            .err()
            .map(|failure| failure.cause().clone())
            .unwrap(),
        AgentError::Provider {
            code: -32042,
            diagnostic: Some(ProviderDiagnostic::new("unknown model")),
        }
    );
    wait_until_gone(&root, "pid").await;
}

#[tokio::test]
async fn a_codex_nothing_has_signed_in_refuses_its_session_and_is_reported_as_codex_said() {
    let _process_slot = process_test_slot().await;
    // Where the one live run against a real Codex ended. Its adapter refuses
    // `session/new` outright when nothing has signed it in: the client sent no
    // ACP `authenticate` — this runtime is shared across vendors and signing in
    // is not something it does — and the launch named no default sign-in
    // request either.
    //
    // The binding's job on that path is to report what Codex said rather than
    // to dress it as a transport or configuration fault, and to leave nothing
    // running behind it. Covered here so it is a contract instead of something
    // only an authenticated machine could ever discover.
    let (root, binding) = test_codex_binding("not-signed-in", 16);
    assert_eq!(
        binding
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
            .err()
            .map(|failure| failure.cause().clone())
            .unwrap(),
        AgentError::Provider {
            code: -32000,
            diagnostic: Some(ProviderDiagnostic::new("Authentication required")),
        }
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
        image_input: None,
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
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
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

#[tokio::test]
async fn codex_steers_by_queue_although_its_adapter_offers_the_extension() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_codex_binding("echo", 16);
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    // The fixture advertises `_meta.steering.supported` because the pinned
    // adapter does. What it does not implement is the contract that goes with
    // it: a steer arriving with no live turn is answered by starting a turn of
    // Codex's own, owned by no prompt this runtime sent, where the shared
    // worker requires `promptRequired` and would read anything else as a
    // protocol violation and tear the session down. The profile declines, so
    // steering queues a prompt instead — see `codex_acp/sessions/profile.rs`.
    let capabilities = opened.session.operation_capabilities();
    assert!(capabilities.negotiated());
    assert!(!capabilities.native_steering());
    assert!(capabilities.session_resume());
    // The fixture binding is given no image source, so this connection carries
    // none whatever the agent advertised.
    assert!(!capabilities.image_input());
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn a_restored_codex_session_is_configured_again_before_it_is_used() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_codex_binding("echo", 16);
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    assert_eq!(
        opened
            .session
            .execute(prompt("before"))
            .await
            .into_result()
            .unwrap(),
        ExecutionOutcome::Completed
    );
    let id = opened.session.id().clone();
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();

    // The path every Codex conversation takes on every gateway restart, and the
    // one place the two-step configuration is applied over a provider state
    // this runtime did not just create.
    let restored = binding
        .open(ProviderOpenRequest::without_startup_control(Some(id)))
        .await
        .unwrap();
    assert_eq!(
        restored
            .session
            .execute(prompt("after"))
            .await
            .into_result()
            .unwrap(),
        ExecutionOutcome::Completed
    );
    restored
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");

    // Resumed rather than opened again: both answer a prompt, and only one of
    // them is the path a gateway restart takes.
    assert!(root.path().join("resumed").is_file());
    // Twice: once for the session that was opened, once for the one that was
    // resumed. The fixture itself holds the order — a model settled before the
    // approval mode — and writes a line only after both have arrived.
    let configured = std::fs::read_to_string(root.path().join("configured")).unwrap();
    assert_eq!(
        configured.lines().collect::<Vec<_>>(),
        [r#"["model", "mode"]"#, r#"["model", "mode"]"#],
    );
}

#[tokio::test]
async fn a_refused_codex_approval_is_the_answer_codex_is_given() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_codex_binding("file-change-permission", 16);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let running = start(&opened, "edit the config").await;
    let ExecutionUpdate::PermissionRequested { id, options, .. } = next(&mut opened).await else {
        panic!("expected permission");
    };
    // The refusal Codex itself offered, chosen by what it means rather than by
    // the name Codex gave it: `reject_once` is Codex's spelling, and what this
    // binding answers with has to be the option Codex sent.
    let refuse = options
        .choices()
        .iter()
        .find(|option| {
            option.decision().clone()
                == PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request())
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
            option_id: refuse.clone(),
        })
        .await
        .map_err(|failure| failure.into_error())
        .unwrap();
    // A refusal is an answer, so the turn finishes rather than failing.
    assert_eq!(running.await.unwrap().unwrap(), ExecutionOutcome::Completed);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(root.path().join("permission-outcome")).unwrap()
        )
        .unwrap(),
        serde_json::json!({"outcome":"selected","optionId":refuse.as_str()})
    );
}

#[tokio::test]
async fn a_codex_approval_the_audit_cannot_record_is_never_given_to_codex() {
    let _process_slot = process_test_slot().await;
    let (root, config, model) = codex_configuration("file-change-permission", 16);
    // Every record refused, which for this run is the answer itself: nothing
    // else is written before it.
    let audit = Arc::new(RecordingAudit {
        reject: true,
        ..Default::default()
    });
    let binding =
        CodexAcpProvider::new(config, &model, TokenLimits::new(900, 100).unwrap(), audit).unwrap();
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let running = start(&opened, "edit the config").await;
    let ExecutionUpdate::PermissionRequested { id, options, .. } = next(&mut opened).await else {
        panic!("expected permission");
    };
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
    let failure = opened
        .session
        .answer_permission(PermissionAnswer {
            attribution: attribution(),
            execution_id: ExecutionId::new("edit the config").unwrap(),
            id,
            option_id: allow,
        })
        .await
        .unwrap_err();
    assert_eq!(failure.error(), &AgentError::AuditFailure);
    assert_eq!(running.await.unwrap(), Err(rejected_audits(3)));
    // The point of refusing: Codex is never told to proceed on a decision
    // nothing could record. What it is told, if anything, is that the request
    // was cancelled — which is the session being torn down around it, not an
    // approval.
    if let Ok(told) = std::fs::read_to_string(root.path().join("permission-outcome")) {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&told).unwrap(),
            serde_json::json!({"outcome": "cancelled"}),
        );
    }
    assert_eq!(
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result(),
        Err(AgentError::AuditFailure)
    );
    assert_gone(&root, "pid");
}
