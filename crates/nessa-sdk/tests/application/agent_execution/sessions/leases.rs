//! Folding lease records: what is committed and what is read back pass the same
//! domain rule, and a record this build cannot read is kept, not refused.

use super::*;
use crate::domain::agent_execution::leases::{
    AgentWork, CommandExit, CommandOutput, CommandRefusal, CommandTerms, CommandWork,
    EnvironmentRef, LeaseDeadline, LeaseGrants, LeasePhase, LeaseWork, SandboxProfile,
    SshDestination,
};

fn id(value: &str) -> LeaseId {
    LeaseId::new(value).unwrap()
}
fn revision(value: u64) -> LeaseRevision {
    LeaseRevision::new(value).unwrap()
}
fn actor(request: &str) -> ActionContext {
    ActionContext::new("person", "desktop", request).unwrap()
}
fn terms() -> LeaseTerms {
    LeaseTerms {
        environment: EnvironmentRef::Here,
        work: LeaseWork::Agent(AgentWork::new("claude", "sonnet").unwrap()),
        sandbox: SandboxProfile::HarnessDefault,
        grants: LeaseGrants::Opening,
        deadline: LeaseDeadline::UntilEnded,
    }
}
fn issued(lease: &str, number: u64) -> LeaseRecord {
    LeaseRecord::Issued {
        lease: id(lease),
        revision: revision(number),
        terms: terms(),
        actor: actor("send"),
    }
}
fn refused(lease: &str, number: u64) -> LeaseRecord {
    LeaseRecord::Refused {
        lease: id(lease),
        revision: revision(number),
        terms: terms(),
        refusal: LeaseRefusal::SandboxUnavailable,
        actor: actor("send"),
    }
}
fn ending(lease: &str, cause: LeaseEndCause) -> LeaseRecord {
    LeaseRecord::Ending {
        lease: id(lease),
        cause,
        actor: Some(actor("close")),
    }
}
fn ended(lease: &str) -> LeaseRecord {
    LeaseRecord::Ended {
        lease: id(lease),
        cleanup: LeaseCleanup::Confirmed { forced: false },
    }
}
fn unreadable() -> LeaseRecord {
    LeaseRecord::Unreadable {
        kind: "sleeping".into(),
        body: "{}".into(),
    }
}
fn fold(records: &[LeaseRecord]) -> Result<CurrentLease, StorageError> {
    let mut current: Option<CurrentLease> = None;
    for record in records {
        current = Some(CurrentLease::apply(current.as_ref(), record)?);
    }
    Ok(current.expect("at least one record"))
}
fn is_corrupt(result: Result<CurrentLease, StorageError>) -> bool {
    matches!(result, Err(StorageError::Corrupt(_)))
}

#[test]
fn a_whole_lease_folds_to_ended_and_keeps_who_asked() {
    let current = fold(&[
        issued("a", 1),
        ending("a", LeaseEndCause::Closed),
        ended("a"),
    ])
    .unwrap();
    assert_eq!(current.records().len(), 3);
    assert_eq!(current.revision(), Some(LeaseRevision::FIRST));
    assert_eq!(current.issued_by(), Some(&actor("send")));
    assert_eq!(current.ended_by(), Some(&actor("close")));
    assert_eq!(
        current.held().map(Lease::phase),
        Some(LeasePhase::Ended {
            cause: LeaseEndCause::Closed,
            cleanup: LeaseCleanup::Confirmed { forced: false },
        })
    );
}

#[test]
fn an_interrupted_lease_takes_late_evidence_once_and_dropped_events_after_it() {
    let current = fold(&[
        issued("a", 1),
        LeaseRecord::Ending {
            lease: id("a"),
            cause: LeaseEndCause::Stopped,
            actor: None,
        },
        LeaseRecord::Interrupted { lease: id("a") },
        LeaseRecord::EventDropped {
            lease: id("a"),
            turn: ExecutionId::new("turn-1").unwrap(),
            cursor: 7,
        },
        LeaseRecord::CleanupReported {
            lease: id("a"),
            cleanup: LeaseCleanup::NotHeld,
        },
    ])
    .unwrap();
    assert_eq!(current.ended_by(), None);
    let lease = current.held().unwrap();
    assert_eq!(lease.dropped_events(), 1);
    assert_eq!(
        lease.phase(),
        LeasePhase::Interrupted {
            cause: LeaseEndCause::Stopped,
            late_cleanup: Some(LeaseCleanup::NotHeld),
        }
    );
}

