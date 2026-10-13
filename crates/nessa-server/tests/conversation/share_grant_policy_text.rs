//! A Drive grant's stored policy text (`store::share_grants`): one spelling
//! per policy, read back through the domain, within the bound it is read under.
use super::*;

/// A policy reads back as itself, and the stored text of the longest one
/// fits the bound its read is acquired under.
#[test]
fn a_policy_reads_back_as_itself_within_its_bound() {
    let longest = "\"".repeat(ToolRules::MAX_RULE_BYTES);
    let rules = ToolRules::new(
        vec![longest.clone(); ToolRules::MAX_RULES],
        vec![longest; ToolRules::MAX_RULES],
    )
    .unwrap();
    let mut policies = vec![ToolPolicy::Rules(rules)];
    policies.extend(
        [
            ToolPreset::ReadOnlyTools,
            ToolPreset::NoShell,
            ToolPreset::AskEverything,
            ToolPreset::All,
        ]
        .map(ToolPolicy::Preset),
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
    ] {
        assert_eq!(policy_from(damaged), None, "{damaged}");
    }
}
