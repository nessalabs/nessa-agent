//! Restoration fingerprints distinguish context settings without retaining credentials.
use super::support::*;
use crate::application::agent_execution::providers::ExecutableUseSnapshot;
use crate::application::agent_execution::sessions::{SessionManager, StorageError};
use crate::domain::agent_execution::sessions::SessionId;
use crate::infrastructure::acp::sessions::{McpServerList, StdioMcpServer};
use crate::infrastructure::session_storage::RecordStorage;

fn provider(config: AcpConfig, model: &ModelMetadata) -> ClaudeAcpProvider {
    ClaudeAcpProvider::new(
        config,
        model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap()
}

#[tokio::test]
async fn context_changes_reject_restore_before_launch_but_credentials_rotate_without_persistence() {
    let _slot = process_test_slot().await;
    let (root, mut config, model) = test_acp_configuration("identity-credentials", 16);
    config
        .environment
        .insert("CLAUDE_CONFIG_DIR".into(), "/fixture/config-a".into());
    config.environment.insert(
        "ANTHROPIC_BASE_URL".into(),
        "https://fixture.invalid/a".into(),
    );
    config.credential_environment.insert(
        "NESSA_FIXTURE_CREDENTIAL".into(),
        "synthetic-secret-one".into(),
    );
    let original = provider(config.clone(), &model);
    let identity = original.identity();
    assert!(identity.context().starts_with("sha256:"));
    assert_eq!(identity.context().len(), 71);
    let storage_root = tempfile::tempdir().unwrap();
    let storage_path = storage_root.path().join("sessions");
    let storage = Arc::new(RecordStorage::new(&storage_path).unwrap());
    let session_id = SessionId::new("fingerprint").unwrap();
    let manager = || {
        SessionManager::open(
            Some(session_id.clone()),
            storage.clone(),
            std::sync::Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
            ),
        )
    };
    let agent = attached_agent(Arc::new(original), manager().await.unwrap())
        .await
        .unwrap();
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let launches = std::fs::read(root.path().join("launches")).unwrap();
    for change in 0..4 {
        let mut changed = config.clone();
        match change {
            0 => {
                changed
                    .environment
                    .insert("CLAUDE_CONFIG_DIR".into(), "/fixture/config-b".into());
            }
            1 => {
                changed.environment.insert(
                    "ANTHROPIC_BASE_URL".into(),
                    "https://fixture.invalid/b".into(),
                );
            }
            2 => changed.workspace = PathBuf::from("/another/workspace"),
            _ => changed.tools_enabled = false,
        }
        let changed = provider(changed, &model);
        assert_ne!(changed.identity(), identity);
        assert!(matches!(
            attached_agent(Arc::new(changed), manager().await.unwrap()).await,
            Err(error) if matches!(error, AgentError::Storage(StorageError::IdentityMismatch))
        ));
        assert_eq!(
            std::fs::read(root.path().join("launches")).unwrap(),
            launches
        );
    }
    config.credential_environment.insert(
        "NESSA_FIXTURE_CREDENTIAL".into(),
        "synthetic-secret-two".into(),
    );
    let rotated = provider(config, &model);
    assert_eq!(rotated.identity(), identity);
    let restored = attached_agent(Arc::new(rotated), manager().await.unwrap())
        .await
        .unwrap();
    restored.close(close_action()).await.unwrap();
    drop(restored);
    let records = std::fs::read_dir(storage_path)
        .unwrap()
        .map(|entry| std::fs::read(entry.unwrap().path()).unwrap())
        .collect::<Vec<_>>();
    for secret in [
        "synthetic-secret-one",
        "synthetic-secret-two",
        "NESSA_FIXTURE_CREDENTIAL",
        "/fixture/config-a",
        "https://fixture.invalid/a",
    ] {
        assert!(!records.iter().any(|file| file
            .windows(secret.len())
            .any(|bytes| bytes == secret.as_bytes())));
    }
    assert_eq!(
        std::fs::read_to_string(root.path().join("credential-received")).unwrap(),
        "yes"
    );
}

