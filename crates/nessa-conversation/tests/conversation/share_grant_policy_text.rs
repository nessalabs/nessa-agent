//! A Drive grant's stored policy text (`store::share_grants`): one spelling
//! per policy, read back through the domain, within the bound it is read under.
use super::*;

/// A policy reads back as itself, and the stored text of the longest one
/// fits the bound its read is acquired under.
#[test]
fn a_policy_reads_back_as_itself_within_its_bound() {
    let longest = "\"".repeat(ToolRules::MAX_RULE_BYTES);
    let patterns = |n: usize| vec![longest.clone(); n];
    // Every host at its longest; a host has nothing JSON escapes.
    let hosts = |n: usize| {
        CommandHosts::new(
            (0..n)
                .map(|i| format!("{i:x}{}", "h".repeat(CommandHosts::MAX_HOST_BYTES - 1)))
                .collect(),
        )
        .unwrap()
    };
    let rules = |allow: usize, deny: usize| {
        ToolPolicy::rules(ToolRules::new(patterns(allow), patterns(deny)).unwrap())
    };
    let full = ToolRules::MAX_RULES;
    let most = CommandHosts::MAX_HOSTS;
    let mut policies = vec![
        rules(full, full),
        rules(full, full).with_command_hosts(hosts(most)),
        rules(full, 0).with_command_hosts(hosts(most)),
        rules(0, full).with_command_hosts(hosts(most)),
        rules(full / 2, full / 2).with_command_hosts(hosts(most)),
    ];
    for preset in [
        ToolPreset::ReadOnlyTools,
        ToolPreset::NoShell,
        ToolPreset::AskEverything,
        ToolPreset::All,
    ] {
        policies.push(ToolPolicy::preset(preset));
        policies.push(ToolPolicy::preset(preset).with_command_hosts(hosts(most)));
    }
    // Hosts are not counted against the patterns.
    assert_eq!(
        rules(full, full)
            .with_command_hosts(hosts(1))
            .command_hosts()
            .names()
            .len(),
        1
    );
    for policy in policies {
        let text = policy_text(&policy);
        assert!(text.len() <= MAX_POLICY_TEXT_BYTES, "{}", text.len());
        assert_eq!(policy_from(&text), Some(policy));
    }
    for damaged in [
        "[]",
        "{}",
        r#"{"preset":"everything"}"#,
        r#"{"allow":[]}"#,
        r#"{"allow":[],"deny":[""]}"#,
        r#"{"allow":[],"deny":[]}"#,
        r#"{"preset":"all","allow":[]}"#,
        // Command hosts beside a policy, never alone, never empty, each a
        // host `run` could name, once.
        r#"{"commandHosts":["devbox"]}"#,
        r#"{"preset":"all","commandHosts":[]}"#,
        r#"{"preset":"all","commandHosts":["here"]}"#,
        r#"{"preset":"all","commandHosts":["devbox","devbox"]}"#,
        r#"{"preset":"all","commandHosts":[" devbox"]}"#,
        r#"{"preset":"all","commandHosts":"devbox"}"#,
    ] {
        assert_eq!(policy_from(damaged), None, "{damaged}");
    }
}

/// A policy stored before command hosts were has no `commandHosts` and
/// names none; one that names hosts reads them back, and its text differs
/// from the same tools on none, so changing a grant's hosts is a change.
#[test]
fn command_hosts_round_trip_and_a_policy_without_them_names_none() {
    let stored = r#"{"preset":"all"}"#;
    let old = policy_from(stored).unwrap();
    assert_eq!(old, ToolPolicy::preset(ToolPreset::All));
    assert!(old.command_hosts().is_empty());
    assert_eq!(policy_text(&old), stored);

    let hosts = CommandHosts::new(vec![
        "peer:".to_owned() + &"ab".repeat(32),
        "devbox".to_owned(),
    ])
    .unwrap();
    let on = ToolPolicy::preset(ToolPreset::All).with_command_hosts(hosts.clone());
    let text = policy_text(&on);
    assert_eq!(policy_from(&text), Some(on.clone()));
    assert_ne!(text, stored);
    assert_eq!(
        policy_from(&text)
            .unwrap()
            .command_hosts()
            .names()
            .iter()
            .collect::<Vec<_>>(),
        ["devbox", &format!("peer:{}", "ab".repeat(32))]
    );
    let rules = ToolPolicy::rules(ToolRules::new(vec!["Read".into()], Vec::new()).unwrap())
        .with_command_hosts(hosts);
    assert_eq!(policy_from(&policy_text(&rules)), Some(rules));
}
