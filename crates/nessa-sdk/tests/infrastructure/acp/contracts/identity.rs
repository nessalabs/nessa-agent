//! Restoration fingerprints distinguish context settings without retaining credentials.
use super::support::*;
use crate::application::agent_execution::providers::ExecutableUseSnapshot;
use crate::application::agent_execution::sessions::SessionManager;
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
async fn a_context_only_mismatch_opens_a_fresh_session_and_credentials_rotate_without_persistence()
{
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
    let launch = original.launch_fingerprint();
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
    let saved_context = agent
        .session_manager()
        .snapshot()
        .await
        .unwrap()
        .provider_context
        .recorded()
        .unwrap()
        .as_str()
        .to_string();
    agent.close(close_action()).await.unwrap();
    drop(agent);

    // A different account directory or tools turned off selects another
    // context. Neither is launched: the fixture asserts the original account
    // directory, and tools off is a different binding. A moved executable is
    // the same restoration identity and is not launched either.
    let launches = std::fs::read(root.path().join("launches")).unwrap();
    let mut account = config.clone();
    account
        .environment
        .insert("CLAUDE_CONFIG_DIR".into(), "/fixture/config-b".into());
    let mut executable = config.clone();
    executable.executable =
        ExecutableUseSnapshot::unmanaged(PathBuf::from("/nonexistent/different-provider"));
    let mut tools = config.clone();
    tools.tools_enabled = false;
    for changed in [account, tools] {
        assert_ne!(provider(changed, &model).identity(), identity);
    }
    let moved = provider(executable, &model);
    assert_eq!(moved.identity(), identity);
    assert_ne!(moved.launch_fingerprint(), launch);
    assert_eq!(
        std::fs::read(root.path().join("launches")).unwrap(),
        launches
    );

    // PATH and the other plumbing keys, including an endpoint, keep the
    // identity. The saved native session is resumed.
    let mut plumbing = config.clone();
    for (key, value) in [
        ("PATH", "/opt/homebrew/bin:/usr/bin"),
        ("TMPDIR", "/var/folders/drift"),
        ("USER", "someone"),
        ("LOGNAME", "someone"),
        ("CODEX_PATH", "/opt/codex"),
        ("CLAUDE_CODE_EXECUTABLE", "/opt/claude"),
        ("ANTHROPIC_BASE_URL", "https://fixture.invalid/b"),
    ] {
        plumbing.environment.insert(key.into(), value.into());
    }
    let plumbing = provider(plumbing, &model);
    assert_eq!(plumbing.identity(), identity);
    let resumed = attached_agent(Arc::new(plumbing), manager().await.unwrap())
        .await
        .unwrap();
    assert_eq!(
        resumed
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .provider_context
            .recorded()
            .unwrap()
            .as_str(),
        saved_context
    );
    resumed.close(close_action()).await.unwrap();
    drop(resumed);
    assert_eq!(
        std::fs::read_to_string(root.path().join("resume-observed")).unwrap(),
        saved_context
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("new-session-count")).unwrap(),
        "1"
    );

    config.credential_environment.insert(
        "NESSA_FIXTURE_CREDENTIAL".into(),
        "synthetic-secret-two".into(),
    );
    let rotated = provider(config.clone(), &model);
    assert_eq!(rotated.identity(), identity);
    let restored = attached_agent(Arc::new(rotated), manager().await.unwrap())
        .await
        .unwrap();
    restored.close(close_action()).await.unwrap();
    drop(restored);

    // An argument change is a different launch, not a different context.
    // The saved native session is resumed.
    let mut arguments = config;
    arguments.arguments.push("semantic-setting".into());
    let arguments = provider(arguments, &model);
    assert_eq!(arguments.identity(), identity);
    assert_ne!(arguments.launch_fingerprint(), launch);
    let resumed_arguments = attached_agent(Arc::new(arguments), manager().await.unwrap())
        .await
        .unwrap();
    assert_eq!(
        resumed_arguments
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .provider_context
            .recorded()
            .unwrap()
            .as_str(),
        saved_context
    );
    resumed_arguments.close(close_action()).await.unwrap();
    drop(resumed_arguments);
    assert_eq!(
        std::fs::read_to_string(root.path().join("resume-observed")).unwrap(),
        saved_context
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("new-session-count")).unwrap(),
        "1"
    );

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
fn plumbing_environment_does_not_change_the_fingerprint() {
    let (_root, mut config, model) = test_acp_configuration("echo", 16);
    config
        .environment
        .insert("HOME".into(), "/fixture/home".into());
    config
        .environment
        .insert("CLAUDE_CONFIG_DIR".into(), "/fixture/config-a".into());
    let original_provider = provider(config.clone(), &model);
    let original = original_provider.identity();
    let launch = original_provider.launch_fingerprint();
    let mut plumbing = config.clone();
    for (key, value) in [
        ("PATH", "/opt/homebrew/bin:/usr/bin"),
        ("TMPDIR", "/var/folders/drift"),
        ("USER", "someone"),
        ("LOGNAME", "someone"),
        ("CODEX_PATH", "/opt/codex"),
        ("CLAUDE_CODE_EXECUTABLE", "/opt/claude"),
    ] {
        plumbing
            .runtime_environment
            .insert(key.into(), value.into());
    }
    // A key that is neither the account nor the runtime still does not
    // select a saved context, and it is not part of the launch fingerprint.
    plumbing.environment.insert(
        "ANTHROPIC_BASE_URL".into(),
        "https://fixture.invalid/b".into(),
    );
    let plumbing = provider(plumbing, &model);
    assert_eq!(plumbing.identity(), original);
    assert_ne!(plumbing.launch_fingerprint(), launch);
    let mut moved = config.clone();
    moved.executable =
        ExecutableUseSnapshot::unmanaged(PathBuf::from("/nonexistent/moved-runtime"));
    let moved = provider(moved, &model);
    assert_eq!(moved.identity(), original);
    assert_ne!(moved.launch_fingerprint(), launch);
    let mut arguments = config.clone();
    arguments.arguments.push("semantic-setting".into());
    let arguments = provider(arguments, &model);
    assert_eq!(arguments.identity(), original);
    assert_ne!(arguments.launch_fingerprint(), launch);
    let mut account = config.clone();
    account
        .environment
        .insert("CLAUDE_CONFIG_DIR".into(), "/fixture/config-b".into());
    assert_ne!(provider(account, &model).identity(), original);
    let mut home = config;
    home.environment
        .insert("HOME".into(), "/fixture/other-home".into());
    assert_ne!(provider(home, &model).identity(), original);
}

#[test]
fn fingerprint_tracks_workspace_policy_prompt_and_limits() {
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
    let mut left = config.clone();
    let mut right = config;
    left.arguments = vec!["ab".into(), "c".into()];
    right.arguments = vec!["a".into(), "bc".into()];
    let left = provider(left, &model);
    let right = provider(right, &model);
    assert_eq!(left.identity(), right.identity());
    assert_ne!(left.launch_fingerprint(), right.launch_fingerprint());
}
