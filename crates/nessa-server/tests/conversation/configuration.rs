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