#[test]
fn records_the_lease_rules_refuse_are_corrupt() {
    let interrupted = [
        issued("a", 1),
        ending("a", LeaseEndCause::Stopped),
        LeaseRecord::Interrupted { lease: id("a") },
    ];
    let cases: Vec<Vec<LeaseRecord>> = vec![
        // A transition before any issuance.
        vec![ending("a", LeaseEndCause::Stopped)],
        // A record naming another lease.
        vec![issued("a", 1), ending("b", LeaseEndCause::Stopped)],
        // A second end (row L6: the first cause stands, nothing more recorded).
        vec![
            issued("a", 1),
            ending("a", LeaseEndCause::Stopped),
            ending("a", LeaseEndCause::Closed),
        ],
        // Cleanup before any end.
        vec![issued("a", 1), ended("a")],
        // An interrupted lease cannot end.
        [interrupted.as_slice(), &[ended("a")]].concat(),
        // Late evidence for a lease that was never interrupted.
        vec![
            issued("a", 1),
            ending("a", LeaseEndCause::Stopped),
            LeaseRecord::CleanupReported {
                lease: id("a"),
                cleanup: LeaseCleanup::NotHeld,
            },
        ],
        // A drop while events are still accepted.
        vec![
            issued("a", 1),
            LeaseRecord::EventDropped {
                lease: id("a"),
                turn: ExecutionId::new("t").unwrap(),
                cursor: 0,
            },
        ],
        // Interrupting a live lease.
        vec![issued("a", 1), LeaseRecord::Interrupted { lease: id("a") }],
        // A refused lease has nothing after it.
        vec![refused("a", 1), ending("a", LeaseEndCause::Stopped)],
    ];
    for records in cases {
        assert!(is_corrupt(fold(&records)), "{records:?}");
    }
}

#[test]
fn l13_a_second_issuance_while_one_is_live_or_ending_is_corrupt() {
    assert!(is_corrupt(fold(&[issued("a", 1), issued("b", 2)])));
    assert!(is_corrupt(fold(&[
        issued("a", 1),
        ending("a", LeaseEndCause::Stopped),
        issued("b", 2),
    ])));
}

#[test]
fn l16_revisions_follow_one_another_across_refusals_and_replacements() {
    let first = fold(&[
        issued("a", 1),
        ending("a", LeaseEndCause::Stopped),
        ended("a"),
    ])
    .unwrap();
    assert!(is_corrupt(CurrentLease::apply(
        Some(&first),
        &issued("b", 1)
    )));
    assert!(is_corrupt(CurrentLease::apply(
        Some(&first),
        &issued("b", 3)
    )));
    let second = CurrentLease::apply(Some(&first), &refused("b", 2)).unwrap();
    assert_eq!(
        second.state(),
        &CurrentLeaseState::Refused {
            lease: id("b"),
            revision: revision(2),
            refusal: LeaseRefusal::SandboxUnavailable,
        }
    );
    assert_eq!(second.held(), None);
    assert_eq!(second.issued_by(), None);
    let third = CurrentLease::apply(Some(&second), &issued("c", 3)).unwrap();
    assert_eq!(third.records(), &[issued("c", 3)]);
    assert!(is_corrupt(CurrentLease::apply(None, &issued("a", 2))));
}

#[test]
fn an_unreadable_record_makes_the_lease_unreadable_until_the_next_issuance() {
    let live = fold(&[issued("a", 1)]).unwrap();
    // Even over a live lease: a later build may have ended it in a way this
    // build cannot read, so nothing further is checked against it.
    let unreadable_lease = CurrentLease::apply(Some(&live), &unreadable()).unwrap();
    assert_eq!(
        unreadable_lease.state(),
        &CurrentLeaseState::Unreadable {
            kind: "sleeping".into()
        }
    );
    assert_eq!(unreadable_lease.revision(), Some(LeaseRevision::FIRST));
    let absorbed = CurrentLease::apply(Some(&unreadable_lease), &ended("zzz")).unwrap();
    let absorbed = CurrentLease::apply(Some(&absorbed), &unreadable()).unwrap();
    assert_eq!(absorbed.records().len(), 3);
    // The revision it took is not known, so any later one is accepted.
    assert!(is_corrupt(CurrentLease::apply(
        Some(&absorbed),
        &issued("b", 1)
    )));
    let next = CurrentLease::apply(Some(&absorbed), &issued("b", 5)).unwrap();
    assert_eq!(next.revision(), Some(revision(5)));
    assert!(next.held().is_some());
}

