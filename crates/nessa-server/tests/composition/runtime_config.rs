//! `sshHosts` in `config.json`: the hosts a conversation may be created on.
use super::{RuntimeConfig, MAX_SSH_HOSTS};

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
