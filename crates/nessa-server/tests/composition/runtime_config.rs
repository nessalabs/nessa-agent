//! `sshHosts` in `config.json`: the hosts a conversation may be created on;
//! and `environmentTools`: where the agent may run commands, and which.
use super::{RuntimeConfig, MAX_POLICY_PROGRAMS, MAX_SSH_HOSTS};
use crate::conversation::domain::CommandPolicy;

fn hosts(json: &str) -> Result<Vec<String>, String> {
    let config = RuntimeConfig::parse(json.as_bytes()).map_err(|error| error.to_string())?;
    Ok(config
        .ssh_hosts()
        .unwrap()
        .iter()
        .map(|host| host.as_str().to_owned())
        .collect())
}

#[test]
fn a_configuration_naming_no_host_names_none() {
    assert_eq!(hosts("{}"), Ok(Vec::new()));
    assert_eq!(hosts(r#"{"sshHosts":[]}"#), Ok(Vec::new()));
}

#[test]
fn each_host_is_kept_in_order_as_a_destination() {
    assert_eq!(
        hosts(r#"{"sshHosts":["devbox","me@build.example"]}"#),
        Ok(vec!["devbox".into(), "me@build.example".into()])
    );
}

#[test]
fn a_host_that_is_not_a_destination_a_repeat_or_too_many_refuse_the_configuration() {
    let many: Vec<String> = (0..=MAX_SSH_HOSTS).map(|n| format!("host{n}")).collect();
    for json in [
        r#"{"sshHosts":["-oProxyCommand=sh"]}"#.to_owned(),
        r#"{"sshHosts":["host name"]}"#.to_owned(),
        r#"{"sshHosts":["devbox","devbox"]}"#.to_owned(),
        r#"{"sshHosts":"devbox"}"#.to_owned(),
        serde_json::json!({ "sshHosts": many }).to_string(),
    ] {
        assert!(hosts(&json).is_err(), "{json}");
    }
}

fn policy(json: &str) -> Result<Option<CommandPolicy>, String> {
    let config = RuntimeConfig::parse(json.as_bytes()).map_err(|error| error.to_string())?;
    Ok(config.command_policy().unwrap())
}

#[test]
fn environment_tools_are_off_until_configured() {
    assert_eq!(policy("{}"), Ok(None));
    assert_eq!(policy(r#"{"environmentTools":null}"#), Ok(None));
}

#[test]
fn environment_tools_grant_commands_on_named_hosts_under_their_programs() {
    let policy = policy(
        r#"{"sshHosts":["devbox","build"],"environmentTools":{"commandHosts":["devbox"],"allowPrograms":["cargo","ls"],"denyPrograms":["ls"]}}"#,
    )
    .unwrap()
    .unwrap();
    // Read as the facts the command policy decides with.
    let set = |names: &[&str]| -> std::collections::BTreeSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    };
    assert_eq!(policy.hosts(), &set(&["devbox"]));
    assert_eq!(policy.allow(), Some(&set(&["cargo", "ls"])));
    assert_eq!(policy.deny(), &set(&["ls"]));
    // A paired environment is named as it dials in, never in sshHosts.
    let peer = format!("peer:{}", "ab".repeat(32));
    let paired = self::policy(
        &serde_json::json!({"environmentTools": {"commandHosts": [peer]}}).to_string(),
    )
    .unwrap()
    .unwrap();
    assert!(paired.hosts().contains(&peer));
}

#[test]
fn environment_tools_naming_an_unknown_host_or_a_path_refuse_the_configuration() {
    let many: Vec<String> = (0..=MAX_POLICY_PROGRAMS).map(|n| format!("p{n}")).collect();
    for json in [
        r#"{"environmentTools":{"commandHosts":["devbox"]}}"#.to_owned(),
        r#"{"environmentTools":{"commandHosts":["peer:AB"]}}"#.to_owned(),
        r#"{"sshHosts":["devbox"],"environmentTools":{}}"#.to_owned(),
        r#"{"sshHosts":["devbox"],"environmentTools":{"commandHosts":["devbox"],"denyPrograms":["/bin/rm"]}}"#.to_owned(),
        r#"{"sshHosts":["devbox"],"environmentTools":{"commandHosts":["devbox"],"allowPrograms":[""]}}"#.to_owned(),
        r#"{"sshHosts":["devbox"],"environmentTools":{"commandHosts":["devbox"],"shell":true}}"#.to_owned(),
        serde_json::json!({"sshHosts": ["devbox"], "environmentTools": {"commandHosts": ["devbox"], "denyPrograms": many}}).to_string(),
    ] {
        assert!(policy(&json).is_err(), "{json}");
    }
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
