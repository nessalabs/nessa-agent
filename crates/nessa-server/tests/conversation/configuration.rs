use super::*;
// `providers` exists on Unix only, and so does everything that needs a clock
// to call it.
#[cfg(unix)]
use crate::composition::local_auth::SystemClock;
use nessa_sdk::domain::common::value_objects::ImageMediaType;
use std::ffi::OsStr;

/// The shared part of the configuration, with one agent under it.
fn one_agent() -> &'static str {
    r#"{"catalog":"/catalog.json","workspace":"/workspace","runtimes":{"claude":{"command":"/node","args":["/acp.js"],"model":"configured-model","toolsEnabled":true}}}"#
}

#[test]
fn agent_configuration_is_explicit_and_rejects_unknown_provider_switches() {
    let config: AgentsConfig = serde_json::from_str(one_agent()).unwrap();
    let claude = config.runtime(AgentId::Claude).unwrap();
    assert!(claude.tools_enabled);
    // Whether an agent runs its own tools is stated, never defaulted: `false` is
    // the one value Codex cannot start on, so a default would have failed the
    // whole gateway for anyone who left the field out.
    let mut value: serde_json::Value = serde_json::from_str(one_agent()).unwrap();
    value["runtimes"]["claude"]
        .as_object_mut()
        .unwrap()
        .remove("toolsEnabled");
    assert!(serde_json::from_value::<AgentsConfig>(value).is_err());
    assert_eq!(claude.output_tokens, 4096);
    assert_eq!(claude.context_tokens, 100000);
    let mut value: serde_json::Value = serde_json::from_str(one_agent()).unwrap();
    value["backend"] = serde_json::json!("test");
    assert!(serde_json::from_value::<AgentsConfig>(value).is_err());
    let mut value: serde_json::Value = serde_json::from_str(one_agent()).unwrap();
    value["runtimes"]["claude"]["backend"] = serde_json::json!("test");
    assert!(serde_json::from_value::<AgentsConfig>(value).is_err());
    assert!(serde_json::from_str::<AgentsConfig>(r#"{"model":"configured-model"}"#).is_err());
}

#[test]
fn an_agent_is_started_by_a_command_and_its_arguments() {
    // Not a runtime and an entry script: an agent that speaks the protocol
    // itself is its own command with its own subcommand, and that could not be
    // said at all in the narrower shape.
    let value = r#"{"catalog":"/catalog.json","workspace":"/workspace","runtimes":{"codex":{"command":"/usr/local/bin/codex","args":["acp"],"model":"configured-model","toolsEnabled":true}}}"#;
    let config: AgentsConfig = serde_json::from_str(value).unwrap();
    let codex = config.runtime(AgentId::Codex).unwrap();
    assert_eq!(
        codex.command.executable(),
        std::path::Path::new("/usr/local/bin/codex")
    );
    assert_eq!(codex.args, ["acp"]);
    // A subcommand is the agent's own vocabulary, not a file this machine is
    // asked about, so nothing here has to exist for the configuration to be
    // readable.
    assert!(codex.paths().is_empty());
    assert_eq!(config.selected().unwrap(), AgentId::Codex);
}

/// An absolute path as *this* platform spells one.
///
/// What counts as a path this machine has to find is the platform's own
/// question, and `/harness/index.js` is not absolute on Windows — it names no
/// drive. Written as a Unix path, this test asserted that a relative argument
/// was found, which is the opposite of what it is for.
fn absolute(name: &str) -> String {
    if cfg!(windows) {
        format!("C:\\{name}")
    } else {
        format!("/{name}")
    }
}

#[test]
fn only_the_arguments_that_are_paths_are_this_machines_to_find() {
    let entry = absolute("harness/index.js");
    let value = serde_json::json!({
        "catalog": absolute("catalog.json"),
        "workspace": absolute("workspace"),
        "runtimes": {"claude": {
            "command": absolute("node"),
            "args": [&entry, "--flag", "relative/path"],
            "model": "m",
            "toolsEnabled": true,
        }},
    });
    let config: AgentsConfig = serde_json::from_value(value).unwrap();
    assert_eq!(
        config.runtime(AgentId::Claude).unwrap().paths(),
        [std::path::PathBuf::from(entry)]
    );
}

#[test]
fn the_only_configured_agent_needs_no_choosing() {
    let config: AgentsConfig = serde_json::from_str(one_agent()).unwrap();
    assert_eq!(config.selected().unwrap(), AgentId::Claude);
}

#[test]
fn a_second_agent_makes_the_choice_between_them_a_thing_to_state() {
    // Answering this by picking the first one would put a person's next
    // conversation on an agent they never named, which is the whole reason the
    // choice exists.
    let mut value: serde_json::Value = serde_json::from_str(one_agent()).unwrap();
    value["runtimes"]["codex"] = serde_json::json!({"command":"/node","args":["/codex.js"],"model":"configured-model","toolsEnabled":true});
    let config: AgentsConfig = serde_json::from_value(value.clone()).unwrap();
    assert!(config.selected().is_err());
    value["selected"] = serde_json::json!("codex");
    let config: AgentsConfig = serde_json::from_value(value).unwrap();
    assert_eq!(config.selected().unwrap(), AgentId::Codex);
}

#[test]
fn a_selection_is_never_honoured_past_what_is_configured() {
    for selected in ["codex", "gemini", ""] {
        let mut value: serde_json::Value = serde_json::from_str(one_agent()).unwrap();
        value["selected"] = serde_json::json!(selected);
        let config: AgentsConfig = serde_json::from_value(value).unwrap();
        assert!(config.selected().is_err(), "{selected:?}");
    }
}

#[test]
fn an_agent_name_with_no_adapter_is_reported_rather_than_skipped() {
    // Skipping it would leave that agent missing from setup, which reads as an
    // agent that is not installed on a machine where it is.
    let mut value: serde_json::Value = serde_json::from_str(one_agent()).unwrap();
    value["runtimes"]["gemini"] = serde_json::json!({"command":"/node","args":["/gemini.js"],"model":"configured-model","toolsEnabled":true});
    let config: AgentsConfig = serde_json::from_value(value).unwrap();
    assert_eq!(config.unknown(), Some("gemini"));
    let refused = config.selected().unwrap_err().to_string();
    assert!(refused.contains("gemini"), "{refused}");
}

#[test]
fn a_configuration_naming_no_agent_starts_nothing() {
    let value = r#"{"catalog":"/catalog.json","workspace":"/workspace"}"#;
    let config: AgentsConfig = serde_json::from_str(value).unwrap();
    assert!(config.agents().is_empty());
    assert!(config.selected().is_err());
}

#[test]
fn whether_an_agent_runs_its_own_tools_is_asked_of_that_agent_alone() {
    // Shared, one operator turning tools off for Claude would take the server
    // down over Codex, which has no text-only mode and refuses to be built
    // without them — an agent they were not configuring at all.
    let value = r#"{"catalog":"/catalog.json","workspace":"/workspace","selected":"claude","runtimes":{
        "claude":{"command":"/node","args":["/acp.js"],"model":"m","toolsEnabled":false},
        "codex":{"command":"/codex","args":["acp"],"model":"m","toolsEnabled":true}}}"#;
    let config: AgentsConfig = serde_json::from_str(value).unwrap();
    assert!(!config.runtime(AgentId::Claude).unwrap().tools_enabled);
    assert!(config.runtime(AgentId::Codex).unwrap().tools_enabled);
}