/// A desktop release stages the runtime under a new directory, and the search
/// path is read from the person's shell again: the program, its entry
/// argument and where it finds its tools all move, and nothing about the
/// conversation does. The identity stays, and the saved session is resumed by
/// the moved program (#1071). The launch fingerprint, which keys a warm-up,
/// moves with it.
#[tokio::test]
async fn moving_the_runtime_keeps_the_identity_and_restores() {
    let _slot = process_test_slot().await;
    let (root, mut config, model) = test_acp_configuration("echo", 16);
    config
        .runtime_environment
        .insert("PATH".into(), "/usr/bin:/bin".into());
    config
        .runtime_environment
        .insert("TMPDIR".into(), "/tmp/before".into());
    let original = provider(config.clone(), &model);
    let identity = original.identity();
    let launched = original.launch_fingerprint();
    let storage_root = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(storage_root.path().join("sessions")).unwrap());
    let session_id = SessionId::new("moved-runtime").unwrap();
    let manager = || {
        SessionManager::open(
            Some(session_id.clone()),
            storage.clone(),
            std::sync::Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
            ),
        )
    };
    let agent = attached_agent(Arc::new(original), manager().await.unwrap())
        .await
        .unwrap();
    agent.close(close_action()).await.unwrap();
    drop(agent);

    // The release's new tree: the same handler and interpreter, elsewhere.
    let release = tempfile::tempdir().unwrap();
    let entry = release.path().join("claude_acp_test_handler.py");
    std::fs::copy(&config.arguments[0], &entry).unwrap();
    let interpreter = release.path().join("python3");
    std::os::unix::fs::symlink(config.executable.executable(), &interpreter).unwrap();
    let mut moved = config;
    moved.executable = ExecutableUseSnapshot::unmanaged(interpreter);
    moved.arguments[0] = entry.into_os_string();
    moved
        .runtime_environment
        .insert("PATH".into(), "/opt/homebrew/bin:/usr/bin:/bin".into());
    moved
        .runtime_environment
        .insert("TMPDIR".into(), "/tmp/after".into());
    let moved = provider(moved, &model);
    assert_eq!(moved.identity(), identity);
    assert_ne!(moved.launch_fingerprint(), launched);
    let restored = attached_agent(Arc::new(moved), manager().await.unwrap())
        .await
        .unwrap();
    restored.close(close_action()).await.unwrap();
    // Resumed, not refused and not opened afresh.
    assert!(root.path().join("resume-observed").exists());
}

#[test]
fn runtime_and_context_keys_must_be_disjoint() {
    let (_root, mut config, model) = test_acp_configuration("echo", 16);
    config.environment.insert("PATH".into(), "/context".into());
    config
        .runtime_environment
        .insert("PATH".into(), "/runtime".into());
    assert!(matches!(
        ClaudeAcpProvider::new(
            config,
            &model,
            TokenLimits::new(900, 100).unwrap(),
            Arc::new(RecordingAudit::default())
        ),
        Err(AgentError::Configuration(_))
    ));
}

fn nessa_server(workspace: &str) -> StdioMcpServer {
    StdioMcpServer {
        name: "nessa".into(),
        command: "/trusted/nessa-mcp".into(),
        args: vec!["--workspace".into(), workspace.into()],
    }
}

/// Saves a conversation whose provider is given `saved`, then restores it with
/// `current`: the identity must be the same, and the restore must resume the
/// saved session with `current` as its server list.
async fn mcp_server_change_keeps_the_identity_and_restores(
    saved: Vec<StdioMcpServer>,
    current: Vec<StdioMcpServer>,
) {
    let _slot = process_test_slot().await;
    // `stand-ins` mode records the MCP entries each session request carried.
    let (root, mut config, model) = test_acp_configuration("stand-ins", 16);
    config.mcp_servers = McpServerList::fixed(saved);
    let original = provider(config.clone(), &model);
    let identity = original.identity();
    let storage_root = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(storage_root.path().join("sessions")).unwrap());
    let session_id = SessionId::new("mcp-servers").unwrap();
    let manager = || {
        SessionManager::open(
            Some(session_id.clone()),
            storage.clone(),
            std::sync::Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
            ),
        )
    };
    let agent = attached_agent(Arc::new(original), manager().await.unwrap())
        .await
        .unwrap();
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let mut changed = config;
    changed.mcp_servers = McpServerList::fixed(current.clone());
    let changed = provider(changed, &model);
    assert_eq!(changed.identity(), identity);
    let restored = attached_agent(Arc::new(changed), manager().await.unwrap())
        .await
        .unwrap();
    restored.close(close_action()).await.unwrap();
    // Resumed, not refused, with the current set.
    let resumed: Vec<serde_json::Value> = serde_json::from_str(
        &std::fs::read_to_string(root.path().join("mcp-servers-resume")).unwrap(),
    )
    .unwrap();
    assert_eq!(resumed.len(), current.len());
    for (sent, server) in resumed.iter().zip(&current) {
        assert_eq!(sent["name"], server.name.as_str());
        assert_eq!(sent["command"], serde_json::json!(server.command));
        assert_eq!(sent["args"], serde_json::json!(server.args));
    }
}

/// The MCP server list is attached to each open, like the stand-in grant, and
/// selects no provider context (ADR 344, #391): adding one keeps the identity,
/// and a conversation saved without it resumes with it.
#[tokio::test]
async fn adding_an_mcp_server_keeps_the_identity_and_restores() {
    mcp_server_change_keeps_the_identity_and_restores(Vec::new(), vec![nessa_server("/different")])
        .await;
}

/// Editing a server's arguments keeps the identity, and the conversation
/// resumes with the edited server.
#[tokio::test]
async fn editing_an_mcp_server_keeps_the_identity_and_restores() {
    mcp_server_change_keeps_the_identity_and_restores(
        vec![nessa_server("/saved")],
        vec![nessa_server("/edited")],
    )
    .await;
}

