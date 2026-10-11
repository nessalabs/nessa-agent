//! Opt-in live-provider check of Nessa's complete Agent restoration path.
use super::support::*;
use crate::{
    application::{agent_execution::sessions::SessionManager, dto::ModelMetadataDto},
    domain::agent_execution::sessions::SessionId,
    infrastructure::acp::sessions::AcpConfig,
    infrastructure::session_storage::RecordStorage,
};

pub(super) fn live_config(
    mut config: AcpConfig,
    executable_variable: &str,
    context_keys: &[&str],
    credential_keys: &[&str],
) -> AcpConfig {
    let executable = std::env::var_os(executable_variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("set {executable_variable} to the pinned adapter executable"));
    config.executable = ExecutableUseSnapshot::unmanaged(executable);
    config.arguments.clear();
    config.environment.clear();
    for key in context_keys {
        if let Some(value) = std::env::var_os(key) {
            config.environment.insert((*key).into(), value);
        }
    }
    for key in credential_keys {
        if let Some(value) = std::env::var_os(key) {
            config.credential_environment.insert((*key).into(), value);
        }
    }
    config.launch_timeout = Duration::from_secs(60);
    config.startup_timeout = Duration::from_secs(90);
    config.execution_timeout = Some(Duration::from_secs(120));
    config.shutdown_grace = Duration::from_secs(3);
    config.kill_timeout = Duration::from_secs(5);
    config.max_frame_bytes = 1_048_576;
    config.max_incoming_frame_bytes = 1_048_576;
    config
}

#[tokio::test]
#[ignore = "requires NESSA_LIVE_CLAUDE_ACP and existing local Claude authentication"]
async fn selected_claude_preset_survives_a_real_agent_close_and_restore() {
    let _process_slot = process_test_slot().await;
    let (workspace, config, fixture_model) = test_acp_configuration("live", 32);
    let config = live_config(
        config,
        "NESSA_LIVE_CLAUDE_ACP",
        &[
            "HOME",
            "PATH",
            "USER",
            "LOGNAME",
            "SHELL",
            "TMPDIR",
            "LANG",
            "CLAUDE_CONFIG_DIR",
        ],
        &["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"],
    );
    let mut metadata = ModelMetadataDto::from(&fixture_model);
    metadata.model_id = "claude-sonnet-5".into();
    let model = ModelMetadata::try_from(metadata).unwrap();
    let provider = Arc::new(
        ClaudeAcpProvider::new(
            config,
            &model,
            TokenLimits::new(900, 100).unwrap(),
            Arc::new(RecordingAudit::default()),
        )
        .unwrap()
        .with_approval_mode(ApprovalMode::Auto)
        .unwrap(),
    );
    let storage_root = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(storage_root.path().join("sessions")).unwrap());
    let local_id = SessionId::new("live-approval-restore").unwrap();
    let agent = attached_agent(
        provider.clone(),
        SessionManager::open(
            Some(local_id.clone()),
            storage.clone(),
            std::sync::Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
            ),
        )
        .await
        .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Auto));
    assert_eq!(
        agent
            .invoke(
                prompt("Reply with OK only. Do not use tools."),
                close_action()
            )
            .await,
        Ok(ExecutionOutcome::Completed)
    );
    let before = agent.session_manager().snapshot().await.unwrap();
    let provider_context = before.provider_context.clone();
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let restored = attached_agent(
        provider,
        SessionManager::open(
            Some(local_id),
            storage,
            std::sync::Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
            ),
        )
        .await
        .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(restored.approval_mode(), Some(ApprovalMode::Auto));
    assert_eq!(
        restored
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .provider_context,
        provider_context
    );
    assert_eq!(
        restored
            .invoke(
                prompt("Reply with READY only. Do not use tools."),
                close_action()
            )
            .await,
        Ok(ExecutionOutcome::Completed)
    );
    restored.close(close_action()).await.unwrap();
    drop(workspace);
}

#[tokio::test]
#[ignore = "requires NESSA_LIVE_CODEX_ACP and existing local Codex authentication"]
async fn selected_codex_preset_survives_a_real_agent_close_and_restore() {
    let _process_slot = process_test_slot().await;
    let (workspace, config, fixture_model) = codex_configuration("live", 32);
    let config = live_config(
        config,
        "NESSA_LIVE_CODEX_ACP",
        &[
            "HOME",
            "PATH",
            "USER",
            "LOGNAME",
            "SHELL",
            "TMPDIR",
            "LANG",
            "CODEX_HOME",
        ],
        &["CODEX_API_KEY", "OPENAI_API_KEY"],
    );
    let mut metadata = ModelMetadataDto::from(&fixture_model);
    metadata.model_id = "gpt-6-sol".into();
    let model = ModelMetadata::try_from(metadata).unwrap();
    let provider = Arc::new(
        CodexAcpProvider::new(
            config,
            &model,
            TokenLimits::new(900, 100).unwrap(),
            Arc::new(RecordingAudit::default()),
        )
        .unwrap(),
    );
    let storage_root = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(storage_root.path().join("sessions")).unwrap());
    let local_id = SessionId::new("live-codex-restore").unwrap();
    let agent = attached_agent(
        provider.clone(),
        SessionManager::open(
            Some(local_id.clone()),
            storage.clone(),
            std::sync::Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
            ),
        )
        .await
        .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Ask));
    assert_eq!(
        agent
            .invoke(
                prompt("Reply with OK only. Do not use tools."),
                close_action()
            )
            .await,
        Ok(ExecutionOutcome::Completed)
    );
    let provider_context = agent
        .session_manager()
        .snapshot()
        .await
        .unwrap()
        .provider_context;
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let restored = attached_agent(
        provider,
        SessionManager::open(
            Some(local_id),
            storage,
            std::sync::Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
            ),
        )
        .await
        .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(restored.approval_mode(), Some(ApprovalMode::Ask));
    assert_eq!(
        restored
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .provider_context,
        provider_context
    );
    assert_eq!(
        restored
            .invoke(
                prompt("Reply with READY only. Do not use tools."),
                close_action()
            )
            .await,
        Ok(ExecutionOutcome::Completed)
    );
    restored.close(close_action()).await.unwrap();
    drop(workspace);
}