#[test]
fn no_agent_is_told_how_to_sign_itself_in() {
    // Codex's adapter will sign itself in from an environment key if the launch
    // names `DEFAULT_AUTH_REQUEST`, which is the obvious way to make an
    // environment-only key work. It does it by *logging in*: the key is written
    // in plaintext into the user's own `auth.json` under `CODEX_HOME`, where it
    // outlives the variable and is then preferred over it. Rotating the
    // variable afterwards leaves the agent sending the old key while this
    // server reports it ready.
    //
    // A gateway starting an agent must not move the operator's credential onto
    // the user's disk, so nothing here names a sign-in method. Checked against
    // the pinned adapter rather than reasoned about, including that the
    // `ephemeral` credential store does not avoid the write.
    for agent in AgentId::ALL {
        let named: Vec<_> = launch_environment(*agent).into_keys().collect();
        assert!(
            !named.contains(&OsString::from("DEFAULT_AUTH_REQUEST")),
            "{}: a launch that signs the agent in writes the key to the user's disk",
            agent.name(),
        );
    }
}

#[test]
fn each_agent_inherits_the_directory_variables_it_resolves_its_own_configuration_from() {
    // Every name asked for is answered, so what the assertions see is the key
    // selection rather than whatever this machine happens to have set.
    let present = |key: &str| Some(OsString::from(format!("/fixture/{key}")));
    let inherited = |agent| -> Vec<String> {
        inherited_environment(agent, None, present)
            .keys()
            .map(|key| key.to_string_lossy().into_owned())
            .collect()
    };

    let opencode = inherited(AgentId::Opencode);
    // Opencode resolves config, data, cache and state from the XDG variables,
    // and with them its providers, its plugins and the account a person signed
    // in on. Under `env_clear` leaving one out is not "unset": Opencode falls
    // back to a path under `HOME` and reads a different installation than the
    // readiness probe answered about.
    for key in [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ] {
        assert!(opencode.contains(&key.to_owned()), "{opencode:?}");
    }
    // And is handed no pointer into another vendor's configuration.
    for key in ["CLAUDE_CONFIG_DIR", "CODEX_HOME"] {
        assert!(!opencode.contains(&key.to_owned()), "{opencode:?}");
    }

    let claude = inherited(AgentId::Claude);
    assert!(
        claude.contains(&"CLAUDE_CONFIG_DIR".to_owned()),
        "{claude:?}"
    );
    let codex = inherited(AgentId::Codex);
    assert!(codex.contains(&"CODEX_HOME".to_owned()), "{codex:?}");
    // The XDG variables are general-purpose, so they are Opencode's only by
    // virtue of being what Opencode reads. An agent with a directory variable
    // of its own has no business being given them as well.
    for other in [claude, codex] {
        assert!(!other.contains(&"XDG_CONFIG_HOME".to_owned()), "{other:?}");
    }

    // Every agent needs the same handful to start at all. `PATH` is not among
    // them because it is not inherited: `agent_search_path` decides it and
    // `the_agent_is_launched_with_the_path_that_rule_chose` is what holds that
    // end. Passing `None` above is what makes this the inherited set alone.
    for agent in [AgentId::Claude, AgentId::Codex, AgentId::Opencode] {
        let shared = inherited(agent);
        for key in ["HOME", "USER", "LOGNAME", "TMPDIR"] {
            assert!(shared.contains(&key.to_owned()), "{agent:?}: {shared:?}");
        }
        assert!(
            !shared.contains(&"PATH".to_owned()),
            "{agent:?}: {shared:?}"
        );
    }
}

