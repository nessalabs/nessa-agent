//! A share's role and tool policy, tested without storage or a socket
//! (`docs/design/read-grants.md`, rows J1 and J3).
use super::{
    CommandHosts, ShareAccess, ShareAccessError, ShareRole, ToolPolicy, ToolPolicyError,
    ToolPreset, ToolRules,
};

fn rules(allow: &[&str], deny: &[&str]) -> Result<ToolRules, ToolPolicyError> {
    ToolRules::new(
        allow.iter().map(|rule| (*rule).to_owned()).collect(),
        deny.iter().map(|rule| (*rule).to_owned()).collect(),
    )
}

/// Row J3: Drive needs a policy, and Read and Comment take none.
#[test]
fn drive_needs_a_policy_and_read_and_comment_take_none() {
    let policy = || Some(ToolPolicy::preset(ToolPreset::NoShell));
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

/// A Drive policy's command hosts are names `run` could take, each once, at
/// most `MAX_HOSTS`; none is the default, and a policy keeps them beside its
/// tools.
#[test]
fn command_hosts_are_bounded_and_named_as_run_names_them() {
    let names = |hosts: &[&str]| hosts.iter().map(|host| (*host).to_owned()).collect();
    let peer = format!("peer:{}", "0f".repeat(32));
    let kept = CommandHosts::new(names(&["devbox", "user@build.example", &peer])).unwrap();
    assert_eq!(
        kept.names().iter().map(String::as_str).collect::<Vec<_>>(),
        ["devbox", &peer, "user@build.example"]
    );
    assert!(CommandHosts::new(Vec::new()).unwrap().is_empty());
    assert_eq!(CommandHosts::new(Vec::new()).unwrap(), CommandHosts::none());
    let full: Vec<String> = (0..CommandHosts::MAX_HOSTS)
        .map(|n| format!("h{n}"))
        .collect();
    assert!(CommandHosts::new(full.clone()).is_ok());
    let mut over = full;
    over.push("one-more".into());
    assert_eq!(CommandHosts::new(over), Err(ToolPolicyError::TooManyHosts));
    for malformed in [
        "",
        "here",
        " devbox",
        "-devbox",
        "dev box",
        "peer:",
        "peer:ZZ",
        &format!("peer:{}", "0F".repeat(32)),
        &"h".repeat(CommandHosts::MAX_HOST_BYTES + 1),
    ] {
        assert_eq!(
            CommandHosts::new(names(&[malformed])),
            Err(ToolPolicyError::MalformedHost),
            "{malformed:?}"
        );
    }
    assert_eq!(
        CommandHosts::new(names(&["devbox", "devbox"])),
        Err(ToolPolicyError::MalformedHost)
    );
    assert!(CommandHosts::new(vec!["h".repeat(CommandHosts::MAX_HOST_BYTES)]).is_ok());
    // Hosts have their own budget: both pattern lists full, and every host.
    let full: Vec<String> = (0..ToolRules::MAX_RULES).map(|n| format!("t{n}")).collect();
    let rules = ToolPolicy::rules(ToolRules::new(full.clone(), full).unwrap());
    let hosts = CommandHosts::new(
        (0..CommandHosts::MAX_HOSTS)
            .map(|n| format!("h{n}"))
            .collect(),
    )
    .unwrap();
    let with_hosts = rules.clone().with_command_hosts(hosts.clone());
    assert_eq!(with_hosts.command_hosts(), &hosts);
    assert_eq!(
        rules
            .with_command_hosts(CommandHosts::none())
            .command_hosts(),
        &CommandHosts::none()
    );
    let policy = ToolPolicy::preset(ToolPreset::NoShell);
    assert!(policy.command_hosts().is_empty());
    let on = policy.clone().with_command_hosts(kept.clone());
    assert_eq!(on.command_hosts(), &kept);
    assert_eq!(on.tools(), policy.tools());
    assert_ne!(on, policy);
}
