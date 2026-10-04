//! Restoration fingerprints distinguish context settings without retaining credentials.
use super::support::*;
use crate::application::agent_execution::providers::ExecutableUseSnapshot;
use crate::application::agent_execution::sessions::{SessionManager, StorageError};
use crate::domain::agent_execution::sessions::SessionId;
use crate::infrastructure::acp::sessions::StdioMcpServer;
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
    for change in 0..6 {
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
            2 => changed.arguments.push("semantic-setting".into()),
            3 => changed.arguments.swap(0, 1),
            4 => {
                changed.executable = ExecutableUseSnapshot::unmanaged(PathBuf::from(
                    "/nonexistent/different-provider",
                ))
            }
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

/// The MCP server list is attached to each open, like the stand-in grant, and
/// selects no provider context (ADR 344, #391): adding one keeps the identity,
/// and a conversation saved without it restores with it, launching no process
/// to find out. The identity it had under the earlier fingerprint, which did
/// hash the servers, differs — even with none (state table R16).
#[tokio::test]
async fn adding_an_mcp_server_keeps_the_identity_and_restores() {
    let _slot = process_test_slot().await;
    // `stand-ins` mode records the MCP entries each session request carried.
    let (root, config, model) = test_acp_configuration("stand-ins", 16);
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
    let mut changed = config.clone();
    changed.mcp_servers.push(StdioMcpServer {
        name: "nessa".into(),
        command: "/trusted/nessa-mcp".into(),
        args: vec!["--workspace".into(), "/different".into()],
    });
    let changed = provider(changed, &model);
    assert_eq!(changed.identity(), identity);
    assert_ne!(
        changed.previous_identity(),
        provider(config, &model).previous_identity()
    );
    let restored = attached_agent(Arc::new(changed), manager().await.unwrap())
        .await
        .unwrap();
    restored.close(close_action()).await.unwrap();
    // Resumed, with the current set.
    let resumed: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.path().join("mcp-servers-resume")).unwrap(),
    )
    .unwrap();
    assert_eq!(resumed[0]["name"], "nessa");
}

#[test]
fn previous_identity_differs_with_no_mcp_servers() {
    let (_root, config, model) = test_acp_configuration("echo", 16);
    assert!(config.mcp_servers.is_empty());
    let current = provider(config, &model);
    let previous = current.previous_identity();
    assert_ne!(previous, current.identity());
    assert_eq!(previous.name(), current.identity().name());
    assert_eq!(previous.model_id(), current.identity().model_id());
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
fn fingerprint_tracks_workspace_policy_prompt_limits_and_unambiguous_arguments() {
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
    served.mcp_servers.push(StdioMcpServer {
        name: "nessa".into(),
        command: "/trusted/nessa-mcp".into(),
        args: vec!["--workspace".into(), "/different".into()],
    });
    assert_eq!(provider(served, &model).identity(), original);
    let mut left = config.clone();
    let mut right = config;
    left.arguments = vec!["ab".into(), "c".into()];
    right.arguments = vec!["a".into(), "bc".into()];
    assert_ne!(
        provider(left, &model).identity(),
        provider(right, &model).identity()
    );
}
