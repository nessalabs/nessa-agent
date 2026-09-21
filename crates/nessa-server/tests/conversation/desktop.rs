use super::*;

/// A bundle holding every file `configure` requires, including one harness per
/// agent Nessa knows about: an installed app that shipped without one of them
/// is an installed app that cannot offer it.
fn bundled_runtime(bundle: &Path) {
    for agent in AgentId::ALL {
        for name in launch_files(*agent) {
            let path = bundle.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "fixture").unwrap();
        }
    }
    for name in ["models.json", "nessa-mcp"] {
        std::fs::write(bundle.join(name), "fixture").unwrap();
    }
}

/// The command and the entry script, as two bundle-relative names.
fn launch_files(agent: AgentId) -> [String; 2] {
    let (command, entry) = bundled_launch(agent);
    [command.into(), entry.into()]
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
    let agents = settings.agents.as_mut().unwrap();
    assert_eq!(agents.workspace, data.join("workspaces/default"));
    assert!(agents.workspace.is_dir());
    // Every bundled agent is configured, and the one a new conversation starts
    // on is stated rather than left to be guessed at between them.
    assert_eq!(agents.agents().len(), AgentId::ALL.len());
    assert_eq!(agents.selected().unwrap(), AgentId::Claude);
    agents.workspace = root.path().join("chosen-workspace");
    for (name, model) in [("claude", "chosen-model"), ("codex", "chosen-codex-model")] {
        agents.runtimes.get_mut(name).unwrap().model = model.into();
    }
    configure(&mut settings, &bundle, &data).unwrap();
    let agents = settings.agents.unwrap();
    assert_eq!(agents.workspace, root.path().join("chosen-workspace"));
    assert_eq!(agents.runtimes["claude"].model, "chosen-model");
    assert_eq!(agents.runtimes["codex"].model, "chosen-codex-model");
    for id in AgentId::ALL {
        let [command, entry] = launch_files(*id);
        let runtime = agents.runtime(*id).unwrap();
        assert_eq!(runtime.command, bundle.join(command), "{id:?}");
        assert_eq!(runtime.paths(), [bundle.join(entry)], "{id:?}");
    }
    assert_eq!(agents.mcp_servers.len(), 1);
    assert_eq!(agents.mcp_servers[0].command, bundle.join("nessa-mcp"));
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
        std::fs::remove_file(bundle.join(&launch_files(*missing)[1])).unwrap();
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
        agents: Some(AgentsConfig {
            catalog: root.path().join("old-models.json"),
            workspace: data.clone(),
            mcp_servers: vec![],
            selected: None,
            runtimes: HashMap::from([(
                "codex".into(),
                AgentRuntime {
                    command: root.path().join("old-node"),
                    args: vec![root.path().join("old-codex.js").to_string_lossy().into()],
                    model: "chosen-model".into(),
                    tools_enabled: true,
                    context_tokens: 100_000,
                    output_tokens: 4096,
                },
            )]),
        }),
        ..RuntimeConfig::default()
    };
    configure(&mut settings, &bundle, &data).unwrap();
    let agents = settings.agents.unwrap();
    assert_eq!(agents.selected().unwrap(), AgentId::Codex);
    assert_eq!(agents.agents().len(), AgentId::ALL.len());
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
        agents: Some(AgentsConfig {
            catalog: root.path().join("old-models.json"),
            workspace: chosen_workspace.clone(),
            mcp_servers: vec![],
            selected: None,
            runtimes: HashMap::from([(
                "claude".into(),
                AgentRuntime {
                    command: root.path().join("old-node"),
                    args: vec![root.path().join("old-entry.js").to_string_lossy().into()],
                    model: "chosen-model".into(),
                    tools_enabled: true,
                    context_tokens: 100_000,
                    output_tokens: 4096,
                },
            )]),
        }),
        ..RuntimeConfig::default()
    };
    configure(&mut settings, &bundle, &data).unwrap();
    assert_eq!(settings.agents.unwrap().workspace, chosen_workspace);
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

#[test]
fn several_configured_agents_with_no_choice_between_them_is_not_the_desktops_to_guess() {
    // The gateway refuses this configuration and says to name one. The desktop
    // reaching a different answer for the same file — and quietly starting every
    // conversation on Nessa's own default — would send someone's work to a
    // vendor they never picked, on the surface where nobody would ever be told.
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("runtime");
    bundled_runtime(&bundle);
    let data = root.path().join("data");
    nessa_local_storage::create_directory(&data).unwrap();
    let runtime = || AgentRuntime {
        command: root.path().join("old-node"),
        args: vec![root.path().join("old-entry.js").to_string_lossy().into()],
        model: "chosen-model".into(),
        tools_enabled: true,
        context_tokens: 100_000,
        output_tokens: 4096,
    };
    let mut settings = RuntimeConfig {
        agents: Some(AgentsConfig {
            catalog: root.path().join("old-models.json"),
            workspace: data.clone(),
            mcp_servers: vec![],
            selected: None,
            runtimes: HashMap::from([("claude".into(), runtime()), ("codex".into(), runtime())]),
        }),
        ..RuntimeConfig::default()
    };
    configure(&mut settings, &bundle, &data).unwrap();
    let agents = settings.agents.unwrap();
    assert_eq!(agents.selected, None);
    assert!(agents.selected().is_err());
}
