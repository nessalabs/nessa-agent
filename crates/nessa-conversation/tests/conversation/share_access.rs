//! A share's role and tool policy, tested without storage or a socket
//! (`docs/design/read-grants.md`, rows J1 and J3).
use super::{
    Capabilities, ShareAccess, ShareAccessError, ToolPolicy, ToolPolicyError, ToolPreset, ToolRules,
};

fn rules(allow: &[&str], deny: &[&str]) -> Result<ToolRules, ToolPolicyError> {
    ToolRules::new(
        allow.iter().map(|rule| (*rule).to_owned()).collect(),
        deny.iter().map(|rule| (*rule).to_owned()).collect(),
    )
}

/// The only capability sets are the three cumulative ones, in any order.
#[test]
fn a_capability_set_is_read_or_read_and_comment_or_all_three() {
    assert_eq!(
        Capabilities::from_names(["comment", "read"]).unwrap(),
        Capabilities::comment()
    );
    assert_eq!(
        Capabilities::from_names(["drive", "read", "comment"]).unwrap(),
        Capabilities::drive()
    );
    for names in [
        &[][..],
        &["read", "read"][..],
        &["comment"][..],
        &["drive"][..],
        &["read", "drive"][..],
        &["owner"][..],
    ] {
        assert_eq!(
            Capabilities::from_names(names.iter().copied()),
            Err(ShareAccessError::Capabilities),
            "{names:?}"
        );
    }
    assert_eq!(Capabilities::drive().as_str(), "read+comment+drive");
    assert_eq!(
        Capabilities::parse("read+comment"),
        Some(Capabilities::comment())
    );
}

/// Row J3: Drive needs a policy, and Read and Comment take none.
#[test]
fn drive_needs_a_policy_and_read_and_comment_take_none() {
    let policy = || Some(ToolPolicy::preset(ToolPreset::NoShell));
    assert_eq!(
        ShareAccess::new(Capabilities::drive(), None),
        Err(ShareAccessError::PolicyRequired)
    );
    for role in [Capabilities::read(), Capabilities::comment()] {
        assert_eq!(
            ShareAccess::new(role, policy()),
            Err(ShareAccessError::PolicyNotAllowed)
        );
        assert_eq!(ShareAccess::new(role, None).unwrap().policy(), None);
    }
    let drive = ShareAccess::new(Capabilities::drive(), policy()).unwrap();
    assert_eq!(drive.capabilities(), Capabilities::drive());
    assert_eq!(drive.policy(), policy().as_ref());
    assert_eq!(
        ShareAccess::read(),
        ShareAccess::new(Capabilities::read(), None).unwrap()
    );
}

/// Row J3: a policy is bounded in how many patterns it holds and how long
/// each is, and a pattern is plain text.
#[test]
fn tool_rules_are_bounded_and_plain() {
    let longest = "x".repeat(ToolRules::MAX_RULE_BYTES);
    let kept = rules(&["Read", &longest], &["Bash(rm -rf:*)"]).unwrap();
    assert_eq!(kept.allow(), ["Read".to_owned(), longest.clone()]);
    assert_eq!(kept.deny(), ["Bash(rm -rf:*)".to_owned()]);
    assert_eq!(rules(&[], &[]), Err(ToolPolicyError::NoRules));
    assert!(rules(&["Read"], &[]).is_ok());
    assert!(rules(&[], &["Bash"]).is_ok());

    // Each list is bounded on its own: both full is allowed, one past is not.
    let full: Vec<String> = (0..ToolRules::MAX_RULES).map(|n| format!("t{n}")).collect();
    assert!(ToolRules::new(full.clone(), full.clone()).is_ok());
    let mut over = full.clone();
    over.push("one-more".into());
    assert_eq!(
        ToolRules::new(over.clone(), full.clone()),
        Err(ToolPolicyError::TooManyRules)
    );
    assert_eq!(
        ToolRules::new(full, over),
        Err(ToolPolicyError::TooManyRules)
    );
    // The bound is bytes: one multi-byte character past it is refused.
    let past = format!("{}é", "x".repeat(ToolRules::MAX_RULE_BYTES - 1));
    assert_eq!(rules(&[&past], &[]), Err(ToolPolicyError::RuleTooLong));
    for malformed in ["", " Read", "Read ", "Re\nad", "\u{7f}"] {
        assert_eq!(
            rules(&[], &[malformed]),
            Err(ToolPolicyError::MalformedRule),
            "{malformed:?}"
        );
    }
}