#[test]
fn an_unreadable_record_past_its_bound_is_corrupt() {
    for record in [
        LeaseRecord::Unreadable {
            kind: "k".repeat(LeaseRecord::MAX_UNREADABLE_KIND_BYTES + 1),
            body: String::new(),
        },
        LeaseRecord::Unreadable {
            kind: "k".into(),
            body: "b".repeat(LeaseRecord::MAX_UNREADABLE_BODY_BYTES + 1),
        },
    ] {
        assert!(is_corrupt(CurrentLease::apply(None, &record)));
    }
}

#[test]
fn one_lease_keeps_a_bounded_number_of_records() {
    let mut records = vec![
        issued("a", 1),
        ending("a", LeaseEndCause::Stopped),
        ended("a"),
    ];
    while records.len() < CurrentLease::MAX_RECORDS {
        records.push(LeaseRecord::EventDropped {
            lease: id("a"),
            turn: ExecutionId::new("t").unwrap(),
            cursor: records.len() as u64,
        });
    }
    let full = fold(&records).unwrap();
    assert!(is_corrupt(CurrentLease::apply(Some(&full), &records[3])));
}

#[test]
fn a_saved_lease_resumes_with_its_revision() {
    let records = [issued("a", 1), ending("a", LeaseEndCause::Stopped)];
    let resumed = CurrentLease::resume(Some(LeaseRevision::FIRST), &records).unwrap();
    assert_eq!(resumed, fold(&records).unwrap());
    assert!(matches!(
        CurrentLease::resume(Some(revision(2)), &records),
        Err(StorageError::Corrupt(_))
    ));
    assert!(matches!(
        CurrentLease::resume(None, &[]),
        Err(StorageError::Corrupt(_))
    ));
    // Only the latest lease is saved: it begins at the revision it names,
    // which the saved revision must match, and nothing comes before it.
    let later = [issued("b", 3), ending("b", LeaseEndCause::Closed)];
    let resumed = CurrentLease::resume(Some(revision(3)), &later).unwrap();
    assert_eq!(resumed.revision(), Some(revision(3)));
    assert_eq!(resumed.records(), later);
    assert!(matches!(
        CurrentLease::resume(Some(revision(2)), &later),
        Err(StorageError::Corrupt(_))
    ));
    assert!(matches!(
        CurrentLease::resume(Some(revision(1)), &[ending("a", LeaseEndCause::Closed)]),
        Err(StorageError::Corrupt(_))
    ));
    // An unreadable lease keeps the newest revision known before it.
    let resumed = CurrentLease::resume(Some(revision(4)), &[unreadable()]).unwrap();
    assert_eq!(resumed.revision(), Some(revision(4)));
    assert!(matches!(
        CurrentLease::resume(None, &[issued("a", 1), unreadable()]),
        Err(StorageError::Corrupt(_))
    ));
}

fn command_terms() -> CommandTerms {
    CommandTerms {
        environment: EnvironmentRef::Ssh(SshDestination::new("devbox").unwrap()),
        command: CommandWork::new(vec!["ls".into()], None, 1_000).unwrap(),
        sandbox: SandboxProfile::None,
    }
}
fn command_issued(command: &str, parent: &str) -> LeaseRecord {
    LeaseRecord::CommandIssued {
        lease: id(command),
        parent: id(parent),
        terms: command_terms(),
        actor: actor("call-1"),
    }
}
fn command_refused(command: &str, parent: &str) -> LeaseRecord {
    LeaseRecord::CommandRefused {
        lease: id(command),
        parent: id(parent),
        terms: command_terms(),
        refusal: CommandRefusal::CommandDenied,
        actor: actor("call-1"),
    }
}
fn command_ended(command: &str, parent: &str) -> LeaseRecord {
    LeaseRecord::CommandEnded {
        lease: id(command),
        parent: id(parent),
        exit: CommandExit::Exited { code: 0 },
        output: CommandOutput::new(3, 0, 0, "ok\n", ""),
        cleanup: Some(LeaseCleanup::Confirmed { forced: false }),
    }
}
fn live_commands(current: &CurrentLease) -> Vec<&str> {
    current
        .held()
        .unwrap()
        .commands()
        .iter()
        .map(LeaseId::as_str)
        .collect()
}

