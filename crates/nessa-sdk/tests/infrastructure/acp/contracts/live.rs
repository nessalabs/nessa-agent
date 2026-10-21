//! Opt-in live-provider check of Nessa's complete Agent restoration path.
use super::support::*;
use crate::{
    application::{agent_execution::sessions::SessionManager, dto::ModelMetadataDto},
    domain::agent_execution::sessions::SessionId,
    infrastructure::acp::sessions::AcpConfig,
    infrastructure::session_storage::RecordStorage,
};
use std::os::unix::fs::PermissionsExt;

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
    config.runtime_environment.clear();
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
                Submitter::owner(close_action())
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
                Submitter::owner(close_action())
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
                Submitter::owner(close_action())
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
                Submitter::owner(close_action())
            )
            .await,
        Ok(ExecutionOutcome::Completed)
    );
    restored.close(close_action()).await.unwrap();
    drop(workspace);
}

/// Project command hooks are on disk in the scratch workspace. The user command
/// hook is a settings file in that same workspace, not a file in the login
/// directory.
///
/// `ClaudeAcpProvider` cannot plant that user hook through `--settings`. The
/// profile hardcodes `disableAllHooks: true` and `settingSources: []`, and the
/// ACP launch has no CLI `--settings` argument. Writing the hook into the real
/// login directory would mutate it, so this test does not. `CLAUDE_CONFIG_DIR`
/// is that login directory (`CLAUDE_CONFIG_DIR`, or `$HOME/.claude`). Credential
/// files are not copied.
///
/// The positive control runs the raw `claude` CLI inside the scratch workspace,
/// so both the project settings and the `--settings` user hook are loaded. The
/// log must contain a line that is exactly `project` and a line that is exactly
/// `user`. It is then truncated.
///
/// Only the project hook is a negative over ACP. The user hook is loaded
/// through `--settings`, and the ACP launch has no such argument, so a quiet
/// log does not show that a user hook was suppressed. The ACP open, prompt,
/// restore, and second prompt must leave the log empty. The test is ignored
/// because it needs the real `claude-agent-acp`, the `claude` CLI, and a
/// Claude login that is not on this machine.
#[tokio::test]
#[ignore = "requires NESSA_LIVE_CLAUDE_ACP and existing local Claude authentication"]
async fn claude_project_and_user_hooks_stay_quiet_across_prompt_and_restore() {
    let _process_slot = process_test_slot().await;
    let (workspace, config, fixture_model) = test_acp_configuration("live-hooks", 32);
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
    let login_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map_or_else(
        || {
            let mut home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
            home.push(".claude");
            home
        },
        PathBuf::from,
    );
    config
        .environment
        .insert("CLAUDE_CONFIG_DIR".into(), login_dir.as_os_str().to_owned());
    let log = workspace.path().join("hook-log");
    std::fs::write(&log, "").unwrap();
    let project_hook = workspace.path().join("project-hook.sh");
    let user_hook = workspace.path().join("user-hook.sh");
    write_append_hook(&project_hook, &log, "project");
    write_append_hook(&user_hook, &log, "user");
    std::fs::create_dir_all(workspace.path().join(".claude")).unwrap();
    std::fs::write(
        workspace.path().join(".claude/settings.json"),
        hook_settings(&project_hook),
    )
    .unwrap();
    let user_settings = workspace.path().join("user-settings.json");
    std::fs::write(&user_settings, hook_settings(&user_hook)).unwrap();
    let control = std::process::Command::new("timeout")
        .arg("180")
        .arg("claude")
        .current_dir(workspace.path())
        .stdin(std::process::Stdio::null())
        .env_remove("ANTHROPIC_API_KEY")
        .env("CLAUDE_CONFIG_DIR", &login_dir)
        .args([
            "-p",
            "Reply with OK only. Do not use tools.",
            "--model",
            "claude-haiku-4-5-20251001",
            "--output-format",
            "json",
            "--dangerously-skip-permissions",
            "--settings",
        ])
        .arg(&user_settings)
        .status()
        .expect("raw claude user-hook control");
    assert!(
        control.success(),
        "raw claude user-hook control exited {control}"
    );
    let control_log = std::fs::read_to_string(&log).unwrap();
    assert!(
        control_log.lines().any(|line| line == "project"),
        "control log has no line that is exactly project"
    );
    assert!(
        control_log.lines().any(|line| line == "user"),
        "control log has no line that is exactly user"
    );
    std::fs::write(&log, "").unwrap();

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
    let local_id = SessionId::new("live-hook-suppression").unwrap();
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
    assert_eq!(
        agent
            .invoke(
                prompt("Reply with OK only. Do not use tools."),
                Submitter::owner(close_action())
            )
            .await,
        Ok(ExecutionOutcome::Completed)
    );
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
    assert_eq!(
        restored
            .invoke(
                prompt("Reply with READY only. Do not use tools."),
                Submitter::owner(close_action())
            )
            .await,
        Ok(ExecutionOutcome::Completed)
    );
    restored.close(close_action()).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "",
        "the project hook appended during open, prompt, or restore"
    );
}

fn write_append_hook(path: &std::path::Path, log: &std::path::Path, source: &str) {
    let script = format!(
        "#!/bin/sh\nprintf '{source}\\n' >> '{log}'\ncat >> '{log}'\n",
        source = source,
        log = log.display()
    );
    std::fs::write(path, script).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

fn hook_settings(command: &std::path::Path) -> String {
    let command = command.display().to_string();
    let events = [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "Stop",
        "SessionEnd",
    ];
    let mut hooks = serde_json::Map::new();
    for event in events {
        hooks.insert(
            event.to_string(),
            serde_json::json!([{
                "hooks": [{"type": "command", "command": command}]
            }]),
        );
    }
    serde_json::to_string(&serde_json::json!({"hooks": hooks})).unwrap()
}
