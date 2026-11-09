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
    assert_eq!(
        agent
            .invoke(
                prompt("initial-context-work"),
                Submitter::owner(close_action())
            )
            .await,
        Ok(ExecutionOutcome::Completed)
    );
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

    // PATH and the other runtime keys keep the identity. The saved native
    // session is resumed. An endpoint selector is not one of those keys.
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
    let plumbing = provider(plumbing, &model);
    assert_eq!(plumbing.identity(), identity);
    assert_ne!(plumbing.launch_fingerprint(), launch);
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

    // A path composition recorded as the staged runtime is not an argument.
    // The saved identity is unchanged. The fixture is not relaunched with it
    // prepended: that path is not this handler's argv.
    let endpoint_config = config.clone();
    let mut bundled = config;
    bundled
        .staged_runtime_arguments
        .push("/opt/staged/runtime/adapter.js".into());
    let bundled = provider(bundled, &model);
    assert_eq!(bundled.identity(), identity);
    assert_ne!(bundled.launch_fingerprint(), launch);

    // A changed endpoint selects another backend. The saved native session
    // is not resumed; a new one is opened.
    let mut endpoint = endpoint_config.clone();
    endpoint.environment.insert(
        "ANTHROPIC_BASE_URL".into(),
        "https://fixture.invalid/b".into(),
    );
    let endpoint = provider(endpoint, &model);
    assert_ne!(endpoint.identity(), identity);
    assert_eq!(endpoint.launch_fingerprint(), launch);
    let opened = attached_agent(Arc::new(endpoint), manager().await.unwrap())
        .await
        .unwrap();
    assert_ne!(
        opened
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
    opened.close(close_action()).await.unwrap();
    drop(opened);
    assert_eq!(
        std::fs::read_to_string(root.path().join("resume-observed")).unwrap(),
        saved_context
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("new-session-count")).unwrap(),
        "2"
    );

    // A profile argument selects another context. The saved native session
    // is not resumed; a new one is opened.
    let mut profile = endpoint_config;
    profile.arguments.push("--profile".into());
    profile.arguments.push("staging".into());
    let profile = provider(profile, &model);
    assert_ne!(profile.identity(), identity);
    assert_ne!(profile.launch_fingerprint(), launch);
    let opened_profile = attached_agent(Arc::new(profile), manager().await.unwrap())
        .await
        .unwrap();
    assert_ne!(
        opened_profile
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
    opened_profile.close(close_action()).await.unwrap();
    drop(opened_profile);
    assert_eq!(
        std::fs::read_to_string(root.path().join("resume-observed")).unwrap(),
        saved_context
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("new-session-count")).unwrap(),
        "3"
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
        "https://fixture.invalid/b",
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
    assert_eq!(
        agent
            .invoke(
                prompt("initial-context-work"),
                Submitter::owner(close_action())
            )
            .await,
        Ok(ExecutionOutcome::Completed)
    );
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
    let plumbing = provider(plumbing, &model);
    assert_eq!(plumbing.identity(), original);
    assert_ne!(plumbing.launch_fingerprint(), launch);
    // An endpoint or profile selector left in the identity environment
    // selects a saved context. It is not launch plumbing.
    let mut endpoint = config.clone();
    endpoint.environment.insert(
        "ANTHROPIC_BASE_URL".into(),
        "https://fixture.invalid/b".into(),
    );
    let endpoint = provider(endpoint, &model);
    assert_ne!(endpoint.identity(), original);
    assert_eq!(endpoint.launch_fingerprint(), launch);
    let mut profile = config.clone();
    profile
        .environment
        .insert("AWS_PROFILE".into(), "other".into());
    let profile = provider(profile, &model);
    assert_ne!(profile.identity(), original);
    assert_eq!(profile.launch_fingerprint(), launch);
    let mut moved = config.clone();
    moved.executable =
        ExecutableUseSnapshot::unmanaged(PathBuf::from("/nonexistent/moved-runtime"));
    let moved = provider(moved, &model);
    assert_eq!(moved.identity(), original);
    assert_ne!(moved.launch_fingerprint(), launch);
    let mut path = config.clone();
    path.staged_runtime_arguments
        .push("/opt/staged/runtime/adapter.js".into());
    let path = provider(path, &model);
    assert_eq!(path.identity(), original);
    assert_ne!(path.launch_fingerprint(), launch);
    let mut config_path = config.clone();
    config_path.arguments.push("--config".into());
    config_path.arguments.push("/profiles/staging.toml".into());
    let config_path = provider(config_path, &model);
    assert_ne!(config_path.identity(), original);
    assert_ne!(config_path.launch_fingerprint(), launch);
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
    assert_ne!(left.identity(), right.identity());
    assert_ne!(left.launch_fingerprint(), right.launch_fingerprint());
}

#[test]
fn a_semantic_argument_changes_the_restoration_identity() {
    let (_root, config, model) = test_acp_configuration("echo", 16);
    let original = provider(config.clone(), &model);
    let mut profile = config;
    profile.arguments.push("--profile".into());
    profile.arguments.push("staging".into());
    let profile = provider(profile, &model);
    assert_ne!(profile.identity(), original.identity());
    assert_ne!(profile.launch_fingerprint(), original.launch_fingerprint());
}

#[test]
fn a_bundled_runtime_path_does_not_change_the_restoration_identity() {
    let (_root, mut config, model) = test_acp_configuration("echo", 16);
    let original = provider(config.clone(), &model);
    config.executable =
        ExecutableUseSnapshot::unmanaged(PathBuf::from("/nonexistent/moved-runtime"));
    config
        .staged_runtime_arguments
        .push("/opt/staged/runtime/adapter.js".into());
    config
        .runtime_environment
        .insert("PATH".into(), "/opt/homebrew/bin:/usr/bin".into());
    config
        .runtime_environment
        .insert("TMPDIR".into(), "/var/folders/drift".into());
    let moved = provider(config, &model);
    assert_eq!(moved.identity(), original.identity());
    assert_ne!(moved.launch_fingerprint(), original.launch_fingerprint());
}

#[test]
fn a_config_path_changes_the_restoration_identity_and_a_moved_staged_runtime_does_not() {
    let (_root, mut config, model) = test_acp_configuration("echo", 16);
    config
        .staged_runtime_arguments
        .push("/opt/staged/runtime/adapter.js".into());
    config.arguments.push("--config".into());
    config.arguments.push("/profiles/staging.toml".into());
    let original = provider(config.clone(), &model);

    let mut changed = config.clone();
    changed.arguments.pop();
    changed.arguments.push("/profiles/other.toml".into());
    let changed = provider(changed, &model);
    assert_ne!(changed.identity(), original.identity());
    assert_ne!(changed.launch_fingerprint(), original.launch_fingerprint());

    config.executable =
        ExecutableUseSnapshot::unmanaged(PathBuf::from("/nonexistent/moved-runtime"));
    config.staged_runtime_arguments = vec!["/tmp/relocated/0-adapter.js".into()];
    config
        .runtime_environment
        .insert("PATH".into(), "/opt/homebrew/bin:/usr/bin".into());
    config
        .runtime_environment
        .insert("TMPDIR".into(), "/var/folders/drift".into());
    let moved = provider(config, &model);
    assert_eq!(moved.identity(), original.identity());
    assert_ne!(moved.launch_fingerprint(), original.launch_fingerprint());
}