/// A model catalog entry a provider can be built on, for one vendor.
#[cfg(unix)]
fn catalog_entry(provider: &str, model: &str) -> serde_json::Value {
    serde_json::json!({
        "provider": provider,
        "modelId": model,
        "displayName": model,
        "input": {"text": true, "image": false, "audio": false},
        "output": {"text": true, "image": false, "audio": false},
        "toolUse": true,
        "reasoning": false,
        "maxContextWindowTokens": 128000,
        "maxOutputTokens": 32000,
        "knowledgeCutoff": "2026-04-30",
        "documentationUrl": "https://example.invalid/model",
    })
}

/// Two agents configured, one of which cannot be built, and the workspace and
/// catalog they are pointed at. `missing` names the agent whose command is not
/// written to disk, which is what `build::provider` refuses on.
#[cfg(unix)]
fn two_agents(root: &Path, selected: &str, missing: AgentId) -> (AgentsConfig, std::path::PathBuf) {
    let catalog = root.join("catalog.json");
    std::fs::write(
        &catalog,
        serde_json::json!({
            "verifiedOn": "2026-09-11",
            "models": [
                catalog_entry("anthropic", "configured-model"),
                catalog_entry("opencode", "configured-model"),
            ],
        })
        .to_string(),
    )
    .unwrap();
    let workspace = root.join("workspace");
    nessa_local_storage::create_directory(&workspace).unwrap();
    let mut runtimes = serde_json::Map::new();
    for agent in [AgentId::Claude, AgentId::Opencode] {
        let command = root.join(agent.name());
        if agent != missing {
            std::fs::write(&command, "fixture").unwrap();
        }
        runtimes.insert(
            agent.name().to_owned(),
            serde_json::json!({
                "command": command,
                "args": [],
                "model": "configured-model",
                "toolsEnabled": true,
            }),
        );
    }
    let config = serde_json::from_value(serde_json::json!({
        "catalog": catalog,
        "workspace": workspace,
        "selected": selected,
        "runtimes": runtimes,
    }))
    .unwrap();
    (config, root.join("conversations"))
}

