use super::*;
use crate::conversation::domain::{ToolPreset, ToolRules};

fn evaluator() -> CedarToolPolicyEvaluator {
    CedarToolPolicyEvaluator::new().expect("the checked-in bundle validates")
}

fn allowed(rules: &[&str]) -> ToolDecision {
    ToolDecision::Allow {
        rules: rules.iter().map(ToString::to_string).collect(),
    }
}

fn refused(rules: &[&str]) -> ToolDecision {
    ToolDecision::Refuse {
        rules: rules.iter().map(ToString::to_string).collect(),
    }
}

fn under(policy: &ToolPolicy, source: ToolSource, name: &str) -> ToolDecision {
    evaluator()
        .evaluate(
            ToolInitiator::Grant {
                grant: "grant-1",
                policy,
            },
            ToolCall::new(source, name),
        )
        .expect("evaluated")
}

fn rules(allow: &[&str], deny: &[&str]) -> ToolPolicy {
    let owned = |list: &[&str]| list.iter().map(ToString::to_string).collect();
    ToolPolicy::Rules(ToolRules::new(owned(allow), owned(deny)).expect("valid rules"))
}

#[test]
fn the_owner_calls_any_tool() {
    for (source, name) in [
        (ToolSource::Claude, "Write"),
        (ToolSource::Codex, "execute"),
        (ToolSource::Gateway, "run"),
        (ToolSource::Claude, "mcp__docs__delete"),
    ] {
        assert_eq!(
            evaluator()
                .evaluate(ToolInitiator::Owner, ToolCall::new(source, name))
                .unwrap(),
            allowed(&["tool.owner"]),
            "{name}"
        );
    }
}

#[test]
fn read_only_tools_allows_only_named_readers() {
    let policy = ToolPolicy::Preset(ToolPreset::ReadOnlyTools);
    for (source, name) in [
        (ToolSource::Claude, "Read"),
        (ToolSource::Claude, "Grep"),
        (ToolSource::Claude, "ToolSearch"),
        (ToolSource::Codex, "read"),
        (ToolSource::Gateway, "environments_list"),
    ] {
        assert_eq!(
            under(&policy, source, name),
            allowed(&["tool.reads"]),
            "{name}"
        );
    }
    for (source, name) in [
        (ToolSource::Claude, "Write"),
        (ToolSource::Claude, "Bash"),
        (ToolSource::Claude, "mcp__docs__search"),
        (ToolSource::Codex, "execute"),
        // Codex names its web search by this kind too.
        (ToolSource::Codex, "search"),
        (ToolSource::Gateway, "run"),
        (ToolSource::Gateway, "artifacts_publish"),
        // A name means something only next to its source.
        (ToolSource::Claude, "read"),
        (ToolSource::Codex, "Read"),
        (ToolSource::OpenCode, "read"),
    ] {
        assert_eq!(under(&policy, source, name), refused(&[]), "{name}");
    }
}

#[test]
fn no_shell_allows_named_readers_and_editors_and_no_command() {
    let policy = ToolPolicy::Preset(ToolPreset::NoShell);
    assert_eq!(
        under(&policy, ToolSource::Claude, "Read"),
        allowed(&["tool.reads"])
    );
    for (source, name) in [
        (ToolSource::Claude, "Edit"),
        (ToolSource::Claude, "WebFetch"),
        (ToolSource::Codex, "edit"),
        (ToolSource::Codex, "search"),
        (ToolSource::Gateway, "artifacts_fetch"),
    ] {
        assert_eq!(
            under(&policy, source, name),
            allowed(&["tool.edits"]),
            "{name}"
        );
    }
    for (source, name) in [
        (ToolSource::Claude, "Bash"),
        (ToolSource::Codex, "execute"),
        (ToolSource::Gateway, "run"),
        // A tool nothing lists may run a command, so it is refused.
        (ToolSource::Claude, "mcp__nessa__run"),
        (ToolSource::Codex, "other"),
    ] {
        assert_eq!(under(&policy, source, name), refused(&[]), "{name}");
    }
}

#[test]
fn a_deferred_tool_is_loaded_then_judged_on_its_own() {
    // Claude's harness asks for ToolSearch before a deferred tool such as
    // WebFetch. The loader is allowed wherever reads are, so the tool after it
    // is decided by its own name.
    let no_shell = ToolPolicy::Preset(ToolPreset::NoShell);
    assert_eq!(
        under(&no_shell, ToolSource::Claude, "ToolSearch"),
        allowed(&["tool.reads"])
    );
    assert_eq!(
        under(&no_shell, ToolSource::Claude, "WebFetch"),
        allowed(&["tool.edits"])
    );
    let read_only = ToolPolicy::Preset(ToolPreset::ReadOnlyTools);
    assert_eq!(
        under(&read_only, ToolSource::Claude, "ToolSearch"),
        allowed(&["tool.reads"])
    );
    assert_eq!(
        under(&read_only, ToolSource::Claude, "WebFetch"),
        refused(&[])
    );
}

#[test]
fn ask_everything_and_all_allow_every_tool_under_their_own_id() {
    for (preset, id) in [
        (ToolPreset::AskEverything, "tool.preset.ask_everything"),
        (ToolPreset::All, "tool.preset.all"),
    ] {
        let policy = ToolPolicy::Preset(preset);
        assert_eq!(under(&policy, ToolSource::Gateway, "run"), allowed(&[id]));
        assert_eq!(
            under(&policy, ToolSource::Claude, "mcp__x__y"),
            allowed(&[id])
        );
    }
}