#[test]
fn l14_a_command_lease_lives_under_its_parent_and_is_kept_only_while_live() {
    let current = fold(&[
        issued("a", 1),
        command_issued("c1", "a"),
        command_refused("c2", "a"),
        command_issued("c3", "a"),
    ])
    .unwrap();
    assert_eq!(live_commands(&current), ["c1", "c3"]);
    // A refusal is evidence in the stream and changes nothing saved.
    assert_eq!(
        current.records(),
        [
            issued("a", 1),
            command_issued("c1", "a"),
            command_issued("c3", "a")
        ]
    );
    let current = CurrentLease::apply(Some(&current), &command_ended("c1", "a")).unwrap();
    assert_eq!(live_commands(&current), ["c3"]);
    assert_eq!(
        current.records(),
        [issued("a", 1), command_issued("c3", "a")]
    );
    // A saved lease with a command still live resumes with it live.
    let resumed = CurrentLease::resume(Some(LeaseRevision::FIRST), current.records()).unwrap();
    assert_eq!(resumed, current);
    // Who issued the lease is still the agent lease's issuer.
    assert_eq!(current.issued_by(), Some(&actor("send")));
}

#[test]
fn l14_the_parent_end_ends_its_commands_and_a_later_command_end_is_late_evidence() {
    let current = fold(&[
        issued("a", 1),
        command_issued("c1", "a"),
        ending("a", LeaseEndCause::Stopped),
    ])
    .unwrap();
    // While the parent ends its command still runs, and may still end.
    assert_eq!(live_commands(&current), ["c1"]);
    let ended_first = fold(&[
        issued("a", 1),
        command_issued("c1", "a"),
        ending("a", LeaseEndCause::Stopped),
        command_ended("c1", "a"),
        ended("a"),
    ])
    .unwrap();
    assert!(ended_first.held().unwrap().commands().is_empty());

    let current = CurrentLease::apply(Some(&current), &ended("a")).unwrap();
    assert!(current.held().unwrap().commands().is_empty());
    assert_eq!(
        current.records(),
        [
            issued("a", 1),
            ending("a", LeaseEndCause::Stopped),
            ended("a")
        ]
    );
    let late = CurrentLease::apply(Some(&current), &command_ended("c1", "a")).unwrap();
    assert_eq!(late, current);

    let interrupted = fold(&[
        issued("a", 1),
        command_issued("c1", "a"),
        ending("a", LeaseEndCause::Closed),
        LeaseRecord::Interrupted { lease: id("a") },
    ])
    .unwrap();
    assert!(interrupted.held().unwrap().commands().is_empty());
    assert!(!interrupted
        .records()
        .iter()
        .any(|record| matches!(record, LeaseRecord::CommandIssued { .. })));
}

#[test]
fn l14_command_records_the_lease_rules_refuse_are_corrupt() {
    let live = fold(&[issued("a", 1), command_issued("c1", "a")]).unwrap();
    let ending_lease = fold(&[issued("a", 1), ending("a", LeaseEndCause::Closed)]).unwrap();
    let refused_lease = fold(&[refused("a", 1)]).unwrap();
    for (prior, record) in [
        (&live, command_issued("c2", "other")),
        (&live, command_refused("c2", "other")),
        (&live, command_ended("c1", "other")),
        (&live, command_issued("c1", "a")),
        (&live, command_issued("a", "a")),
        (&live, command_ended("c2", "a")),
        (&ending_lease, command_issued("c2", "a")),
        (&ending_lease, command_ended("c2", "a")),
        (&refused_lease, command_issued("c2", "a")),
    ] {
        assert!(
            is_corrupt(CurrentLease::apply(Some(prior), &record)),
            "{record:?}"
        );
    }
    assert!(is_corrupt(CurrentLease::apply(
        None,
        &command_issued("c", "a")
    )));
    let mut full = live;
    for n in 2..=Lease::MAX_LIVE_COMMANDS {
        full = CurrentLease::apply(Some(&full), &command_issued(&format!("c{n}"), "a")).unwrap();
    }
    assert!(is_corrupt(CurrentLease::apply(
        Some(&full),
        &command_issued("c9", "a")
    )));
}