/// One agent that cannot be built does not take the others down with it.
///
/// What `build::provider` refuses on is local to one agent — a command that is
/// not there, a model its vendor does not serve — and none of it says anything
/// about the rest. Before this, a person with Claude installed and Opencode
/// merely configured got a gateway that would not start, with a message about
/// Opencode and no way to reach the page that would have installed it.
// `providers` is Unix-only; on other platforms it refuses outright.
#[cfg(unix)]
#[test]
fn an_agent_that_cannot_be_built_does_not_take_the_others_with_it() {
    let root = tempfile::tempdir().unwrap();
    let (config, conversations) = two_agents(root.path(), "claude", AgentId::Opencode);
    nessa_local_storage::create_directory(&conversations).unwrap();

    let built = providers(
        &config,
        &conversations,
        Arc::new(SystemClock),
        Arc::new(NoImages),
    )
    .unwrap();
    // Absent rather than present-and-broken: the conversation service answers a
    // missing agent with `AgentNotConfigured`, which reaches the one client that
    // asked for it and leaves every other conversation alone.
    assert!(built.providers.contains_key(&AgentId::Claude));
    assert!(!built.providers.contains_key(&AgentId::Opencode));
}

/// And Opencode is a provider composition can actually build, which nothing
/// asserted before: every other test on this path stops at the configuration.
// `providers` is Unix-only; on other platforms it refuses outright.
#[cfg(unix)]
#[test]
fn every_configured_agent_that_can_be_built_is() {
    let root = tempfile::tempdir().unwrap();
    // No agent left out, so nothing is missing: `AgentId::Claude` names an
    // agent that is configured here, which is what makes this the both-built
    // case rather than a second copy of the one above.
    let (config, conversations) = two_agents(root.path(), "claude", AgentId::Claude);
    std::fs::write(root.path().join(AgentId::Claude.name()), "fixture").unwrap();
    nessa_local_storage::create_directory(&conversations).unwrap();

    let built = providers(
        &config,
        &conversations,
        Arc::new(SystemClock),
        Arc::new(NoImages),
    )
    .unwrap();
    assert_eq!(built.providers.len(), 2);
    assert!(built.providers.contains_key(&AgentId::Opencode));
}

