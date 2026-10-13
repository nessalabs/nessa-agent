//! The command policy, answered by Cedar: one test per policy id, the grant's
//! own hosts, an error refusing, and — until `run` asks this port — the
//! same answers as `CommandPolicy::admit` and `grants` for the owner.
use super::*;
use crate::conversation::application::admit_command;
use crate::conversation::domain::{ToolPolicy, ToolPreset};
use nessa_sdk::domain::agent_execution::leases::CommandRefusal;
use std::collections::BTreeSet;

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn evaluator(policy: &CommandPolicy) -> CedarCommandPolicyEvaluator {
    CedarCommandPolicyEvaluator::new(policy).expect("the checked-in bundle validates")
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

const PRESET: ToolPolicy = ToolPolicy::Preset(ToolPreset::All);

fn host(host: &str) -> CommandQuestion<'_> {
    CommandQuestion::Host { host }
}

fn run<'a>(host: &'a str, program: &'a str) -> CommandQuestion<'a> {
    CommandQuestion::Run { host, program }
}

#[test]
fn command_owner_host_the_owner_uses_every_configured_host_and_no_other() {
    let policy = CommandPolicy::new(names(&["devbox", "peer:lab"]), None, Vec::new());
    let cedar = evaluator(&policy);
    for name in ["devbox", "peer:lab"] {
        assert_eq!(
            cedar.evaluate(CommandInitiator::Owner, host(name)).unwrap(),
            allowed(&["command.owner_host"]),
            "{name}"
        );
        assert_eq!(
            cedar
                .evaluate(CommandInitiator::Owner, run(name, "ls"))
                .unwrap(),
            allowed(&["command.owner_host"]),
            "{name}"
        );
    }
    assert_eq!(
        cedar
            .evaluate(CommandInitiator::Owner, host("build"))
            .unwrap(),
        refused(&[])
    );
    assert_eq!(
        cedar
            .evaluate(CommandInitiator::Owner, run("build", "ls"))
            .unwrap(),
        refused(&[])
    );
}

#[test]
fn command_grant_host_a_grant_uses_only_configured_hosts_it_names_itself() {
    let policy = CommandPolicy::new(names(&["devbox", "build"]), None, Vec::new());
    let cedar = evaluator(&policy);
    let own = set(&["devbox", "elsewhere"]);
    let grant = CommandInitiator::Grant {
        grant: "grant-1",
        policy: &PRESET,
        hosts: Some(&own),
    };
    assert_eq!(
        cedar.evaluate(grant, host("devbox")).unwrap(),
        allowed(&["command.grant_host"])
    );
    assert_eq!(
        cedar.evaluate(grant, run("devbox", "ls")).unwrap(),
        allowed(&["command.grant_host"])
    );
    // Configured, but the grant does not name it: the owner's hosts are not
    // the grant's.
    assert_eq!(cedar.evaluate(grant, host("build")).unwrap(), refused(&[]));
    // Named by the grant, but not configured on this gateway.
    assert_eq!(
        cedar.evaluate(grant, host("elsewhere")).unwrap(),
        refused(&[])
    );
}

#[test]
fn a_grant_that_names_no_hosts_uses_none() {
    let policy = CommandPolicy::new(names(&["devbox"]), None, Vec::new());
    let cedar = evaluator(&policy);
    let empty = BTreeSet::new();
    for hosts in [None, Some(&empty)] {
        let grant = CommandInitiator::Grant {
            grant: "grant-1",
            policy: &PRESET,
            hosts,
        };
        assert_eq!(
            cedar.evaluate(grant, host("devbox")).unwrap(),
            refused(&[]),
            "{hosts:?}"
        );
        assert_eq!(
            admit_command(&cedar, grant, "devbox", "ls"),
            Err(CommandRefusal::EnvironmentNotGranted),
            "{hosts:?}"
        );
    }
}

#[test]
fn command_unlisted_program_with_an_allow_list_only_a_bare_listed_name_runs() {
    let policy = CommandPolicy::new(names(&["devbox"]), Some(names(&["cargo"])), Vec::new());
    let cedar = evaluator(&policy);
    assert_eq!(
        cedar
            .evaluate(CommandInitiator::Owner, run("devbox", "cargo"))
            .unwrap(),
        allowed(&["command.owner_host"])
    );
    for program in ["sh", "/usr/bin/cargo", "./cargo", "cargo-ext"] {
        assert_eq!(
            cedar
                .evaluate(CommandInitiator::Owner, run("devbox", program))
                .unwrap(),
            refused(&["command.unlisted_program"]),
            "{program}"
        );
    }
    // The program is not looked at when only the host is asked.
    assert_eq!(
        cedar
            .evaluate(CommandInitiator::Owner, host("devbox"))
            .unwrap(),
        allowed(&["command.owner_host"])
    );
}

