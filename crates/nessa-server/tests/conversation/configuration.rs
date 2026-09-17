use super::*;
#[test]
fn agent_configuration_is_explicit_and_rejects_unknown_provider_switches() {
    let valid = r#"{"catalog":"/catalog.json","node":"/node","acpEntry":"/acp.js","workspace":"/workspace","model":"configured-model"}"#;
    let config: AgentConfig = serde_json::from_str(valid).unwrap();
    assert!(!config.tools_enabled);
    assert_eq!(config.output_tokens, 4096);
    assert_eq!(config.context_tokens, 100000);
    let mut value: serde_json::Value = serde_json::from_str(valid).unwrap();
    value["backend"] = serde_json::json!("test");
    assert!(serde_json::from_value::<AgentConfig>(value).is_err());
    assert!(serde_json::from_str::<AgentConfig>(r#"{"model":"configured-model"}"#).is_err());
}
