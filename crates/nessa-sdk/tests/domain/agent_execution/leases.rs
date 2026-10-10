//! The lease ordering table ("Lease states and orderings" in
//! `docs/design/runtime-architecture.md`), row by row, on the domain rule alone.
//!
//! ```text
//! issue ──▶ Live ──end──▶ Ending ──cleanup──▶ Ended
//!                           └──interrupt──▶ Interrupted ──cleanup──▶ accounted
//! ```
//! Each test names the row it holds.

use nessa_sdk::domain::agent_execution::{
    leases::{
        AgentWork, CleanupDecision, CommandOutput, CommandWork, EndDecision, EnvironmentRef, Lease,
        LeaseCleanup, LeaseDeadline, LeaseEndCause, LeaseError, LeaseGrants, LeaseId, LeasePhase,
        LeaseRefusal, LeaseRevision, LeaseTerms, LeaseWork, SandboxProfile, SandboxProfiles,
        SshDestination,
    },
    ExecutionError,
};

fn terms(deadline: LeaseDeadline) -> LeaseTerms {
    LeaseTerms {
        environment: EnvironmentRef::Here,
        work: LeaseWork::Agent(AgentWork::new("claude", "sonnet").unwrap()),
        sandbox: SandboxProfile::HarnessDefault,
        grants: LeaseGrants::Opening,
        deadline,
    }
}

fn live(deadline: LeaseDeadline) -> Lease {
    Lease::issue(
        LeaseId::new("lease-1").unwrap(),
        LeaseRevision::FIRST,
        terms(deadline),
    )
}

fn ending(cause: LeaseEndCause) -> Lease {
    let mut lease = live(LeaseDeadline::UntilEnded);
    assert_eq!(lease.end(cause), EndDecision::Began);
    lease
}

fn interrupted() -> Lease {
    let mut lease = ending(LeaseEndCause::Stopped);
    lease.interrupt().unwrap();
    lease
}

const CONFIRMED: LeaseCleanup = LeaseCleanup::Confirmed { forced: false };

#[test]
fn l1_an_issued_lease_is_live_with_exactly_what_was_granted() {
    let granted = terms(LeaseDeadline::UntilEnded);
    let lease = Lease::issue(
        LeaseId::new("lease_A-9").unwrap(),
        LeaseRevision::FIRST,
        granted.clone(),
    );
    assert_eq!(lease.id().as_str(), "lease_A-9");
    assert_eq!(lease.revision(), LeaseRevision::FIRST);
    assert_eq!(lease.terms(), &granted);
    assert_eq!(lease.phase(), LeasePhase::Live);
    assert_eq!(lease.dropped_events(), 0);
    assert!(lease.accepts_events());
    let LeaseWork::Agent(work) = &lease.terms().work;
    assert_eq!((work.agent(), work.model()), ("claude", "sonnet"));
}

#[test]
fn l1_admission_grants_the_profile_both_sides_hold_and_never_what_was_only_asked() {
    let both = SandboxProfiles::HARNESS_DEFAULT.intersect(SandboxProfiles::HARNESS_DEFAULT);
    assert_eq!(
        both.admit(SandboxProfile::HarnessDefault),
        Ok(SandboxProfile::HarnessDefault)
    );
    assert!(both.contains(SandboxProfile::HarnessDefault));
}

#[test]
fn l2_a_profile_one_side_cannot_hold_is_refused_with_its_reason() {
    for (binding, environment) in [
        (SandboxProfiles::NONE, SandboxProfiles::HARNESS_DEFAULT),
        (SandboxProfiles::HARNESS_DEFAULT, SandboxProfiles::NONE),
    ] {
        let both = binding.intersect(environment);
        assert!(!both.contains(SandboxProfile::HarnessDefault));
        assert_eq!(
            both.admit(SandboxProfile::HarnessDefault),
            Err(LeaseRefusal::SandboxUnavailable)
        );
    }
}

#[test]
fn l3_a_renewal_moves_a_live_deadline_later_without_a_new_revision() {
    let mut lease = live(LeaseDeadline::At(100));
    assert_eq!(lease.renew(100), Err(LeaseError::DeadlineNotLater));
    assert_eq!(lease.renew(99), Err(LeaseError::DeadlineNotLater));
    lease.renew(200).unwrap();
    assert_eq!(lease.terms().deadline, LeaseDeadline::At(200));
    assert_eq!(lease.revision(), LeaseRevision::FIRST);
    assert_eq!(lease.phase(), LeasePhase::Live);
}

