//! A Drive grant's stored policy text (`store::share_grants`): one spelling
//! per policy, read back through the domain, within the bound it is read under.
use super::*;

/// A policy reads back as itself, and the stored text of the longest one
/// fits the bound its read is acquired under.
#[test]
fn a_policy_reads_back_as_itself_within_its_bound() {
    let longest = "\"".repeat(ToolRules::MAX_RULE_BYTES);
    let patterns = |n: usize| vec![longest.clone(); n];
    let rules = |allow: usize, deny: usize| {
        ToolPolicy::rules(ToolRules::new(patterns(allow), patterns(deny)).unwrap())
    };
    let full = ToolRules::MAX_RULES;
    let mut policies = vec![
        rules(full, full),
        rules(full, 0),
        rules(0, full),
        rules(full / 2, full / 2),
    ];
    for preset in [
        ToolPreset::ReadOnlyTools,
        ToolPreset::NoShell,
        ToolPreset::AskEverything,
        ToolPreset::All,
    ] {
        policies.push(ToolPolicy::preset(preset));
    }
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
        r#"{"commandHosts":["devbox"]}"#,
    ] {
        assert_eq!(policy_from(damaged), None, "{damaged}");
    }
}

/// A policy an older build stored with command hosts, the machines its turns
/// could run commands on before every gateway ran only its own agents
/// (ADR 1417), reads as the same tools on no host, at the longest such a
/// build wrote; written again, it is spelled without them.
#[test]
fn a_policy_stored_with_command_hosts_reads_without_them() {
    let host = |i: usize| format!("{i:x}{}", "h".repeat(252));
    let hosts = (0..16).map(host).collect::<Vec<_>>();
    let longest = "\"".repeat(ToolRules::MAX_RULE_BYTES);
    let patterns = vec![longest; ToolRules::MAX_RULES];
    let rules = ToolPolicy::rules(ToolRules::new(patterns.clone(), patterns.clone()).unwrap());
    let stored = json!({ "allow": patterns, "commandHosts": hosts, "deny": patterns }).to_string();
    assert!(stored.len() <= MAX_POLICY_TEXT_BYTES, "{}", stored.len());
    assert_eq!(policy_from(&stored), Some(rules.clone()));
    assert_eq!(
        policy_text(&rules),
        policy_text(&policy_from(&stored).unwrap())
    );
    assert!(!policy_text(&rules).contains("commandHosts"));

    let preset = format!(
        r#"{{"commandHosts":["devbox","peer:{}"],"preset":"all"}}"#,
        "ab".repeat(32)
    );
    assert_eq!(
        policy_from(&preset),
        Some(ToolPolicy::preset(ToolPreset::All))
    );
    assert_eq!(
        policy_text(&ToolPolicy::preset(ToolPreset::All)),
        r#"{"preset":"all"}"#
    );
}
