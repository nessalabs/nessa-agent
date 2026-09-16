use super::*;
#[test]
fn bundle_configuration_is_relocatable_and_does_not_overwrite_user_settings() {
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("Nessa.app/runtime");
    std::fs::create_dir_all(
        bundle.join("claude-acp/node_modules/@agentclientprotocol/claude-agent-acp/dist"),
    )
    .unwrap();
    for name in [
        "node",
        "models.json",
        "nessa-mcp",
        "claude-acp/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js",
    ] {
        std::fs::write(bundle.join(name), "fixture").unwrap();
    }
    let data = root.path().join("data");
    nessa_local_storage::create_directory(&data).unwrap();
    let mut settings = RuntimeConfig::default();
    configure(&mut settings, &bundle, &data).unwrap();
    let agent = settings.agent.as_mut().unwrap();
    assert_eq!(agent.workspace, data.join("workspaces/default"));
    assert!(agent.workspace.is_dir());
    agent.workspace = root.path().join("chosen-workspace");
    agent.model = "chosen-model".into();
    configure(&mut settings, &bundle, &data).unwrap();
    let agent = settings.agent.unwrap();
    assert_eq!(agent.workspace, root.path().join("chosen-workspace"));
    assert_eq!(agent.model, "chosen-model");
    assert_eq!(agent.node, bundle.join("node"));
    assert_eq!(agent.mcp_servers.len(), 1);
    assert_eq!(agent.mcp_servers[0].command, bundle.join("nessa-mcp"));
    assert!(!data.join("config.json").exists());
}
#[test]
fn incomplete_runtime_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    assert!(configure(&mut RuntimeConfig::default(), root.path(), root.path()).is_err());
}

#[cfg(unix)]
#[test]
fn default_workspace_rejects_a_symlinked_ancestor() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("runtime");
    std::fs::create_dir_all(
        bundle.join("claude-acp/node_modules/@agentclientprotocol/claude-agent-acp/dist"),
    )
    .unwrap();
    for name in [
        "node",
        "models.json",
        "nessa-mcp",
        "claude-acp/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js",
    ] {
        std::fs::write(bundle.join(name), "fixture").unwrap();
    }
    let data = root.path().join("data");
    nessa_local_storage::create_directory(&data).unwrap();
    let redirected = root.path().join("redirected");
    nessa_local_storage::create_directory(&redirected).unwrap();
    symlink(&redirected, data.join("workspaces")).unwrap();

    assert!(configure(&mut RuntimeConfig::default(), &bundle, &data).is_err());
    assert!(!redirected.join("default").exists());

    let chosen_workspace = root.path().join("chosen-workspace");
    let mut settings = RuntimeConfig {
        agent: Some(AgentConfig {
            catalog: root.path().join("old-models.json"),
            node: root.path().join("old-node"),
            acp_entry: root.path().join("old-entry.js"),
            workspace: chosen_workspace.clone(),
            model: "chosen-model".into(),
            tools_enabled: true,
            mcp_servers: vec![],
            context_tokens: 100_000,
            output_tokens: 4096,
        }),
        ..RuntimeConfig::default()
    };
    configure(&mut settings, &bundle, &data).unwrap();
    assert_eq!(settings.agent.unwrap().workspace, chosen_workspace);
    assert!(!redirected.join("default").exists());
}

#[test]
fn desktop_identity_rejects_configuration_that_names_another_installed_runtime() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("manifest.json"),
        serde_json::json!({"fingerprint":"a".repeat(64)}).to_string(),
    )
    .unwrap();
    let instance = "b4a38c5b-cf70-4d90-9059-7d9a3a51c658";
    let identity = super::runtime_identity(
        root.path(),
        "a".repeat(64),
        "c".repeat(64),
        instance.into(),
        123,
    )
    .unwrap();
    assert_eq!(identity.fingerprint().as_str(), "a".repeat(64));
    assert!(super::runtime_identity(
        root.path(),
        "b".repeat(64),
        "c".repeat(64),
        instance.into(),
        123
    )
    .is_err());
    assert!(super::runtime_identity(
        root.path(),
        "a".repeat(64),
        "invalid".into(),
        instance.into(),
        123
    )
    .is_err());
}