#[test]
fn l3_only_a_live_lease_with_a_deadline_renews() {
    let mut lease = live(LeaseDeadline::UntilEnded);
    assert_eq!(lease.renew(1), Err(LeaseError::NoDeadline));
    let mut lease = live(LeaseDeadline::At(1));
    lease.end(LeaseEndCause::Stopped);
    assert_eq!(lease.renew(2), Err(LeaseError::NotLive));
}

#[test]
fn l4_a_passed_deadline_begins_ending_as_expired() {
    let mut lease = live(LeaseDeadline::At(100));
    assert_eq!(lease.expire(99), Err(LeaseError::NotDue));
    assert_eq!(lease.expire(100), Ok(EndDecision::Began));
    assert_eq!(
        lease.phase(),
        LeasePhase::Ending {
            cause: LeaseEndCause::Expired
        }
    );
    assert_eq!(lease.expire(200), Err(LeaseError::NotLive));
    assert_eq!(
        live(LeaseDeadline::UntilEnded).expire(u64::MAX),
        Err(LeaseError::NoDeadline)
    );
}

#[test]
fn l5_each_end_cause_is_recorded_first_and_events_still_settle_while_ending() {
    for cause in [
        LeaseEndCause::Stopped,
        LeaseEndCause::Closed,
        LeaseEndCause::Revoked,
        LeaseEndCause::Expired,
        LeaseEndCause::Lost,
    ] {
        let lease = ending(cause);
        assert_eq!(lease.phase(), LeasePhase::Ending { cause });
        assert!(lease.accepts_events());
        assert!(!lease.phase().is_final());
    }
}

#[test]
fn l6_a_later_cause_joins_the_first_and_the_lease_ends_with_the_earliest() {
    let mut lease = ending(LeaseEndCause::Closed);
    assert_eq!(
        lease.end(LeaseEndCause::Expired),
        EndDecision::Joined {
            cause: LeaseEndCause::Closed
        }
    );
    assert_eq!(lease.report_cleanup(CONFIRMED), Ok(CleanupDecision::Ended));
    assert_eq!(
        lease.end(LeaseEndCause::Stopped),
        EndDecision::Joined {
            cause: LeaseEndCause::Closed
        }
    );
    assert_eq!(
        lease.phase(),
        LeasePhase::Ended {
            cause: LeaseEndCause::Closed,
            cleanup: CONFIRMED
        }
    );
    let mut lease = interrupted();
    assert_eq!(
        lease.end(LeaseEndCause::Closed),
        EndDecision::Joined {
            cause: LeaseEndCause::Stopped
        }
    );
}

#[test]
fn l7_cleanup_evidence_ends_an_ending_lease_and_final_states_never_reopen() {
    let mut lease = ending(LeaseEndCause::Stopped);
    let forced = LeaseCleanup::Confirmed { forced: true };
    assert_eq!(lease.report_cleanup(forced), Ok(CleanupDecision::Ended));
    assert!(lease.phase().is_final());
    assert!(!lease.accepts_events());
    assert_eq!(lease.report_cleanup(CONFIRMED), Err(LeaseError::Final));
    assert_eq!(lease.interrupt(), Err(LeaseError::Final));
    assert_eq!(
        live(LeaseDeadline::UntilEnded).report_cleanup(CONFIRMED),
        Err(LeaseError::NotEnding)
    );
}

#[test]
fn l8_a_passed_cleanup_deadline_interrupts_and_later_evidence_accounts_once() {
    let mut lease = interrupted();
    assert_eq!(
        lease.phase(),
        LeasePhase::Interrupted {
            cause: LeaseEndCause::Stopped,
            late_cleanup: None
        }
    );
    assert!(lease.phase().is_final());
    assert_eq!(lease.interrupt(), Err(LeaseError::Final));
    assert_eq!(
        lease.report_cleanup(LeaseCleanup::NotHeld),
        Ok(CleanupDecision::Accounted)
    );
    assert_eq!(
        lease.phase(),
        LeasePhase::Interrupted {
            cause: LeaseEndCause::Stopped,
            late_cleanup: Some(LeaseCleanup::NotHeld)
        }
    );
    assert_eq!(
        lease.report_cleanup(CONFIRMED),
        Err(LeaseError::AlreadyAccounted)
    );
    assert_eq!(
        live(LeaseDeadline::UntilEnded).interrupt(),
        Err(LeaseError::NotEnding)
    );
}

