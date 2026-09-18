use super::*;

/// A bundle holding every file `configure` requires, including one harness per
/// agent Nessa knows about: an installed app that shipped without one of them
/// is an installed app that cannot offer it.
fn bundled_runtime(bundle: &Path) {
    for agent in AgentId::ALL {
        let entry = bundle.join(harness_entry(*agent));
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(entry, "fixture").unwrap();
    }
    for name in ["node", "models.json", "nessa-mcp"] {
        std::fs::write(bundle.join(name), "fixture").unwrap();
    }
}

#[test]
fn bundle_configuration_is_relocatable_and_does_not_overwrite_user_settings() {
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("Nessa.app/runtime");
    bundled_runtime(&bundle);
    let data = root.path().join("data");
    nessa_local_storage::create_directory(&data).unwrap();
    let mut settings = RuntimeConfig::default();
    configure(&mut settings, &bundle, &data).unwrap();
    let agent = settings.agent.as_mut().unwrap();
    assert_eq!(agent.workspace, data.join("workspaces/default"));
    assert!(agent.workspace.is_dir());
    // Every bundled agent is configured, and the one a new conversation starts
    // on is stated rather than left to be guessed at between them.
    assert_eq!(agent.agents().len(), AgentId::ALL.len());
    assert_eq!(agent.selected().unwrap(), AgentId::Claude);
    agent.workspace = root.path().join("chosen-workspace");
    agent.claude.as_mut().unwrap().model = "chosen-model".into();
    agent.codex.as_mut().unwrap().model = "chosen-codex-model".into();
    configure(&mut settings, &bundle, &data).unwrap();
    let agent = settings.agent.unwrap();
    assert_eq!(agent.workspace, root.path().join("chosen-workspace"));
    assert_eq!(agent.claude.as_ref().unwrap().model, "chosen-model");
    assert_eq!(agent.codex.as_ref().unwrap().model, "chosen-codex-model");
    assert_eq!(agent.node, bundle.join("node"));
    for id in AgentId::ALL {
        assert_eq!(
            agent.runtime(*id).unwrap().acp_entry,
            bundle.join(harness_entry(*id)),
            "{id:?}"
        );
    }
    assert_eq!(agent.mcp_servers.len(), 1);
    assert_eq!(agent.mcp_servers[0].command, bundle.join("nessa-mcp"));
    assert!(!data.join("config.json").exists());
}

#[test]
fn a_bundle_missing_one_agents_harness_is_not_a_runtime_to_start() {
    // Reported as an incomplete bundle rather than quietly configuring the
    // agents that are there: setup would go on offering the missing one.
    for missing in AgentId::ALL {
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("runtime");
        bundled_runtime(&bundle);
        std::fs::remove_file(bundle.join(harness_entry(*missing))).unwrap();
        let data = root.path().join("data");
        nessa_local_storage::create_directory(&data).unwrap();
        assert!(
            configure(&mut RuntimeConfig::default(), &bundle, &data).is_err(),
            "{missing:?}"
        );
    }
}

#[test]
fn an_installation_that_only_knew_one_agent_keeps_starting_on_it() {
    // The upgrade that adds an agent must not move a running installation onto
    // it. What was the only agent configured stays the one a conversation that
    // names none is created on.
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("runtime");
    bundled_runtime(&bundle);
    let data = root.path().join("data");
    nessa_local_storage::create_directory(&data).unwrap();
    let mut settings = RuntimeConfig {
        agent: Some(AgentConfig {
            catalog: root.path().join("old-models.json"),
            node: root.path().join("old-node"),
            workspace: data.clone(),
            tools_enabled: true,
            mcp_servers: vec![],
            selected: None,
            claude: None,
            codex: Some(AgentRuntime {
                acp_entry: root.path().join("old-codex.js"),
                model: "chosen-model".into(),
                context_tokens: 100_000,
                output_tokens: 4096,
            }),
        }),
        ..RuntimeConfig::default()
    };
    configure(&mut settings, &bundle, &data).unwrap();
    let agent = settings.agent.unwrap();
    assert_eq!(agent.selected().unwrap(), AgentId::Codex);
    assert_eq!(agent.agents().len(), AgentId::ALL.len());
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
    bundled_runtime(&bundle);
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
            workspace: chosen_workspace.clone(),
            tools_enabled: true,
            mcp_servers: vec![],
            selected: None,
            claude: Some(AgentRuntime {
                acp_entry: root.path().join("old-entry.js"),
                model: "chosen-model".into(),
                context_tokens: 100_000,
                output_tokens: 4096,
            }),
            codex: None,
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
