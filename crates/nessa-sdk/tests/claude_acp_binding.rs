#![cfg(unix)]
use nessa_sdk::domain::agent_execution::builders::PromptBuilder;
use nessa_sdk::domain::agent_execution::events::*;
use nessa_sdk::domain::agent_execution::value_objects::*;
use nessa_sdk::{
    application::{
        agent_binding::*,
        dto::{ModalitiesDto, ModelMetadataDto},
    },
    domain::{common::value_objects::TokenLimits, model_metadata::entities::ModelMetadata},
    infrastructure::claude_acp::{ClaudeAcpBinding, ClaudeAcpConfig},
};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use tempfile::TempDir;
use tokio::time::timeout;

fn fixture_configuration(mode: &str, capacity: usize) -> (TempDir, ClaudeAcpConfig, ModelMetadata) {
    let root = tempfile::tempdir().unwrap();
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let model = ModelMetadata::try_from(ModelMetadataDto {
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
    let config = ClaudeAcpConfig {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec![
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/acp_peer.py")
                .into_os_string(),
            mode.into(),
        ],
        environment: BTreeMap::new(),
        workspace: root.path().to_path_buf(),
        file_tools: true,
        permissions: PermissionConfig::once_only(),
        startup_timeout: Duration::from_secs(2),
        prompt_timeout: Some(Duration::from_millis(700)),
        shutdown_grace: Duration::from_millis(100),
        kill_timeout: Duration::from_millis(300),
        event_capacity: capacity,
        max_frame_bytes: 8192,
    };
    (root, config, model)
}
fn fixture_binding(mode: &str, capacity: usize) -> (TempDir, ClaudeAcpBinding) {
    let (root, config, model) = fixture_configuration(mode, capacity);
    (
        root,
        ClaudeAcpBinding::new(config, &model, TokenLimits::new(900, 100).unwrap()).unwrap(),
    )
}
fn prompt(text: &str) -> PromptRequest {
    PromptRequest {
        execution_id: text.into(),
        prompt: PromptBuilder::new().text(text).build().unwrap(),
        input_tokens: 10,
        reserved_output_tokens: 100,
    }
}
async fn start(
    opened: &OpenedBinding,
    text: &str,
) -> tokio::task::JoinHandle<Result<PromptOutcome, BindingError>> {
    let session = Arc::clone(&opened.session);
    let input = prompt(text);
    tokio::spawn(async move { session.prompt(input).await })
}
async fn next(opened: &mut OpenedBinding) -> AgentTurnUpdate {
    timeout(Duration::from_secs(3), opened.events.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_update()
}
fn assert_gone(root: &TempDir, file: &str) {
    let pid: i32 = std::fs::read_to_string(root.path().join(file))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "fixture process {pid} survived"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

#[tokio::test]
async fn streams_two_prompts_with_one_immutable_session_and_repeated_stop() {
    let (root, binding) = fixture_binding("echo", 16);
    let mut opened = binding.open().await.unwrap();
    assert_eq!(
        opened.capabilities.model().model_id(),
        "exact-fixture-model"
    );
    // Resolve both calls before reading any events: old output must retain its
    // execution identity and terminal boundary even while a new prompt runs.
    for input in ["first", "second"] {
        assert_eq!(
            opened.session.prompt(prompt(input)).await.unwrap(),
            PromptOutcome::Completed
        );
    }
    for input in ["first", "second"] {
        let event = opened.events.next().await.unwrap().unwrap();
        assert_eq!(event.execution_id().as_str(), input);
        assert_eq!(
            event.update().clone(),
            AgentTurnUpdate::Message(MessageChunk::Text(input.into()))
        );
        let terminal = opened.events.next().await.unwrap().unwrap();
        assert_eq!(terminal.execution_id().as_str(), input);
        assert_eq!(
            terminal.update().clone(),
            AgentTurnUpdate::Finished(PromptOutcome::Completed)
        );
    }
    let first = opened.session.stop().await.unwrap();
    assert_eq!(opened.session.stop().await.unwrap(), first);
    assert_eq!(
        opened.session.prompt(prompt("late")).await,
        Err(BindingError::Closed)
    );
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn isolated_bindings_and_stop_during_streaming() {
    let (root_a, a) = fixture_binding("stall", 16);
    let (root_b, b) = fixture_binding("echo", 16);
    let mut a = a.open().await.unwrap();
    let mut b = b.open().await.unwrap();
    let active = start(&a, "long").await;
    next(&mut a).await;
    assert_eq!(
        a.session.prompt(prompt("busy")).await,
        Err(BindingError::Busy)
    );
    a.session.stop().await.unwrap();
    assert_eq!(active.await.unwrap().unwrap(), PromptOutcome::Cancelled);
    assert_gone(&root_a, "pid");
    let active = start(&b, "unaffected").await;
    assert_eq!(
        next(&mut b).await,
        AgentTurnUpdate::Message(MessageChunk::Text("unaffected".into()))
    );
    assert_eq!(active.await.unwrap().unwrap(), PromptOutcome::Completed);
    b.session.stop().await.unwrap();
    assert_gone(&root_b, "pid");
}

#[tokio::test]
async fn permission_is_typed_once_only_and_can_be_denied() {
    for allow in [false, true] {
        let (root, binding) = fixture_binding("permission", 16);
        let mut opened = binding.open().await.unwrap();
        let active = start(&opened, "write").await;
        assert!(matches!(next(&mut opened).await, AgentTurnUpdate::Tool(_)));
        let AgentTurnUpdate::PermissionRequested {
            id,
            input,
            tool,
            options,
        } = next(&mut opened).await
        else {
            panic!("expected permission");
        };
        assert_eq!(tool.title().as_deref(), Some("Write fixture.txt"));
        assert_eq!(
            *input,
            FileToolInput::Write {
                path: FilePath::new(
                    std::fs::canonicalize(root.path())
                        .unwrap()
                        .join("fixture.txt")
                        .to_str()
                        .unwrap()
                )
                .unwrap(),
                content: "fixture".into()
            }
        );
        let answer = PermissionAnswer {
            execution_id: "write".into(),
            id: id.as_str().to_owned(),
            option_id: options
                .choices()
                .iter()
                .find(|option| {
                    option.decision()
                        == if allow {
                            PermissionDecision::AllowOnce
                        } else {
                            PermissionDecision::RejectOnce
                        }
                })
                .unwrap()
                .id()
                .as_str()
                .to_owned(),
        };
        assert_eq!(options.choices().len(), 2);
        assert_eq!(
            opened
                .session
                .answer_permission(PermissionAnswer {
                    option_id: "never-choose".into(),
                    ..answer.clone()
                })
                .await,
            Err(BindingError::StalePermission)
        );
        assert_eq!(
            opened
                .session
                .answer_permission(PermissionAnswer {
                    execution_id: "another-execution".into(),
                    ..answer.clone()
                })
                .await,
            Err(BindingError::StalePermission)
        );
        assert!(!root.path().join("fixture.txt").exists());
        opened
            .session
            .answer_permission(answer.clone())
            .await
            .unwrap();
        assert_eq!(
            opened.session.answer_permission(answer).await,
            Err(BindingError::StalePermission)
        );
        assert_eq!(active.await.unwrap().unwrap(), PromptOutcome::Completed);
        assert_eq!(root.path().join("fixture.txt").exists(), allow);
        let AgentTurnUpdate::Tool(patch) = next(&mut opened).await else {
            panic!("expected sparse patch")
        };
        assert_eq!(patch.title().clone(), None);
        assert_eq!(patch.content().clone(), Some(vec![]));
        assert_eq!(patch.locations().clone(), None);
        opened.session.stop().await.unwrap();
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn stop_cancels_pending_permission_before_cleanup() {
    let (root, binding) = fixture_binding("permission-stop", 16);
    let mut opened = binding.open().await.unwrap();
    let active = start(&opened, "write").await;
    next(&mut opened).await;
    let AgentTurnUpdate::PermissionRequested { id, .. } = next(&mut opened).await else {
        panic!("expected permission")
    };
    opened.session.stop().await.unwrap();
    assert_eq!(active.await.unwrap().unwrap(), PromptOutcome::Cancelled);
    let choice: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.path().join("permission-outcome")).unwrap(),
    )
    .unwrap();
    assert_eq!(choice["outcome"], "cancelled");
    assert!(!root.path().join("fixture.txt").exists());
    assert_eq!(
        opened
            .session
            .answer_permission(PermissionAnswer {
                execution_id: "write".into(),
                id: id.as_str().to_owned(),
                option_id: "approve-one".into()
            })
            .await,
        Err(BindingError::Closed)
    );
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn fails_closed_on_configuration_and_startup_deadline() {
    for mode in [
        "wrong-model",
        "wrong-mode",
        "wrong-version",
        "startup-stall",
    ] {
        let (root, binding) = fixture_binding(mode, 16);
        assert!(binding.open().await.is_err(), "{mode}");
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn protocol_failures_and_output_overflow_close_owned_scope() {
    for mode in [
        "malformed",
        "oversize",
        "wrong-session",
        "unknown-reason",
        "config-change",
        "unknown-tool",
        "provider-error",
        "eof",
        "flood",
    ] {
        let (root, binding) = fixture_binding(mode, 2);
        let opened = binding.open().await.unwrap();
        let result = timeout(
            Duration::from_secs(3),
            opened.session.prompt(prompt("test")),
        )
        .await
        .unwrap();
        assert!(result.is_err(), "{mode}: {result:?}");
        if mode == "provider-error" {
            assert_eq!(result, Err(BindingError::Provider { code: -32000 }));
        }
        opened.session.stop().await.unwrap();
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn maps_terminal_reasons_and_rejects_client_execution_requests() {
    for (mode, expected) in [
        ("max_tokens", PromptOutcome::OutputLimit),
        ("max_turn_requests", PromptOutcome::RequestLimit),
        ("refusal", PromptOutcome::Refused),
        ("cancelled", PromptOutcome::Cancelled),
        ("unknown-request", PromptOutcome::Completed),
    ] {
        let (root, binding) = fixture_binding(mode, 16);
        let opened = binding.open().await.unwrap();
        assert_eq!(
            opened.session.prompt(prompt("test")).await.unwrap(),
            expected
        );
        opened.session.stop().await.unwrap();
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn admission_rejects_invalid_input_without_using_provider() {
    let (root, binding) = fixture_binding("echo", 16);
    let opened = binding.open().await.unwrap();
    for invalid in [
        PromptRequest {
            execution_id: "".into(),
            ..prompt("x")
        },
        PromptRequest {
            input_tokens: 801,
            ..prompt("x")
        },
        PromptRequest {
            reserved_output_tokens: 99,
            ..prompt("x")
        },
    ] {
        assert!(matches!(
            opened.session.prompt(invalid).await,
            Err(BindingError::InvalidInput(_))
        ));
    }
    assert_eq!(
        opened.session.prompt(prompt("valid")).await.unwrap(),
        PromptOutcome::Completed
    );
    opened.session.stop().await.unwrap();
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn prompt_deadline_and_dropped_handles_cleanup() {
    let (root, binding) = fixture_binding("stall", 16);
    let opened = binding.open().await.unwrap();
    assert_eq!(
        opened.session.prompt(prompt("test")).await,
        Err(BindingError::Deadline)
    );
    opened.session.stop().await.unwrap();
    assert_gone(&root, "pid");
    let (root, binding) = fixture_binding("echo", 16);
    let opened = binding.open().await.unwrap();
    drop(opened);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn force_stops_a_term_resistant_parent_and_reaps_its_child() {
    let (root, binding) = fixture_binding("ignore-stop", 16);
    let mut opened = binding.open().await.unwrap();
    let active = start(&opened, "long").await;
    assert_eq!(
        next(&mut opened).await,
        AgentTurnUpdate::Message(MessageChunk::Text("running".into()))
    );
    let cleanup = timeout(Duration::from_secs(3), opened.session.stop())
        .await
        .unwrap()
        .unwrap();
    assert!(cleanup.forced);
    assert_eq!(active.await.unwrap().unwrap(), PromptOutcome::Cancelled);
    assert_gone(&root, "pid");
    assert_gone(&root, "child-pid");
}

#[tokio::test]
async fn idle_binding_failures_are_visible_to_the_event_consumer() {
    let (root, binding) = fixture_binding("idle-config-change", 16);
    let mut opened = binding.open().await.unwrap();
    assert!(matches!(
        opened.events.next().await,
        Err(BindingError::Protocol(_))
    ));
    assert_eq!(opened.events.next().await.unwrap(), None);
    opened.session.stop().await.unwrap();
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn known_completion_wins_a_later_stop_without_rewriting_the_result() {
    let (root, binding) = fixture_binding("echo", 16);
    let mut opened = binding.open().await.unwrap();
    let active = start(&opened, "done").await;
    assert_eq!(
        next(&mut opened).await,
        AgentTurnUpdate::Message(MessageChunk::Text("done".into()))
    );
    opened.session.stop().await.unwrap();
    assert_eq!(active.await.unwrap().unwrap(), PromptOutcome::Completed);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn cancelling_open_cleans_up_a_process_that_never_initializes() {
    let (root, binding) = fixture_binding("startup-stall", 16);
    let opening = tokio::spawn(async move { binding.open().await });
    timeout(Duration::from_secs(2), async {
        while !root.path().join("pid").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    opening.abort();
    let _ = opening.await;
    timeout(Duration::from_secs(2), async {
        let pid: i32 = std::fs::read_to_string(root.path().join("pid"))
            .unwrap()
            .parse()
            .unwrap();
        while unsafe { libc::kill(pid, 0) } == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn dropping_only_the_event_reader_stops_unobservable_execution() {
    let (root, binding) = fixture_binding("stall", 16);
    let mut opened = binding.open().await.unwrap();
    let active = start(&opened, "long").await;
    next(&mut opened).await;
    drop(opened.events);
    assert_eq!(active.await.unwrap(), Err(BindingError::Backpressure));
    opened.session.stop().await.unwrap();
    assert_gone(&root, "pid");
}

#[test]
fn native_configuration_cannot_enable_model_false_tools_or_extended_limits() {
    let (_root, config, model) = fixture_configuration("echo", 16);
    let mut dto = ModelMetadataDto::from(&model);
    dto.tool_use = false;
    let no_tools = ModelMetadata::try_from(dto.clone()).unwrap();
    assert!(matches!(
        ClaudeAcpBinding::new(
            config.clone(),
            &no_tools,
            TokenLimits::new(900, 100).unwrap()
        ),
        Err(BindingError::Configuration(_))
    ));
    let text_only = ClaudeAcpConfig {
        file_tools: false,
        ..config.clone()
    };
    assert!(
        ClaudeAcpBinding::new(text_only, &no_tools, TokenLimits::new(900, 100).unwrap()).is_ok()
    );
    dto.tool_use = true;
    dto.max_context_window_tokens = 1_000_000;
    dto.max_output_tokens = 128_000;
    let model = ModelMetadata::try_from(dto).unwrap();
    for limits in [
        TokenLimits::new(200_001, 64_000).unwrap(),
        TokenLimits::new(200_000, 64_001).unwrap(),
    ] {
        assert!(matches!(
            ClaudeAcpBinding::new(config.clone(), &model, limits),
            Err(BindingError::Configuration(_))
        ));
    }
    assert!(
        ClaudeAcpBinding::new(config, &model, TokenLimits::new(200_000, 64_000).unwrap()).is_ok()
    );
}

#[tokio::test]
async fn unlimited_prompt_survives_a_day_and_still_accepts_stop() {
    let (root, mut config, model) = fixture_configuration("stall", 16);
    config.prompt_timeout = None;
    let binding =
        ClaudeAcpBinding::new(config, &model, TokenLimits::new(900, 100).unwrap()).unwrap();
    let mut opened = binding.open().await.unwrap();
    let active = start(&opened, "long-running").await;
    assert_eq!(
        next(&mut opened).await,
        AgentTurnUpdate::Message(MessageChunk::Text("running".into()))
    );
    // Only advance time after real process startup, so virtual startup deadlines
    // cannot race the operating system launching the fixture.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(24 * 60 * 60)).await;
    tokio::task::yield_now().await;
    tokio::time::resume();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!root.path().join("cancel-observed").exists());
    assert!(!active.is_finished());
    opened.session.stop().await.unwrap();
    assert_eq!(active.await.unwrap().unwrap(), PromptOutcome::Cancelled);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn terminal_delivery_failure_is_reported_by_both_prompt_and_event_reader() {
    let (root, binding) = fixture_binding("echo", 1);
    let mut opened = binding.open().await.unwrap();
    assert_eq!(
        opened.session.prompt(prompt("full")).await,
        Err(BindingError::Backpressure)
    );
    assert_eq!(
        next(&mut opened).await,
        AgentTurnUpdate::Message(MessageChunk::Text("full".into()))
    );
    assert_eq!(opened.events.next().await, Err(BindingError::Backpressure));
    assert_eq!(opened.events.next().await, Ok(None));
    opened.session.stop().await.unwrap();
    assert_gone(&root, "pid");
}

#[test]
fn persistent_permission_configuration_requires_supported_durable_scopes() {
    for decision in [
        PermissionDecision::AllowAlways,
        PermissionDecision::RejectAlways,
    ] {
        let (_root, mut config, model) = fixture_configuration("permission", 16);
        config.permissions = PermissionConfig::new(vec![decision]).unwrap();
        assert!(matches!(
            ClaudeAcpBinding::new(config, &model, TokenLimits::new(900, 100).unwrap()),
            Err(BindingError::Unsupported(_))
        ));
    }
}
