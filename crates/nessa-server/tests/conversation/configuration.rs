use super::*;

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
    assert_eq!(codex.command, std::path::Path::new("/usr/local/bin/codex"));
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
        inherited_environment(agent, present)
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

    // Every agent needs the same handful to start at all.
    for agent in [AgentId::Claude, AgentId::Codex, AgentId::Opencode] {
        let shared = inherited(agent);
        for key in ["PATH", "HOME", "USER", "LOGNAME", "TMPDIR"] {
            assert!(shared.contains(&key.to_owned()), "{agent:?}: {shared:?}");
        }
    }
}