/// Opencode can be started against the catalog Nessa actually ships.
///
/// Every other test on this path writes its own catalog, so none of them could
/// tell "composition builds an Opencode provider" from "composition builds one
/// when handed a catalog invented for the test". This one hands it the shipped
/// file and the model the desktop starts Opencode on, which is the pair a real
/// installation has.
// `providers` is Unix-only; on other platforms it refuses outright.
#[cfg(unix)]
#[test]
fn opencode_builds_against_the_catalog_nessa_ships() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    nessa_local_storage::create_directory(&workspace).unwrap();
    let command = root.path().join("opencode");
    std::fs::write(&command, "fixture").unwrap();
    let conversations = root.path().join("conversations");
    nessa_local_storage::create_directory(&conversations).unwrap();

    let config: AgentsConfig = serde_json::from_value(serde_json::json!({
        "catalog": concat!(env!("CARGO_MANIFEST_DIR"), "/../nessa-sdk/data/models.json"),
        "workspace": workspace,
        "selected": "opencode",
        "runtimes": {"opencode": {
            "command": command,
            "args": ["acp"],
            // What `composition::desktop::default_model` starts Opencode on.
            "model": "opencode/big-pickle",
            "toolsEnabled": true,
        }},
    }))
    .unwrap();

    // Selected, so a refusal would be fatal rather than logged: this asserts
    // the provider was built, not that the failure was survivable.
    let built = providers(
        &config,
        &conversations,
        Arc::new(SystemClock),
        Arc::new(NoImages),
    )
    .unwrap();
    assert!(built.providers.contains_key(&AgentId::Opencode));
    assert!(built.unavailable.is_empty());
}

/// No Opencode model Nessa ships can be sent an image, and the whole gateway
/// stops keeping them once one is configured.
///
/// Said out loud here because it is invisible everywhere else. The binding
/// declares image input whenever composition supplies a byte source, which is
/// the right declaration; the catalogue is what withholds it. All three
/// `opencode` entries record no `imageInput` limits — `mimo-v2.5-free`
/// declares `input.image` and records none — and `EffectiveCapabilities`
/// offers image input only where the limits are, so the effective modality is
/// text.
///
/// The second half is the one that reaches conversations that have nothing to
/// do with Opencode. `image_limits` fits an upload to the strictest configured
/// model, and a model recording no limits cannot be met by any image at all,
/// so the shared attachment store keeps nothing — for Claude conversations in
/// the same gateway too. Not new with Opencode: the four `openai` entries are
/// the same shape, so a Claude-plus-Codex installation is already here, and
/// the durable repair is a domain that refuses `input.image` without limits
/// rather than an edit to this file.
///
/// So this goes red the day somebody records limits for a Zen model, which is
/// the day the comment on the declaration in `opencode_acp::sessions::binding`
/// needs reading again. That is the point of it.
// `image_limits` reads the catalog through `model`, which is Unix-only here.
#[cfg(unix)]
#[test]
fn no_opencode_model_nessa_ships_can_be_sent_an_image() {
    let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/../nessa-sdk/data/models.json");
    for model in [
        "opencode/nemotron-3-ultra-free",
        "opencode/big-pickle",
        "opencode/mimo-v2.5-free",
    ] {
        let config: AgentsConfig = serde_json::from_value(serde_json::json!({
            "catalog": shipped,
            "workspace": "/workspace",
            "selected": "opencode",
            "runtimes": {"opencode": {
                "command": "/opencode",
                "args": ["acp"],
                "model": model,
                "toolsEnabled": true,
            }},
        }))
        .unwrap();

        assert_eq!(
            image_limits(&config).unwrap(),
            None,
            "{model} records image limits; the binding's docstring says none does"
        );
    }
}

