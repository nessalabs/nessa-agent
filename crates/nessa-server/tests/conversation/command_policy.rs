//! Which commands the agent may run, and where, decided without anything
//! running.
use super::CommandPolicy;
use nessa_sdk::domain::agent_execution::leases::CommandRefusal;

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn a_host_not_granted_is_refused_before_its_program_is_looked_at() {
    let policy = CommandPolicy::new(names(&["devbox"]), None, Vec::new());
    assert!(policy.grants("devbox"));
    assert!(!policy.grants("build"));
    assert_eq!(
        policy.admit("build", "ls"),
        Err(CommandRefusal::EnvironmentNotGranted)
    );
    assert_eq!(policy.admit("devbox", "ls"), Ok(()));
}

#[test]
fn a_program_is_matched_by_its_file_name_and_a_denial_wins() {
    let policy = CommandPolicy::new(
        names(&["devbox"]),
        Some(names(&["cargo", "rm"])),
        names(&["rm"]),
    );
    assert_eq!(policy.admit("devbox", "cargo"), Ok(()));
    assert_eq!(policy.admit("devbox", "/home/me/.cargo/bin/cargo"), Ok(()));
    for denied in ["rm", "/bin/rm", "./rm", "sh", "cargo-ext"] {
        assert_eq!(
            policy.admit("devbox", denied),
            Err(CommandRefusal::CommandDenied),
            "{denied}"
        );
    }
    let open = CommandPolicy::new(names(&["devbox"]), None, names(&["rm"]));
    assert_eq!(open.admit("devbox", "anything"), Ok(()));
    assert_eq!(open.admit("devbox", "/usr/bin/rm"), Err(CommandRefusal::CommandDenied));
}

#[test]
fn a_policy_described_differs_whenever_what_it_grants_does() {
    let base = CommandPolicy::new(names(&["devbox"]), None, Vec::new());
    for other in [
        CommandPolicy::new(names(&["build"]), None, Vec::new()),
        CommandPolicy::new(names(&["devbox"]), Some(Vec::new()), Vec::new()),
        CommandPolicy::new(names(&["devbox"]), None, names(&["rm"])),
    ] {
        assert_ne!(base.describe(), other.describe());
    }
    assert_eq!(
        base.describe(),
        CommandPolicy::new(names(&["devbox", "devbox"]), None, Vec::new()).describe()
    );
}
