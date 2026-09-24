use super::*;
use nessa_sdk::application::agent_execution::providers::ExecutableUseSnapshot;
#[cfg(unix)]
use std::io::Write;

#[cfg(unix)]
use crate::agent_install::{
    application::RuntimeStore,
    domain::{preferred_release, AgentName, PinnedRelease},
    infrastructure::releases_for,
};

fn opencode_runtime(model: &str) -> AgentRuntime {
    AgentRuntime {
        command: ExecutableUseSnapshot::unmanaged(PathBuf::from("/unverified/opencode")),
        args: vec!["old-argument".into()],
        model: model.into(),
        tools_enabled: false,
        context_tokens: 72_000,
        output_tokens: 3072,
    }
}

#[cfg(unix)]
fn opencode_release() -> (AgentName, PinnedRelease) {
    let agent = AgentName::parse(AgentId::Opencode.name()).expect("known agent name");
    let releases = releases_for(&agent).expect("compiled release pins parse");
    let release = preferred_release(releases, &host_platform())
        .expect("Unix desktop test hosts have a pinned OpenCode release");
    (agent, release)
}

#[cfg(unix)]
fn runtime_archive(release: &PinnedRelease) -> Vec<u8> {
    let body = b"test OpenCode runtime";
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let mut header = tar::Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(0o755);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    builder
        .append_data(&mut header, release.launch().as_str(), body.as_slice())
        .expect("append runtime to archive");
    builder
        .into_inner()
        .expect("finish tar archive")
        .finish()
        .expect("finish gzip archive")
}

#[cfg(unix)]
fn publish_runtime(data: &Path, release: &PinnedRelease) -> PathBuf {
    let (agent, _) = opencode_release();
    let store = ManagedRuntimes::new(data.join("agents"));
    let mut staged = store.stage(&agent).expect("stage runtime archive");
    staged
        .file_mut()
        .write_all(&runtime_archive(release))
        .expect("write runtime archive");
    let publication = store
        .publish(&agent, release, &mut staged)
        .expect("publish runtime");
    publication.executable().to_owned()
}

/// A bundle holding every file `configure` requires, including one harness per
/// agent Nessa *bundles*: an installed app that shipped without one of those is
/// an installed app that cannot offer it.
fn bundled_runtime(bundle: &Path) {
    for (_, names) in bundled_agents() {
        for name in names {
            let path = bundle.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "fixture").unwrap();
        }
    }
    for name in ["models.json", "nessa-mcp"] {
        std::fs::write(bundle.join(name), "fixture").unwrap();
    }
}

