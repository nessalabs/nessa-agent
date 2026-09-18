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

#[test]
fn only_the_arguments_that_are_paths_are_this_machines_to_find() {
    let value = r#"{"catalog":"/catalog.json","workspace":"/workspace","runtimes":{"claude":{"command":"/node","args":["/harness/index.js","--flag","relative/path"],"model":"m","toolsEnabled":true}}}"#;
    let config: AgentsConfig = serde_json::from_str(value).unwrap();
    assert_eq!(
        config.runtime(AgentId::Claude).unwrap().paths(),
        [std::path::PathBuf::from("/harness/index.js")]
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
