//! `config.json` as the gateway reads it at startup.
use super::RuntimeConfig;

/// Keys an older build wrote for agents on other computers (ADR 1417): a
/// file naming them is still read, so a gateway upgraded in place starts.
#[test]
fn a_configuration_naming_retired_remote_keys_is_still_read() {
    let peer = format!("peer:{}", "ab".repeat(32));
    let older = serde_json::json!({
        "sshHosts": ["devbox", "me@build.example"],
        "defaultEnvironment": peer,
        "envServe": { "commands": true, "artifactsIn": true },
        "environmentTools": { "commandHosts": ["devbox"], "denyPrograms": ["rm"] },
        "pool": { "order": [peer], "excluded": [] },
    });
    assert!(RuntimeConfig::parse(older.to_string().as_bytes()).is_ok());
    assert!(RuntimeConfig::parse(b"{}").is_ok());
}

#[test]
fn parent_stop_setting_defaults_and_strict_override() {
    for input in ["{}", r#"{"subagents":{}}"#] {
        let config: RuntimeConfig = serde_json::from_str(input).unwrap();
        assert!(config.subagents.stop_children_on_parent_stop);
    }
    let config: RuntimeConfig =
        serde_json::from_str(r#"{"subagents":{"stopChildrenOnParentStop":false}}"#).unwrap();
    assert!(!config.subagents.stop_children_on_parent_stop);
    for input in [
        r#"{"subagents":null}"#,
        r#"{"subagents":{"stopChildrenOnParentStop":null}}"#,
        r#"{"subagents":{"stopChildrenOnParentStop":"false"}}"#,
        r#"{"subagents":{"unknown":false}}"#,
    ] {
        assert!(
            serde_json::from_str::<RuntimeConfig>(input).is_err(),
            "{input}"
        );
    }
}
