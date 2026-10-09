//! Folding lease records: what is committed and what is read back pass the same
//! domain rule, and a record this build cannot read is kept, not refused.

use super::*;
use crate::domain::agent_execution::leases::{
    AgentWork, EnvironmentRef, LeaseDeadline, LeaseGrants, LeasePhase, LeaseWork, SandboxProfile,
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
