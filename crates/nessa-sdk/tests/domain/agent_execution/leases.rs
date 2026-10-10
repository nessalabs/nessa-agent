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
        AgentWork, CleanupDecision, EndDecision, EnvironmentRef, Lease, LeaseCleanup,
        LeaseDeadline, LeaseEndCause, LeaseError, LeaseGrants, LeaseId, LeasePhase, LeaseRefusal,
        LeaseRevision, LeaseTerms, LeaseWork, SandboxProfile, SandboxProfiles, SshDestination,
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
    ];
    let messages: std::collections::HashSet<String> =
        errors.iter().map(ToString::to_string).collect();
    assert_eq!(messages.len(), errors.len());
    let error: &dyn std::error::Error = &LeaseError::Busy;
    assert!(error.source().is_none());
}
