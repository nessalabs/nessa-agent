use super::*;

/// The shared part of the configuration, with one agent under it.
fn one_agent() -> &'static str {
    r#"{"catalog":"/catalog.json","node":"/node","workspace":"/workspace","claude":{"acpEntry":"/acp.js","model":"configured-model"}}"#
}

#[test]
fn agent_configuration_is_explicit_and_rejects_unknown_provider_switches() {
    let config: AgentConfig = serde_json::from_str(one_agent()).unwrap();
    assert!(!config.tools_enabled);
    let claude = config.runtime(AgentId::Claude).unwrap();
    assert_eq!(claude.output_tokens, 4096);
    assert_eq!(claude.context_tokens, 100000);
    let mut value: serde_json::Value = serde_json::from_str(one_agent()).unwrap();
    value["backend"] = serde_json::json!("test");
    assert!(serde_json::from_value::<AgentConfig>(value).is_err());
    let mut value: serde_json::Value = serde_json::from_str(one_agent()).unwrap();
    value["claude"]["backend"] = serde_json::json!("test");
    assert!(serde_json::from_value::<AgentConfig>(value).is_err());
    assert!(serde_json::from_str::<AgentConfig>(r#"{"model":"configured-model"}"#).is_err());
}

#[test]
fn the_only_configured_agent_needs_no_choosing() {
    let config: AgentConfig = serde_json::from_str(one_agent()).unwrap();
    assert_eq!(config.selected().unwrap(), AgentId::Claude);
}

#[test]
fn a_second_agent_makes_the_choice_between_them_a_thing_to_state() {
    // Answering this by picking the first one would put a person's next
    // conversation on an agent they never named, which is the whole reason the
    // choice exists.
    let mut value: serde_json::Value = serde_json::from_str(one_agent()).unwrap();
    value["codex"] = serde_json::json!({"acpEntry":"/codex.js","model":"configured-model"});
    let config: AgentConfig = serde_json::from_value(value.clone()).unwrap();
    assert!(config.selected().is_err());
    value["selected"] = serde_json::json!("codex");
    let config: AgentConfig = serde_json::from_value(value).unwrap();
    assert_eq!(config.selected().unwrap(), AgentId::Codex);
}

#[test]
fn a_selection_is_never_honoured_past_what_is_configured() {
    for selected in ["codex", "gemini", ""] {
        let mut value: serde_json::Value = serde_json::from_str(one_agent()).unwrap();
        value["selected"] = serde_json::json!(selected);
        let config: AgentConfig = serde_json::from_value(value).unwrap();
        assert!(config.selected().is_err(), "{selected:?}");
    }
}

#[test]
fn a_configuration_naming_no_agent_starts_nothing() {
    let value = r#"{"catalog":"/catalog.json","node":"/node","workspace":"/workspace"}"#;
    let config: AgentConfig = serde_json::from_str(value).unwrap();
    assert!(config.agents().is_empty());
    assert!(config.selected().is_err());
}