#[test]
fn l9_events_are_dropped_and_counted_only_once_a_lease_stops_accepting_them() {
    let mut lease = live(LeaseDeadline::UntilEnded);
    assert_eq!(lease.drop_event(), Err(LeaseError::EventsAccepted));
    lease.end(LeaseEndCause::Stopped);
    assert_eq!(lease.drop_event(), Err(LeaseError::EventsAccepted));
    lease.report_cleanup(CONFIRMED).unwrap();
    lease.drop_event().unwrap();
    lease.drop_event().unwrap();
    assert_eq!(lease.dropped_events(), 2);
    let mut lease = interrupted();
    lease.drop_event().unwrap();
    assert_eq!(lease.dropped_events(), 1);
}

#[test]
fn l10_a_lost_lease_ends_as_lost_and_reports_cleanup_against_the_ended_lease() {
    let mut lease = ending(LeaseEndCause::Lost);
    assert_eq!(
        lease.report_cleanup(LeaseCleanup::Confirmed { forced: true }),
        Ok(CleanupDecision::Ended)
    );
    assert_eq!(
        lease.phase(),
        LeasePhase::Ended {
            cause: LeaseEndCause::Lost,
            cleanup: LeaseCleanup::Confirmed { forced: true }
        }
    );
}

#[test]
fn l12_an_environment_with_not_held_for_the_lease_ends_it_lost_with_that_evidence() {
    let mut lease = ending(LeaseEndCause::Lost);
    lease.report_cleanup(LeaseCleanup::NotHeld).unwrap();
    assert_eq!(
        lease.phase(),
        LeasePhase::Ended {
            cause: LeaseEndCause::Lost,
            cleanup: LeaseCleanup::NotHeld
        }
    );
}

#[test]
fn l13_no_lease_is_issued_while_the_previous_one_is_live_or_ending() {
    for phase in [
        LeasePhase::Live,
        LeasePhase::Ending {
            cause: LeaseEndCause::Stopped,
        },
    ] {
        assert_eq!(
            Lease::next_revision(Some(phase), Some(LeaseRevision::FIRST)),
            Err(LeaseError::Busy)
        );
    }
}

#[test]
fn l16_a_replacement_after_a_final_lease_takes_the_next_revision() {
    assert_eq!(Lease::next_revision(None, None), Ok(LeaseRevision::FIRST));
    let second = LeaseRevision::new(2).unwrap();
    for phase in [
        LeasePhase::Ended {
            cause: LeaseEndCause::Closed,
            cleanup: CONFIRMED,
        },
        LeasePhase::Interrupted {
            cause: LeaseEndCause::Closed,
            late_cleanup: None,
        },
    ] {
        assert_eq!(
            Lease::next_revision(Some(phase), Some(LeaseRevision::FIRST)),
            Ok(second)
        );
    }
    // A refused lease leaves no phase but still took its revision.
    assert_eq!(Lease::next_revision(None, Some(second)).unwrap().get(), 3);
    let last = LeaseRevision::new(u64::MAX).unwrap();
    assert_eq!(last.next(), None);
    assert_eq!(
        Lease::next_revision(None, Some(last)),
        Err(LeaseError::RevisionsExhausted)
    );
}

#[test]
fn lease_values_refuse_what_no_record_may_carry() {
    assert_eq!(
        LeaseRevision::new(0),
        Err(ExecutionError::InvalidLeaseRevision)
    );
    assert!(LeaseRevision::FIRST < LeaseRevision::new(2).unwrap());
    for bad in ["", "has space", "slash/", "é"] {
        assert_eq!(LeaseId::new(bad), Err(ExecutionError::InvalidLeaseId));
    }
    assert!(LeaseId::new("a".repeat(LeaseId::MAX_BYTES)).is_ok());
    assert_eq!(
        LeaseId::new("a".repeat(LeaseId::MAX_BYTES + 1)),
        Err(ExecutionError::InvalidLeaseId)
    );
    assert!(matches!(
        AgentWork::new(" ", "m"),
        Err(ExecutionError::EmptyValue(_))
    ));
    assert!(matches!(
        AgentWork::new("a", ""),
        Err(ExecutionError::EmptyValue(_))
    ));
    assert!(matches!(
        AgentWork::new("a".repeat(AgentWork::MAX_AGENT_BYTES + 1), "m"),
        Err(ExecutionError::ValueTooLong { .. })
    ));
    assert!(matches!(
        AgentWork::new("a", "m".repeat(AgentWork::MAX_MODEL_BYTES + 1)),
        Err(ExecutionError::ValueTooLong { .. })
    ));
}