/// The two ways an agent can be left out are told apart, because readiness
/// needs them apart.
///
/// An agent whose command is not on the machine yet is one an install fixes, so
/// the probe has to go on stating it and answering `not-installed`. An agent
/// whose command is right there and which still could not be built failed on
/// something installing does not re-ask — here, a catalog that serves no model
/// under its vendor — and reporting that one `ready` offers a conversation that
/// cannot be opened.
// `providers` is Unix-only; on other platforms it refuses outright.
#[cfg(unix)]
#[test]
fn only_a_failure_an_install_cannot_fix_is_reported_unstartable() {
    let root = tempfile::tempdir().unwrap();
    let (config, conversations) = two_agents(root.path(), "claude", AgentId::Opencode);
    nessa_local_storage::create_directory(&conversations).unwrap();

    let built = providers(
        &config,
        &conversations,
        Arc::new(SystemClock),
        Arc::new(NoImages),
    )
    .unwrap();
    assert!(!built.providers.contains_key(&AgentId::Opencode));
    assert!(
        built.unavailable.is_empty(),
        "an agent that is merely not installed is not unstartable; it is not installed"
    );

    // The same pair, with Opencode's command in place and a catalog that names
    // no model under its vendor. Nothing anyone installs changes that answer.
    let root = tempfile::tempdir().unwrap();
    let (config, conversations) = two_agents(root.path(), "claude", AgentId::Claude);
    std::fs::write(root.path().join(AgentId::Claude.name()), "fixture").unwrap();
    std::fs::write(
        root.path().join("catalog.json"),
        serde_json::json!({
            "verifiedOn": "2026-09-11",
            "models": [catalog_entry("anthropic", "configured-model")],
        })
        .to_string(),
    )
    .unwrap();
    nessa_local_storage::create_directory(&conversations).unwrap();

    let built = providers(
        &config,
        &conversations,
        Arc::new(SystemClock),
        Arc::new(NoImages),
    )
    .unwrap();
    assert!(!built.providers.contains_key(&AgentId::Opencode));
    assert_eq!(
        built.unavailable,
        std::collections::HashSet::from([AgentId::Opencode])
    );
}

/// The agent the installation is set to use is the exception.
///
/// A server that cannot start a conversation on the agent it is set to is not a
/// degraded server, so that one is still fatal — and saying so at startup is
/// the only place it can be said, since the alternative is a gateway that
/// refuses the first conversation anybody opens.
// `providers` is Unix-only; on other platforms it refuses outright.
#[cfg(unix)]
#[test]
fn the_selected_agent_failing_to_build_is_still_fatal() {
    let root = tempfile::tempdir().unwrap();
    let (config, conversations) = two_agents(root.path(), "opencode", AgentId::Opencode);
    nessa_local_storage::create_directory(&conversations).unwrap();

    assert!(providers(
        &config,
        &conversations,
        Arc::new(SystemClock),
        Arc::new(NoImages)
    )
    .is_err());
}

/// The packaged case: the host resolved a path at registration, and that is the
/// one the agent gets — not the service's own, which has none of the user's
/// tools on it.
#[test]
fn the_agent_takes_the_hosts_resolved_path_over_the_services_own() {
    assert_eq!(
        agent_search_path(
            Some("/opt/homebrew/bin:/usr/bin:/bin".into()),
            Some("/usr/bin:/bin:/usr/sbin:/sbin".into()),
        ),
        Some("/opt/homebrew/bin:/usr/bin:/bin".into())
    );
}

/// The developer loop: `just server` is started from a terminal, there is no
/// host to resolve anything, and that terminal's path is already the right one.
#[test]
fn without_a_resolved_path_the_process_keeps_its_own() {
    assert_eq!(
        agent_search_path(None, Some("/Users/me/.cargo/bin:/usr/bin".into())),
        Some("/Users/me/.cargo/bin:/usr/bin".into())
    );
}

/// An empty variable is not a path. Treating it as one gives the agent an empty
/// `PATH`, which searches the working directory it writes to.
#[test]
fn an_empty_variable_is_not_a_path() {
    assert_eq!(
        agent_search_path(Some("".into()), Some("/usr/bin:/bin".into())),
        Some("/usr/bin:/bin".into())
    );
    assert_eq!(agent_search_path(Some("".into()), Some("".into())), None);
    assert_eq!(agent_search_path(None, None), None);
}

