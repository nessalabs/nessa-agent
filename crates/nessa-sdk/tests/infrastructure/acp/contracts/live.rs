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