#[test]
fn an_ssh_destination_can_never_be_read_as_an_option_or_a_second_word() {
    for refused in [
        "-oProxyCommand=sh",
        "-",
        "@devbox",
        "host name",
        "host;rm",
        "$(id)",
        "host\nx",
        "a`b`",
        "[::1]",
        "",
    ] {
        assert_eq!(
            SshDestination::new(refused),
            Err(ExecutionError::InvalidSshDestination),
            "{refused:?}"
        );
    }
    for accepted in ["devbox", "me@host.example", "dev-box_2", "100.64.0.1"] {
        assert_eq!(SshDestination::new(accepted).unwrap().as_str(), accepted);
    }
    assert!(SshDestination::new("a".repeat(SshDestination::MAX_BYTES)).is_ok());
    assert!(SshDestination::new("a".repeat(SshDestination::MAX_BYTES + 1)).is_err());
}

#[test]
fn every_lease_error_explains_itself() {
    let errors = [
        LeaseError::NotLive,
        LeaseError::NotEnding,
        LeaseError::Final,
        LeaseError::AlreadyAccounted,
        LeaseError::NoDeadline,
        LeaseError::DeadlineNotLater,
        LeaseError::NotDue,
        LeaseError::EventsAccepted,
        LeaseError::Busy,
        LeaseError::RevisionsExhausted,
        LeaseError::CommandsFull,
        LeaseError::DuplicateCommand,
        LeaseError::UnknownCommand,
    ];
    let messages: std::collections::HashSet<String> =
        errors.iter().map(ToString::to_string).collect();
    assert_eq!(messages.len(), errors.len());
    let error: &dyn std::error::Error = &LeaseError::Busy;
    assert!(error.source().is_none());
}

fn command(id: &str) -> LeaseId {
    LeaseId::new(id).unwrap()
}

#[test]
fn l14_a_live_lease_admits_a_bounded_number_of_distinct_commands() {
    let mut lease = live(LeaseDeadline::UntilEnded);
    assert!(lease.commands().is_empty());
    for n in 0..Lease::MAX_LIVE_COMMANDS {
        lease.admit_command(command(&format!("cmd-{n}"))).unwrap();
    }
    assert_eq!(
        lease.admit_command(command("cmd-x")),
        Err(LeaseError::CommandsFull)
    );
    assert_eq!(
        lease.admit_command(command("cmd-0")),
        Err(LeaseError::DuplicateCommand)
    );
    assert_eq!(
        lease.admit_command(command("lease-1")),
        Err(LeaseError::DuplicateCommand)
    );
    lease.end_command(&command("cmd-1")).unwrap();
    assert_eq!(
        lease.end_command(&command("cmd-1")),
        Err(LeaseError::UnknownCommand)
    );
    lease.admit_command(command("cmd-x")).unwrap();
    let live: Vec<&str> = lease.commands().iter().map(LeaseId::as_str).collect();
    assert_eq!(live, ["cmd-0", "cmd-2", "cmd-3", "cmd-x"]);
}

#[test]
fn l14_only_a_live_lease_admits_a_command_and_its_end_ends_them_all() {
    let mut lease = live(LeaseDeadline::UntilEnded);
    lease.admit_command(command("cmd-0")).unwrap();
    assert_eq!(lease.end(LeaseEndCause::Stopped), EndDecision::Began);
    assert_eq!(
        lease.admit_command(command("cmd-1")),
        Err(LeaseError::NotLive)
    );
    // While it ends, a command it already runs is still running.
    assert_eq!(lease.commands().len(), 1);
    lease.report_cleanup(CONFIRMED).unwrap();
    assert!(lease.commands().is_empty());

    let mut lease = ending(LeaseEndCause::Closed);
    let mut with_command = live(LeaseDeadline::UntilEnded);
    with_command.admit_command(command("cmd-0")).unwrap();
    with_command.end(LeaseEndCause::Closed);
    with_command.interrupt().unwrap();
    assert!(with_command.commands().is_empty());
    lease.interrupt().unwrap();
    assert_eq!(
        lease.end_command(&command("cmd-0")),
        Err(LeaseError::UnknownCommand)
    );
}