/// Changing only a server's command keeps the identity, and the conversation
/// resumes with the new command. A stand-in's command is the gateway's
/// executable, so this is the gateway moving.
#[tokio::test]
async fn moving_an_mcp_servers_command_keeps_the_identity_and_restores() {
    let moved = StdioMcpServer {
        command: "/moved/nessa-mcp".into(),
        ..nessa_server("/saved")
    };
    mcp_server_change_keeps_the_identity_and_restores(vec![nessa_server("/saved")], vec![moved])
        .await;
}

/// Removing a server keeps the identity, and the conversation resumes with no
/// servers.
#[tokio::test]
async fn removing_an_mcp_server_keeps_the_identity_and_restores() {
    mcp_server_change_keeps_the_identity_and_restores(vec![nessa_server("/saved")], Vec::new())
        .await;
}

#[test]
fn credential_and_context_keys_must_be_disjoint() {
    let (_root, mut config, model) = test_acp_configuration("echo", 16);
    config.environment.insert("SAME".into(), "context".into());
    config
        .credential_environment
        .insert("SAME".into(), "synthetic-secret".into());
    assert!(matches!(
        ClaudeAcpProvider::new(
            config,
            &model,
            TokenLimits::new(900, 100).unwrap(),
            Arc::new(RecordingAudit::default())
        ),
        Err(AgentError::Configuration(_))
    ));
}

/// Grants that give every open the token named.
struct TokenGrants(&'static str);
impl crate::infrastructure::acp::sessions::StandInGrants for TokenGrants {
    fn grant(
        &self,
        _: &crate::domain::agent_execution::sessions::SessionId,
    ) -> crate::infrastructure::acp::sessions::StandInGrant {
        crate::infrastructure::acp::sessions::StandInGrant::new(
            vec![("NESSA_MCP_SESSION".into(), self.0.into())],
            Box::new(()),
        )
    }
}

#[test]
fn fingerprint_tracks_workspace_policy_prompt_limits_and_context_environment() {
    let (_root, config, model) = test_acp_configuration("echo", 16);
    let original = provider(config.clone(), &model).identity();
    let mut changed = config.clone();
    changed.workspace = PathBuf::from("/another/workspace");
    assert_ne!(provider(changed, &model).identity(), original);
    let mut changed = config.clone();
    changed.permissions = PermissionOfferPolicy::new(vec![PermissionDecision::new(
        PermissionEffect::Deny,
        PermissionScope::request(),
    )])
    .unwrap();
    assert_ne!(provider(changed, &model).identity(), original);
    let instructions = SystemPromptBuilder::new()
        .text(
            PromptSource::new(PromptSourceKind::Core, "host").unwrap(),
            "Keep the selected context.",
        )
        .build()
        .unwrap();
    assert_ne!(
        provider(config.clone(), &model)
            .with_system_prompt(instructions)
            .identity(),
        original
    );
    assert_ne!(
        ClaudeAcpProvider::new(
            config.clone(),
            &model,
            TokenLimits::new(800, 90).unwrap(),
            Arc::new(RecordingAudit::default())
        )
        .unwrap()
        .identity(),
        original
    );
    // A host's grants for its MCP stand-ins are not context: a fresh token
    // each open never changes what a session resumes as.
    for token in ["first", "second"] {
        let (opened, _grant) = crate::infrastructure::acp::sessions::StandInSessions::granted_by(
            Arc::new(TokenGrants(token)),
        )
        .opened(Some(
            &crate::domain::agent_execution::sessions::SessionId::new("conversation").unwrap(),
        ));
        assert_eq!(opened.environment().len(), 1);
        let mut granted = config.clone();
        granted.stand_ins = opened;
        assert_eq!(provider(granted, &model).identity(), original);
    }
    // Nor is the MCP server list (ADR 344, #391).
    let mut served = config.clone();
    served.mcp_servers = McpServerList::fixed(vec![StdioMcpServer {
        name: "nessa".into(),
        command: "/trusted/nessa-mcp".into(),
        args: vec!["--workspace".into(), "/different".into()],
    }]);
    assert_eq!(provider(served, &model).identity(), original);
    let mut configured = config.clone();
    configured
        .environment
        .insert("CODEX_HOME".into(), "/another/home".into());
    assert_ne!(provider(configured, &model).identity(), original);
    // Nor what says which program runs and how it finds its tools; those move
    // the launch fingerprint instead, whose fields stay unambiguous.
    let mut left = config.clone();
    let mut right = config.clone();
    left.arguments = vec!["ab".into(), "c".into()];
    right.arguments = vec!["a".into(), "bc".into()];
    assert_eq!(provider(left.clone(), &model).identity(), original);
    assert_eq!(provider(right.clone(), &model).identity(), original);
    assert_ne!(
        provider(left, &model).launch_fingerprint(),
        provider(right, &model).launch_fingerprint()
    );
    let mut context = config.clone();
    let mut runtime = config;
    context.environment.insert("KEY".into(), "value".into());
    runtime
        .runtime_environment
        .insert("KEY".into(), "value".into());
    assert_eq!(provider(runtime.clone(), &model).identity(), original);
    assert_ne!(
        provider(context, &model).launch_fingerprint(),
        provider(runtime, &model).launch_fingerprint()
    );
}