/// The rule above decides nothing unless the launched environment uses it. The
/// two were wired together separately, and a merge that kept one and dropped
/// the other would still compile and still pass every test above.
#[test]
fn the_agent_is_launched_with_the_path_that_rule_chose() {
    let launched = inherited_environment(
        AgentId::Codex,
        Some("/opt/homebrew/bin".into()),
        |key: &str| std::env::var_os(key),
    );
    assert_eq!(
        launched.get(OsStr::new("PATH")),
        Some(&OsString::from("/opt/homebrew/bin"))
    );
    // No path to hand over means none is named, rather than an empty one, which
    // would search the working directory the agent writes to.
    assert!(
        !inherited_environment(AgentId::Codex, None, |key: &str| std::env::var_os(key))
            .contains_key(OsStr::new("PATH"))
    );
}

/// One store holds the uploads of conversations that each run on their own
/// agent, so an image has to be one every configured agent would take.
///
/// Fitting to one model and sending to another is a refusal at the moment of
/// sending, which is the one point where there is nothing left to do about it:
/// the bytes are already kept, the message is already written.
#[test]
fn an_image_is_fitted_to_what_every_configured_agent_would_take() {
    let limits = |types: Vec<ImageMediaType>, encoded, max_edge, many, native| {
        ImageInputLimits::new(types, encoded, max_edge, many, native).unwrap()
    };
    let roomy = limits(
        vec![
            ImageMediaType::Png,
            ImageMediaType::Jpeg,
            ImageMediaType::Webp,
        ],
        8_000_000,
        8000,
        2000,
        1600,
    );
    let tight = limits(
        vec![ImageMediaType::Jpeg, ImageMediaType::Png],
        5_000_000,
        4000,
        1500,
        1400,
    );
    let both = narrower(&roomy, &tight).expect("both accept PNG and JPEG");
    // Every figure is the smaller, so the image meets both.
    assert_eq!(both.max_encoded_bytes(), 5_000_000);
    assert_eq!(both.max_edge_px(), 4000);
    assert_eq!(both.many_images_max_edge_px(), 1500);
    assert_eq!(both.native_long_edge_px(), 1400);
    // WebP goes, because one of the two would refuse it; the order is the
    // first's, which is the order the catalog published.
    assert_eq!(
        both.media_types(),
        [ImageMediaType::Png, ImageMediaType::Jpeg]
    );
    // Narrowing is symmetric in what it admits, whichever way round it is asked.
    let other_way = narrower(&tight, &roomy).unwrap();
    assert_eq!(other_way.max_encoded_bytes(), both.max_encoded_bytes());
    assert_eq!(other_way.max_edge_px(), both.max_edge_px());
    // Two models with no encoding in common leave nothing this gateway could
    // store and then send, which is not an error: it is no images.
    assert!(narrower(
        &limits(vec![ImageMediaType::Png], 8_000_000, 8000, 2000, 1600),
        &limits(vec![ImageMediaType::Gif], 8_000_000, 8000, 2000, 1600),
    )
    .is_none());
}

/// An image source none of these tests reads.
///
/// `providers` hands one to every agent it builds; what is asserted above is
/// which agents got built, and nothing here ever prompts, so a source that
/// panics if read is the honest stand-in.
#[cfg(unix)]
struct NoImages;
#[cfg(unix)]
impl nessa_sdk::application::agent_execution::providers::UserImageSource for NoImages {
    fn read(
        &self,
        _: nessa_sdk::domain::agent_execution::prompts::ImageReference,
    ) -> nessa_sdk::application::agent_execution::providers::UserImageFuture<'_> {
        unreachable!("no test here prompts an agent")
    }
}