#[test]
fn command_denied_program_a_denied_file_name_never_runs_and_wins() {
    let policy = CommandPolicy::new(names(&["devbox"]), Some(names(&["rm"])), names(&["rm"]));
    let cedar = evaluator(&policy);
    assert_eq!(
        cedar
            .evaluate(CommandInitiator::Owner, run("devbox", "rm"))
            .unwrap(),
        refused(&["command.denied_program"])
    );
    assert_eq!(
        cedar
            .evaluate(CommandInitiator::Owner, run("devbox", "/usr/bin/rm"))
            .unwrap(),
        refused(&["command.denied_program", "command.unlisted_program"])
    );
    let own = set(&["devbox"]);
    let grant = CommandInitiator::Grant {
        grant: "grant-1",
        policy: &PRESET,
        hosts: Some(&own),
    };
    assert_eq!(
        cedar.evaluate(grant, run("devbox", "rm")).unwrap(),
        refused(&["command.denied_program"])
    );
}

#[test]
fn a_host_or_program_named_like_syntax_is_still_only_text() {
    let policy = CommandPolicy::new(names(&["devbox"]), None, names(&["rm"]));
    let cedar = evaluator(&policy);
    for name in [r#"devbox" || true || ""#, r"devbox\", "*"] {
        assert_eq!(
            cedar.evaluate(CommandInitiator::Owner, host(name)).unwrap(),
            refused(&[]),
            "{name}"
        );
    }
    assert_eq!(
        cedar
            .evaluate(CommandInitiator::Owner, run("devbox", r#"rm" "#))
            .unwrap(),
        allowed(&["command.owner_host"])
    );
}

#[test]
fn a_policy_that_errors_makes_the_command_decision_unavailable() {
    // Cedar skips a forbid whose condition errors; were that ignored, this
    // bundle would allow the command.
    let bundle = Bundle::from_sources(
        include_str!("../../src/conversation/infrastructure/tool_policy/cedar/schema.json"),
        &[
            r#"@id("all") permit(principal, action, resource);"#,
            r#"@id("overflow") forbid(principal, action, resource) when { 9223372036854775807 + 1 > 0 };"#,
        ],
    )
    .expect("validates: the overflow only shows when evaluated");
    let policy = CommandPolicy::new(names(&["devbox"]), None, Vec::new());
    let cedar = CedarCommandPolicyEvaluator::with_bundle(bundle, &policy).unwrap();
    assert_eq!(
        cedar.evaluate(CommandInitiator::Owner, host("devbox")),
        Err(ToolPolicyUnavailable)
    );
    assert_eq!(
        cedar.evaluate(CommandInitiator::Owner, run("devbox", "ls")),
        Err(ToolPolicyUnavailable)
    );
    // And `run` refuses it, as the first question it could not answer.
    assert_eq!(
        admit_command(&cedar, CommandInitiator::Owner, "devbox", "ls"),
        Err(CommandRefusal::EnvironmentNotGranted)
    );
}

/// Scaffolding for the switch (slice J4 part 3): until `run` asks this port,
/// Cedar must answer the owner exactly as `CommandPolicy` does. Deleted with
/// `CommandPolicy::admit` and `grants`.
#[test]
fn for_the_owner_cedar_answers_as_command_policy_does() {
    let policies = [
        CommandPolicy::new(names(&["devbox"]), None, Vec::new()),
        CommandPolicy::new(names(&["devbox"]), None, names(&["rm"])),
        CommandPolicy::new(
            names(&["devbox", "peer:lab"]),
            Some(names(&["cargo", "rm"])),
            names(&["rm"]),
        ),
        CommandPolicy::new(names(&["devbox"]), Some(Vec::new()), Vec::new()),
        CommandPolicy::new(Vec::new(), None, Vec::new()),
        CommandPolicy::new(Vec::new(), Some(Vec::new()), Vec::new()),
    ];
    let hosts = ["devbox", "peer:lab", "build", ""];
    let programs = [
        "cargo",
        "rm",
        "/bin/rm",
        "/usr/bin/rm",
        "./rm",
        "./cargo",
        "/tmp/x/cargo",
        "cargo-ext",
        "sh",
        "ls",
        "",
        "/",
        "rm/",
    ];
    for policy in &policies {
        let cedar = evaluator(policy);
        for host_name in hosts {
            let granted = matches!(
                cedar.evaluate(CommandInitiator::Owner, host(host_name)),
                Ok(ToolDecision::Allow { .. })
            );
            assert_eq!(granted, policy.grants(host_name), "{policy:?} {host_name}");
            for program in programs {
                assert_eq!(
                    admit_command(&cedar, CommandInitiator::Owner, host_name, program).map(|_| ()),
                    policy.admit(host_name, program),
                    "{policy:?} {host_name} {program}"
                );
            }
        }
    }
}