/// Drive the pinned Claude adapter through a policy stop, its cancel, and the
/// next prompt.
///
/// A stopping refusal fires only from `decline_review`: a review this binding
/// will not offer. Bash is one of those tools. A permission the host could
/// answer does not stop the turn. Claude's profile stays unknown; this wrapper
/// is the test's proof, not a claim about Claude. Promote Claude only after
/// this passes on a machine that is already logged in.
///
/// Output of 100 tokens is exhausted by the tool-call payload before Bash is
/// requested. A 1_000_000/8_192 model ceiling with 100_000/4_096 limits produced
/// no event within 150s, so the ceilings stay inside the binding's 200_000/64_000
/// and the limits stay near the working preset except for enough output for one
/// tool call.
#[tokio::test]
#[ignore = "requires NESSA_LIVE_CLAUDE_ACP and existing local Claude authentication"]
async fn claude_stop_cancel_and_the_next_prompt() {
    let _process_slot = process_test_slot().await;
    let (workspace, config, fixture_model) = test_acp_configuration("live", 32);
    let mut config = live_config(
        config,
        "NESSA_LIVE_CLAUDE_ACP",
        &[
            "HOME",
            "PATH",
            "USER",
            "LOGNAME",
            "SHELL",
            "TMPDIR",
            "LANG",
            "CLAUDE_CONFIG_DIR",
        ],
        &["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"],
    );
    config.refusal = ConfiguredRefusal::stopping(
        StopKind::EndTurn,
        ApprovalRuleReference::new(
            "refuse-shell",
            "1",
            ActionContext::new("host", "policy", "grant").unwrap(),
        )
        .unwrap(),
    );
    let mut metadata = ModelMetadataDto::from(&fixture_model);
    metadata.model_id = "claude-sonnet-5".into();
    metadata.max_context_window_tokens = 200_000;
    metadata.max_output_tokens = 8_192;
    let model = ModelMetadata::try_from(metadata).unwrap();
    let audit = Arc::new(RecordingAudit::default());
    let provider = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(8_000, 2_048).unwrap(),
        audit.clone(),
    )
    .unwrap();
    let mut opened = super::super::profile_substitution::open_proven_stop(
        &provider,
        audit,
        ProviderOpenRequest::without_startup_control(None),
    )
    .await
    .unwrap_or_else(|error| panic!("proven-stop open failed: {error:?}"));
    let first = start(
        &opened,
        "Use the Bash tool to run echo nessa-stop. Do not use any other tool.",
    )
    .await;
    let mut seen = Vec::new();
    let mut declined = false;
    let mut finished = false;
    for _ in 0..40 {
        let update = match timeout(Duration::from_secs(30), opened.events.next()).await {
            Ok(Ok(Some(event))) => event.into_update(),
            other => panic!("claude produced no event ({other:?}); events so far:\n{seen:?}"),
        };
        seen.push(format!("{update:?}"));
        match &update {
            ExecutionUpdate::ReviewDeclined(notice) => {
                assert_eq!(
                    notice.decline().outcome(),
                    RefusalOutcome::EndTurn,
                    "events so far:\n{seen:?}"
                );
                declined = true;
            }
            ExecutionUpdate::Finished(ExecutionOutcome::Cancelled) if declined => {
                finished = true;
                break;
            }
            ExecutionUpdate::Finished(_) => {
                panic!("claude finished without the stopping refusal; events:\n{seen:?}");
            }
            _ => {}
        }
    }
    assert!(
        declined && finished,
        "claude never declined Bash and finished the stopped turn; events:\n{seen:?}"
    );
    assert_eq!(
        timeout(Duration::from_secs(30), first)
            .await
            .unwrap_or_else(|_| panic!("the stopped prompt was not answered; events:\n{seen:?}"))
            .unwrap(),
        Ok(ExecutionOutcome::Cancelled)
    );
    let second = start(
        &opened,
        "Reply with the single word ready. Do not use tools.",
    )
    .await;
    assert_eq!(
        timeout(Duration::from_secs(120), second)
            .await
            .unwrap_or_else(|_| panic!("the next prompt did not finish; events:\n{seen:?}"))
            .unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    let _ = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await;
    drop(workspace);
}
