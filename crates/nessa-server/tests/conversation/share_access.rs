//! A share's role and tool policy, tested without storage or a socket
//! (`docs/design/read-grants.md`, rows J1 and J3).
use super::{
    ShareAccess, ShareAccessError, ShareRole, ToolPolicy, ToolPolicyError, ToolPreset, ToolRules,
};

fn rules(allow: &[&str], deny: &[&str]) -> Result<ToolRules, ToolPolicyError> {
    ToolRules::new(
        allow.iter().map(|rule| (*rule).to_owned()).collect(),
        deny.iter().map(|rule| (*rule).to_owned()).collect(),
    )
}

#[test]
fn each_role_allows_what_the_roles_before_it_allow() {
    use ShareRole::{Comment, Drive, Read};
    for (role, allows) in [
        (Read, [true, false, false]),
        (Comment, [true, true, false]),
        (Drive, [true, true, true]),
    ] {
        for (other, expected) in [Read, Comment, Drive].into_iter().zip(allows) {
            assert_eq!(
                role.at_least(other),
                expected,
                "{role:?} at least {other:?}"
            );
        }
    }
}

/// Row J3: Drive needs a policy, and Read and Comment take none.
#[test]
fn drive_needs_a_policy_and_read_and_comment_take_none() {
    let policy = || Some(ToolPolicy::Preset(ToolPreset::NoShell));
    assert_eq!(
        ShareAccess::new(ShareRole::Drive, None),
        Err(ShareAccessError::PolicyRequired)
    );
    for role in [ShareRole::Read, ShareRole::Comment] {
        assert_eq!(
            ShareAccess::new(role, policy()),
            Err(ShareAccessError::PolicyNotAllowed)
        );
        assert_eq!(ShareAccess::new(role, None).unwrap().policy(), None);
    }
    let drive = ShareAccess::new(ShareRole::Drive, policy()).unwrap();
    assert_eq!(drive.role(), ShareRole::Drive);
    assert_eq!(drive.policy(), policy().as_ref());
    assert_eq!(
        ShareAccess::read(),
        ShareAccess::new(ShareRole::Read, None).unwrap()
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
