//! The command policy, answered by Cedar: one test per policy id, the grant's
//! own hosts, an error refusing, and the questions `run` asks as truth tables
//! for the owner and for a grant.
use super::*;
use crate::conversation::application::admit_command;
use crate::conversation::domain::{CommandHosts, ToolPolicy, ToolPreset};
use nessa_sdk::domain::agent_execution::leases::CommandRefusal;

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// A Drive policy whose turns may use `hosts`.
fn on(hosts: &[&str]) -> ToolPolicy {
    ToolPolicy::preset(ToolPreset::All)
        .with_command_hosts(CommandHosts::new(names(hosts)).expect("valid hosts"))
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
fn an_agent_turn_is_refused_by_cedar_even_on_an_owner_command_host() {
    let policy = CommandPolicy::new(names(&["devbox"]), None, Vec::new());
    let cedar = evaluator(&policy);
    assert_eq!(
        cedar
            .evaluate(CommandInitiator::Agent, host("devbox"))
            .unwrap(),
        refused(&["command.agent_host"])
    );
    assert_eq!(
        cedar
            .evaluate(CommandInitiator::Agent, run("devbox", "ls"))
            .unwrap(),
        refused(&["command.agent_host"])
    );
    assert_eq!(
        admit_command(&cedar, CommandInitiator::Agent, "devbox", "ls"),
        Err(CommandRefusal::EnvironmentNotGranted)
    );
}

#[test]
fn command_grant_host_a_grant_uses_only_configured_hosts_it_names_itself() {
    let policy = CommandPolicy::new(names(&["devbox", "build"]), None, Vec::new());
    let cedar = evaluator(&policy);
    let own = on(&["devbox", "elsewhere"]);
    let grant = CommandInitiator::Grant {
        grant: "grant-1",
        policy: &own,
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
    let presets = [
        ToolPreset::ReadOnlyTools,
        ToolPreset::NoShell,
        ToolPreset::AskEverything,
        ToolPreset::All,
    ];
    for preset in presets {
        let policy = ToolPolicy::preset(preset);
        let grant = CommandInitiator::Grant {
            grant: "grant-1",
            policy: &policy,
        };
        assert_eq!(
            cedar.evaluate(grant, host("devbox")).unwrap(),
            refused(&[]),
            "{preset:?}"
        );
        assert_eq!(
            admit_command(&cedar, grant, "devbox", "ls"),
            Err(CommandRefusal::EnvironmentNotGranted),
            "{preset:?}"
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
    let own = on(&["devbox"]);
    let grant = CommandInitiator::Grant {
        grant: "grant-1",
        policy: &own,
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

/// What `run` asks, as a truth table: the owner and three grants (one
/// naming the host, one naming another, one naming none) against a host
/// configured or not and a program allowed, unlisted or denied. A grant
/// meets exactly the owner's answer on the hosts it names, and a refused
/// host everywhere else.
#[test]
fn run_admits_the_owner_on_every_configured_host_and_a_grant_only_on_its_own() {
    let policy = CommandPolicy::new(
        names(&["devbox", "build"]),
        Some(names(&["cargo", "rm"])),
        names(&["rm"]),
    );
    let cedar = evaluator(&policy);
    let (devbox, build, none) = (
        on(&["devbox"]),
        on(&["build"]),
        ToolPolicy::preset(ToolPreset::All),
    );
    let grant = |policy| CommandInitiator::Grant {
        grant: "grant-1",
        policy,
    };
    use CommandRefusal::{CommandDenied as Denied, EnvironmentNotGranted as NotGranted};
    type Row = (&'static str, &'static str, [Result<(), CommandRefusal>; 4]);
    let rows: [Row; 6] = [
        // host, program: owner, grant on devbox, grant on build, grant on none
        (
            "devbox",
            "cargo",
            [Ok(()), Ok(()), Err(NotGranted), Err(NotGranted)],
        ),
        (
            "devbox",
            "sh",
            [Err(Denied), Err(Denied), Err(NotGranted), Err(NotGranted)],
        ),
        (
            "devbox",
            "rm",
            [Err(Denied), Err(Denied), Err(NotGranted), Err(NotGranted)],
        ),
        (
            "devbox",
            "/usr/bin/cargo",
            [Err(Denied), Err(Denied), Err(NotGranted), Err(NotGranted)],
        ),
        (
            "build",
            "cargo",
            [Ok(()), Err(NotGranted), Ok(()), Err(NotGranted)],
        ),
        ("elsewhere", "cargo", [Err(NotGranted); 4]),
    ];
    for (host_name, program, expected) in rows {
        let initiators = [
            CommandInitiator::Owner,
            grant(&devbox),
            grant(&build),
            grant(&none),
        ];
        for (initiator, expected) in initiators.into_iter().zip(expected) {
            assert_eq!(
                admit_command(&cedar, initiator, host_name, program).map(|_| ()),
                expected,
                "{initiator:?} {host_name} {program}"
            );
            assert_eq!(
                matches!(
                    cedar.evaluate(initiator, host(host_name)),
                    Ok(ToolDecision::Allow { .. })
                ),
                expected != Err(NotGranted),
                "{initiator:?} {host_name}"
            );
        }
    }
}