#[test]
fn a_command_is_an_argument_vector_bounded_so_its_record_always_fits() {
    let ok = |argv: &[&str]| argv.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
    let work = CommandWork::new(
        ok(&["cargo", "test", "-p", "a b"]),
        Some("crates/x".into()),
        1,
    )
    .unwrap();
    assert_eq!(work.program(), "cargo");
    assert_eq!(work.argv().len(), 4);
    assert_eq!(work.cwd(), Some("crates/x"));
    assert_eq!(work.timeout_ms(), 1);
    let work = CommandWork::new(ok(&["ls"]), None, CommandWork::MAX_TIMEOUT_MS).unwrap();
    assert_eq!(work.cwd(), None);
    // Newline and tab are what an argument such as a commit message needs.
    assert!(CommandWork::new(ok(&["git", "commit", "-m", "a\n\tb"]), None, 1).is_ok());

    for argv in [vec![], ok(&[" "]), ok(&["", "x"])] {
        assert_eq!(
            CommandWork::new(argv, None, 1),
            Err(ExecutionError::EmptyValue("command program"))
        );
    }
    assert!(matches!(
        CommandWork::new(vec!["a".into(); CommandWork::MAX_ARGS + 1], None, 1),
        Err(ExecutionError::TooManyValues { .. })
    ));
    assert!(CommandWork::new(vec!["a".into(); CommandWork::MAX_ARGS], None, 1).is_ok());
    assert!(matches!(
        CommandWork::new(
            ok(&["a", &"b".repeat(CommandWork::MAX_ARGV_BYTES)]),
            None,
            1
        ),
        Err(ExecutionError::ValueTooLong { .. })
    ));
    for bad in ["\u{0}", "\r", "\u{1b}[2J", "\u{85}"] {
        assert_eq!(
            CommandWork::new(ok(&["echo", bad]), None, 1),
            Err(ExecutionError::InvalidCommandArgument),
            "{bad:?}"
        );
    }
    for cwd in ["", "/etc", "../up", "a/../b", "./a", "a//b", "a/", "a\nb"] {
        assert_eq!(
            CommandWork::new(ok(&["ls"]), Some(cwd.into()), 1),
            Err(ExecutionError::InvalidPath),
            "{cwd:?}"
        );
    }
    assert!(matches!(
        CommandWork::new(
            ok(&["ls"]),
            Some("a".repeat(CommandWork::MAX_CWD_BYTES + 1)),
            1
        ),
        Err(ExecutionError::ValueTooLong { .. })
    ));
    for timeout in [0, CommandWork::MAX_TIMEOUT_MS + 1] {
        assert_eq!(
            CommandWork::new(ok(&["ls"]), None, timeout),
            Err(ExecutionError::InvalidCommandTimeout)
        );
    }
    const { assert!(CommandWork::DEFAULT_TIMEOUT_MS <= CommandWork::MAX_TIMEOUT_MS) };
}

#[test]
fn a_command_output_keeps_the_last_of_each_stream_as_showable_text() {
    let output = CommandOutput::new(10, 20, 3, "ok\n\tdone", "warn\r\u{1b}[0m");
    assert_eq!(
        (
            output.stdout_bytes(),
            output.stderr_bytes(),
            output.dropped_bytes()
        ),
        (10, 20, 3)
    );
    assert_eq!(output.stdout_tail(), "ok\n\tdone");
    assert_eq!(output.stderr_tail(), "warn\u{fffd}\u{fffd}[0m");

    let long = format!(
        "{}{}",
        "x".repeat(10),
        "é".repeat(CommandOutput::MAX_TAIL_BYTES)
    );
    let output = CommandOutput::new(0, 0, 0, &long, "");
    assert!(output.stdout_tail().len() <= CommandOutput::MAX_TAIL_BYTES);
    assert!(output.stdout_tail().chars().all(|c| c == 'é'));
    assert_eq!(output.stdout_tail().len(), CommandOutput::MAX_TAIL_BYTES);
    // A cut that would split a character starts after it instead.
    let odd = format!("ab{}x", "é".repeat(CommandOutput::MAX_TAIL_BYTES / 2));
    let output = CommandOutput::new(0, 0, 0, "", &odd);
    assert_eq!(
        output.stderr_tail().len(),
        CommandOutput::MAX_TAIL_BYTES - 1
    );
    assert!(output.stderr_tail().ends_with("éx"));
}

#[test]
fn a_command_sandbox_is_only_ever_granted_by_an_environment_that_holds_it() {
    assert!(SandboxProfiles::UNENCLOSED.contains(SandboxProfile::None));
    assert!(!SandboxProfiles::UNENCLOSED.contains(SandboxProfile::HarnessDefault));
    assert!(!SandboxProfiles::HARNESS_DEFAULT.contains(SandboxProfile::None));
    assert_eq!(
        SandboxProfiles::UNENCLOSED.admit(SandboxProfile::None),
        Ok(SandboxProfile::None)
    );
    assert_eq!(
        SandboxProfiles::UNENCLOSED.admit(SandboxProfile::HarnessDefault),
        Err(LeaseRefusal::SandboxUnavailable)
    );
    assert_eq!(
        SandboxProfiles::UNENCLOSED.intersect(SandboxProfiles::HARNESS_DEFAULT),
        SandboxProfiles::NONE
    );
    assert_eq!(
        SandboxProfiles::UNENCLOSED.intersect(SandboxProfiles::UNENCLOSED),
        SandboxProfiles::UNENCLOSED
    );
}