/// Every agent the desktop ships, with its command and entry script as two
/// bundle-relative names. Not every agent Nessa knows: Opencode is meant to be
/// fetched onto the machine rather than shipped, so a bundle containing it
/// would be one nobody builds.
fn bundled_agents() -> Vec<(AgentId, [String; 2])> {
    AgentId::ALL
        .iter()
        .filter_map(|agent| {
            let (command, entry) = bundled_launch(*agent)?;
            Some((*agent, [command.into(), entry.into()]))
        })
        .collect()
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
    assert_eq!(agents.agents().len(), bundled_agents().len());
    // And nothing was configured for the agent the desktop does not ship.
    assert!(agents.runtime(AgentId::Opencode).is_none());
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
    for (id, [command, entry]) in bundled_agents() {
        let runtime = agents.runtime(id).unwrap();
        assert_eq!(runtime.command.executable(), bundle.join(command), "{id:?}");
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
    for (missing, names) in bundled_agents() {
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("runtime");
        bundled_runtime(&bundle);
        std::fs::remove_file(bundle.join(&names[1])).unwrap();
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
                    command: ExecutableUseSnapshot::unmanaged(root.path().join("old-node")),
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
    assert_eq!(agents.agents().len(), bundled_agents().len());
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
                    command: ExecutableUseSnapshot::unmanaged(root.path().join("old-node")),
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
        command: ExecutableUseSnapshot::unmanaged(root.path().join("old-node")),
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

#[test]
fn removing_an_unverified_selected_runtime_falls_back_to_the_bundled_default() {
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("runtime");
    bundled_runtime(&bundle);
    let data = root.path().join("data");
    nessa_local_storage::create_directory(&data).unwrap();
    let mut settings = RuntimeConfig::default();
    configure(&mut settings, &bundle, &data).unwrap();
    let agents = settings.agents.as_mut().unwrap();
    agents.runtimes.insert(
        AgentId::Opencode.name().into(),
        opencode_runtime("opencode/big-pickle"),
    );
    agents.selected = Some(AgentId::Opencode.name().into());

    configure(&mut settings, &bundle, &data).unwrap();

    let agents = settings.agents.unwrap();
    assert!(agents.runtime(AgentId::Opencode).is_none());
    assert_eq!(agents.selected().unwrap(), DEFAULT_AGENT);
}

#[test]
fn an_unverified_selected_agent_without_a_runtime_entry_still_falls_back() {
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("runtime");
    bundled_runtime(&bundle);
    let data = root.path().join("data");
    nessa_local_storage::create_directory(&data).unwrap();
    let mut settings = RuntimeConfig::default();
    configure(&mut settings, &bundle, &data).unwrap();
    let agents = settings.agents.as_mut().unwrap();
    assert!(agents.runtime(AgentId::Opencode).is_none());
    agents.selected = Some(AgentId::Opencode.name().into());

    configure(&mut settings, &bundle, &data).unwrap();

    let agents = settings.agents.unwrap();
    assert!(agents.runtime(AgentId::Opencode).is_none());
    assert_eq!(agents.selected().unwrap(), DEFAULT_AGENT);
}

#[cfg(unix)]
#[test]
fn a_verified_current_install_becomes_the_exact_desktop_launch() {
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("runtime");
    bundled_runtime(&bundle);
    let data = root.path().join("data");
    nessa_local_storage::create_directory(&data).unwrap();
    let mut settings = RuntimeConfig::default();
    configure(&mut settings, &bundle, &data).unwrap();
    let (agent, release) = opencode_release();
    let releases = releases_for(&agent).expect("compiled release pins parse");
    let other = releases
        .into_iter()
        .find(|candidate| {
            candidate.version() != release.version()
                || candidate.archive_digest() != release.archive_digest()
                || candidate.launch() != release.launch()
        })
        .expect("OpenCode pins more than one artifact");
    let other_path = publish_runtime(&data, &other);
    let selected_path = publish_runtime(&data, &release);
    let agents = settings.agents.as_mut().unwrap();
    agents.runtimes.insert(
        AgentId::Opencode.name().into(),
        opencode_runtime("opencode/minimax-m3"),
    );
    agents.selected = Some(AgentId::Opencode.name().into());

    configure(&mut settings, &bundle, &data).unwrap();

    let agents = settings.agents.unwrap();
    let configured = agents.runtime(AgentId::Opencode).unwrap();
    assert_eq!(configured.command.executable(), selected_path);
    assert_ne!(configured.command.executable(), other_path);
    assert_eq!(configured.args, ["acp"]);
    assert_eq!(configured.model, "opencode/minimax-m3");
    assert!(!configured.tools_enabled);
    assert_eq!(configured.context_tokens, 72_000);
    assert_eq!(configured.output_tokens, 3072);
    assert_eq!(agents.selected().unwrap(), AgentId::Opencode);
}

#[cfg(unix)]
#[test]
fn a_stale_current_install_is_not_injected() {
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("runtime");
    bundled_runtime(&bundle);
    let data = root.path().join("data");
    nessa_local_storage::create_directory(&data).unwrap();
    let mut settings = RuntimeConfig::default();
    configure(&mut settings, &bundle, &data).unwrap();
    let (_, release) = opencode_release();
    let published = publish_runtime(&data, &release);
    std::fs::remove_file(&published).expect("remove installed executable");
    let agents = settings.agents.as_mut().unwrap();
    agents.runtimes.insert(
        AgentId::Opencode.name().into(),
        opencode_runtime("opencode/minimax-m3"),
    );
    agents.selected = Some(AgentId::Opencode.name().into());

    configure(&mut settings, &bundle, &data).unwrap();

    let agents = settings.agents.unwrap();
    assert!(agents.runtime(AgentId::Opencode).is_none());
    assert_eq!(agents.selected().unwrap(), DEFAULT_AGENT);
}

#[cfg(unix)]
#[test]
fn an_install_record_for_another_pin_is_not_injected() {
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("runtime");
    bundled_runtime(&bundle);
    let data = root.path().join("data");
    nessa_local_storage::create_directory(&data).unwrap();
    let mut settings = RuntimeConfig::default();
    configure(&mut settings, &bundle, &data).unwrap();
    let (agent, preferred) = opencode_release();
    let mismatch = releases_for(&agent)
        .expect("compiled release pins parse")
        .into_iter()
        .find(|candidate| {
            candidate.version() != preferred.version()
                || candidate.archive_digest() != preferred.archive_digest()
                || candidate.launch() != preferred.launch()
        })
        .expect("OpenCode pins more than one artifact");
    let mismatched_path = publish_runtime(&data, &mismatch);
    let agents = settings.agents.as_mut().unwrap();
    agents.runtimes.insert(
        AgentId::Opencode.name().into(),
        opencode_runtime("opencode/minimax-m3"),
    );
    agents.selected = Some(AgentId::Opencode.name().into());

    configure(&mut settings, &bundle, &data).unwrap();

    assert!(mismatched_path.is_file());
    let agents = settings.agents.unwrap();
    assert!(agents.runtime(AgentId::Opencode).is_none());
    assert_eq!(agents.selected().unwrap(), DEFAULT_AGENT);
}

#[test]
fn an_unreadable_optional_store_does_not_take_down_bundled_agents() {
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("runtime");
    bundled_runtime(&bundle);
    let data = root.path().join("data");
    nessa_local_storage::create_directory(&data).unwrap();
    // A file where the managed-runtime directory belongs makes every store
    // query fail without relying on host permission enforcement.
    std::fs::write(data.join("agents"), b"not a directory").unwrap();
    let mut settings = RuntimeConfig::default();

    configure(&mut settings, &bundle, &data).unwrap();

    let agents = settings.agents.unwrap();
    assert!(agents.runtime(AgentId::Claude).is_some());
    assert!(agents.runtime(AgentId::Codex).is_some());
    assert!(agents.runtime(AgentId::Opencode).is_none());
    assert_eq!(agents.selected().unwrap(), DEFAULT_AGENT);
}