#[test]
fn a_deny_pattern_wins_over_an_allow_pattern() {
    let policy = rules(&["Read", "mcp__docs__*"], &["mcp__docs__delete*"]);
    assert_eq!(
        under(&policy, ToolSource::Claude, "Read"),
        allowed(&["tool.rules.allow.0"])
    );
    assert_eq!(
        under(&policy, ToolSource::Claude, "mcp__docs__search"),
        allowed(&["tool.rules.allow.1"])
    );
    assert_eq!(
        under(&policy, ToolSource::Claude, "mcp__docs__delete_page"),
        refused(&["tool.rules.deny.0"])
    );
    // Allow patterns name what is allowed: anything else is refused.
    assert_eq!(under(&policy, ToolSource::Claude, "Write"), refused(&[]));
}

#[test]
fn deny_patterns_alone_leave_every_other_tool_allowed() {
    let policy = rules(&[], &["Bash", "Web*"]);
    assert_eq!(
        under(&policy, ToolSource::Claude, "Write"),
        allowed(&["tool.rules.unlisted"])
    );
    assert_eq!(
        under(&policy, ToolSource::Claude, "WebFetch"),
        refused(&["tool.rules.deny.1"])
    );
    assert_eq!(
        under(&policy, ToolSource::Gateway, "run"),
        allowed(&["tool.rules.unlisted"])
    );
}

#[test]
fn a_star_matches_any_run_of_characters_anywhere() {
    let policy = rules(&["R*d", "*_list", "*"], &[]);
    assert_eq!(
        under(&policy, ToolSource::Claude, "Read"),
        allowed(&["tool.rules.allow.0", "tool.rules.allow.2"])
    );
    assert_eq!(
        under(&policy, ToolSource::Gateway, "environments_list"),
        allowed(&["tool.rules.allow.1", "tool.rules.allow.2"])
    );
    let exact = rules(&["Read"], &[]);
    assert_eq!(under(&exact, ToolSource::Claude, "Reader"), refused(&[]));
    assert_eq!(under(&exact, ToolSource::Claude, "read"), refused(&[]));
}

#[test]
fn a_deny_pattern_of_stars_alone_denies_every_tool() {
    for pattern in ["*", "**"] {
        let policy = rules(&["Read"], &[pattern]);
        assert_eq!(
            under(&policy, ToolSource::Claude, "Read"),
            refused(&["tool.rules.deny.0"]),
            "{pattern}"
        );
    }
}

#[test]
fn a_pattern_matches_characters_not_bytes() {
    let policy = rules(&["outil_é*", "工具"], &[]);
    assert_eq!(
        under(&policy, ToolSource::Claude, "outil_écrire"),
        allowed(&["tool.rules.allow.0"])
    );
    assert_eq!(
        under(&policy, ToolSource::Claude, "工具"),
        allowed(&["tool.rules.allow.1"])
    );
    assert_eq!(under(&policy, ToolSource::Claude, "outil_e"), refused(&[]));
}

#[test]
fn a_policy_that_errors_makes_the_decision_unavailable() {
    // Cedar skips a forbid whose condition errors; were that ignored, this
    // bundle would allow the call.
    let bundle = Bundle::from_sources(
        include_str!("../../src/conversation/infrastructure/tool_policy/cedar/schema.json"),
        &[
            r#"@id("all") permit(principal, action, resource);"#,
            r#"@id("overflow") forbid(principal, action, resource) when { 9223372036854775807 + 1 > 0 };"#,
        ],
    )
    .expect("validates: the overflow only shows when evaluated");
    let evaluator = CedarToolPolicyEvaluator::with_bundle(bundle);
    assert_eq!(
        evaluator.evaluate(
            ToolInitiator::Owner,
            ToolCall::new(ToolSource::Claude, "Read")
        ),
        Err(ToolPolicyUnavailable)
    );
}

#[test]
fn a_pattern_is_text_never_policy_syntax() {
    // Each of these would widen the policy if it were formatted into Cedar.
    for pattern in [
        r#"x" || true || ""#,
        r#"x"}; permit(principal,action,resource);"#,
        r"\*",
        r#"\""#,
    ] {
        let policy = rules(&[pattern], &[]);
        assert_eq!(
            under(&policy, ToolSource::Claude, "Write"),
            refused(&[]),
            "{pattern}"
        );
        let literal = pattern.replace('*', "");
        if !pattern.contains('*') {
            assert_eq!(
                under(&policy, ToolSource::Claude, &literal),
                allowed(&["tool.rules.allow.0"]),
                "{pattern}"
            );
        }
    }
}

#[test]
fn a_grant_named_like_syntax_is_still_one_grant() {
    let policy = rules(&["Read"], &[]);
    let decision = evaluator()
        .evaluate(
            ToolInitiator::Grant {
                grant: r#"a" } permit(principal, action, resource); //"#,
                policy: &policy,
            },
            ToolCall::new(ToolSource::Claude, "Write"),
        )
        .unwrap();
    assert_eq!(decision, refused(&[]));
}

#[test]
fn a_bundle_that_does_not_validate_is_unavailable() {
    let schema =
        include_str!("../../src/conversation/infrastructure/tool_policy/cedar/schema.json");
    for files in [
        // No @id.
        &["permit(principal, action, resource);"][..],
        // An id reserved for a grant's rules.
        &[r#"@id("tool.rules.allow.0") permit(principal, action, resource);"#][..],
        // A repeated id.
        &[
            r#"@id("a") permit(principal, action, resource);"#,
            r#"@id("a") permit(principal, action, resource);"#,
        ][..],
        // An attribute the schema does not have.
        &[
            r#"@id("a") permit(principal is Nessa::Grant, action, resource) when { principal.role == "drive" };"#,
        ][..],
        &["not cedar"][..],
    ] {
        assert!(Bundle::from_sources(schema, files).is_err(), "{files:?}");
    }
    assert!(Bundle::from_sources("{", &[]).is_err());
}
