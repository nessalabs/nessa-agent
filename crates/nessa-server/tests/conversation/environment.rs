//! The lease fence and the records that account for a lease, without a
//! service: admission (rows L1, L2), which events a lease accepts (row L9),
//! and how a lease left by an earlier run is ended (rows L11, L12, L16).
use super::*;
use crate::conversation::infrastructure::in_process_environment;
use crate::conversation_test_support::{Provider, ProviderFactory};
use nessa_sdk::application::agent_execution::executions::{ExecutionEvent, ExecutionUpdate};
use nessa_sdk::application::agent_execution::sessions::StorageError;
use nessa_sdk::domain::agent_execution::{executions::MessageChunk, leases::LeaseRevision};
use std::collections::VecDeque;

struct Script(VecDeque<ExecutionEvent>);
impl ExecutionEventStream for Script {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        let next = self.0.pop_front();
        Box::pin(async move { Ok(next) })
    }
}

fn event(turn: &str) -> ExecutionEvent {
    ExecutionEvent::new(
        ExecutionId::new(turn).unwrap(),
        ExecutionUpdate::Message(MessageChunk::text("x")),
    )
}

fn request(binding: SandboxProfiles) -> LeaseRequest {
    LeaseRequest {
        work: AgentWork::new("claude", "test").unwrap(),
        sandbox: SandboxProfile::HarnessDefault,
        binding,
    }
}

fn binding() -> Arc<dyn AgentProvider> {
    Arc::new(Provider::new(Arc::new(ProviderFactory::default())))
}

fn events(fence: &LeaseFence, turns: &[&str]) -> FencedEvents {
    FencedEvents {
        inner: Box::new(Script(turns.iter().map(|turn| event(turn)).collect())),
        state: fence.state.clone(),
    }
}

#[tokio::test]
async fn l1_the_in_process_environment_grants_the_harness_default_and_the_fence_is_live() {
    let opening = open_lease(
        in_process_environment().as_ref(),
        request(SandboxProfiles::HARNESS_DEFAULT),
        binding(),
    )
    .await;
    assert_eq!(opening.refusal, None);
    assert_eq!(opening.terms.environment, EnvironmentRef::Here);
    assert_eq!(opening.terms.sandbox, SandboxProfile::HarnessDefault);
    assert_eq!(opening.terms.grants, LeaseGrants::Opening);
    assert_eq!(opening.terms.deadline, LeaseDeadline::UntilEnded);
    assert_eq!(opening.fence.state().phase, FencePhase::Live);
    assert_eq!(opening.fence.identity(), binding().identity());
}

#[tokio::test]
async fn l2_a_binding_that_declares_no_profile_is_refused_and_its_fence_opens_nothing() {
    let opening = open_lease(
        in_process_environment().as_ref(),
        request(SandboxProfiles::NONE),
        binding(),
    )
    .await;
    assert_eq!(opening.refusal, Some(LeaseRefusal::SandboxUnavailable));
    assert_eq!(opening.terms.sandbox, SandboxProfile::HarnessDefault);
    assert_eq!(opening.fence.state().phase, FencePhase::Closed);
}

#[tokio::test]
async fn l9_events_settle_while_ending_and_are_dropped_with_evidence_once_closed() {
    let opening = open_lease(
        in_process_environment().as_ref(),
        request(SandboxProfiles::HARNESS_DEFAULT),
        binding(),
    )
    .await;
    let fence = &opening.fence;
    let mut stream = events(fence, &["t1", "t2", "t3", "t4"]);
    assert!(stream.next().await.unwrap().is_some());
    fence.ending();
    // The work being stopped still reports how it settled.
    assert!(stream.next().await.unwrap().is_some());
    fence.close();
    // Ending again does not reopen a closed fence.
    fence.ending();
    assert!(stream.next().await.unwrap().is_none());
    assert_eq!(
        fence.take_drops(),
        [
            LeaseRecord::EventDropped {
                lease: opening.lease.clone(),
                turn: ExecutionId::new("t3").unwrap(),
                cursor: 3,
            },
            LeaseRecord::EventDropped {
                lease: opening.lease.clone(),
                turn: ExecutionId::new("t4").unwrap(),
                cursor: 4,
            },
        ]
    );
    assert!(fence.take_drops().is_empty());
}

#[tokio::test]
async fn l9_a_cursor_counts_every_event_under_the_lease_across_its_sessions() {
    let opening = open_lease(
        in_process_environment().as_ref(),
        request(SandboxProfiles::HARNESS_DEFAULT),
        binding(),
    )
    .await;
    let fence = &opening.fence;
    let mut first = events(fence, &["t1", "t2"]);
    assert!(first.next().await.unwrap().is_some());
    assert!(first.next().await.unwrap().is_some());
    fence.close();
    // A second session under the same lease does not start counting again,
    // so no two drops of one lease share a place.
    let mut second = events(fence, &["t1"]);
    assert!(second.next().await.unwrap().is_none());
    assert_eq!(
        fence.take_drops(),
        [LeaseRecord::EventDropped {
            lease: opening.lease.clone(),
            turn: ExecutionId::new("t1").unwrap(),
            cursor: 3,
        }]
    );
}

#[tokio::test]
async fn l9_drops_past_the_kept_bound_are_counted_and_not_kept_over_the_whole_lease() {
    let opening = open_lease(
        in_process_environment().as_ref(),
        request(SandboxProfiles::HARNESS_DEFAULT),
        binding(),
    )
    .await;
    let fence = &opening.fence;
    fence.close();
    let turns =
        |count: u64| -> Vec<String> { (0..count).map(|index| format!("t{index}")).collect() };
    // Dropped in two batches, as on a close past its deadline: once when the
    // lease is interrupted and again when its late cleanup is recorded.
    let early = turns(MAX_KEPT_DROPS - 1);
    let early: Vec<&str> = early.iter().map(String::as_str).collect();
    assert!(events(fence, &early).next().await.unwrap().is_none());
    let first = fence.take_drops();
    let late = turns(4);
    let late: Vec<&str> = late.iter().map(String::as_str).collect();
    assert!(events(fence, &late).next().await.unwrap().is_none());
    assert_eq!(fence.state().uncounted, 3);
    let second = fence.take_drops();
    assert_eq!(fence.state().uncounted, 0);
    assert_eq!(second.len(), 1);

    // The lease that takes the most other records still has room for every
    // drop kept: none of them is refused when it is recorded.
    let id = || opening.lease.clone();
    let mut records = vec![
        issued(id().as_str(), 1),
        LeaseRecord::Ending {
            lease: id(),
            cause: LeaseEndCause::Closed,
            actor: None,
        },
        LeaseRecord::Interrupted { lease: id() },
    ];
    records.extend(first);
    records.push(LeaseRecord::CleanupReported {
        lease: id(),
        cleanup: LeaseCleanup::Confirmed { forced: false },
    });
    records.extend(second);
    assert_eq!(records.len(), CurrentLease::MAX_RECORDS);
    held(&records);
}

#[test]
fn l7_l8_a_close_that_confirmed_cleanup_but_could_not_save_it_still_ends_the_lease() {
    let confirmed = CloseOutcome { forced: true };
    assert_eq!(confirmed_cleanup(&Ok(confirmed)), Some(confirmed));
    assert_eq!(
        confirmed_cleanup(&Err(AgentError::StorageDuringClose {
            error: StorageError::Io("the disk went away".into()),
            cleanup_result: Box::new(Ok(confirmed)),
        })),
        Some(confirmed)
    );
    assert_eq!(
        confirmed_cleanup(&Err(AgentError::StorageDuringClose {
            error: StorageError::Io("the disk went away".into()),
            cleanup_result: Box::new(Err(AgentError::CleanupUncertain)),
        })),
        None
    );
    assert_eq!(confirmed_cleanup(&Err(AgentError::CleanupUncertain)), None);
}

fn held(records: &[LeaseRecord]) -> CurrentLease {
    let mut current: Option<CurrentLease> = None;
    for record in records {
        current = Some(CurrentLease::apply(current.as_ref(), record).unwrap());
    }
    current.unwrap()
}

fn issued(lease: &str, revision: u64) -> LeaseRecord {
    LeaseRecord::Issued {
        lease: LeaseId::new(lease).unwrap(),
        revision: LeaseRevision::new(revision).unwrap(),
        terms: LeaseTerms {
            environment: EnvironmentRef::Here,
            work: LeaseWork::Agent(AgentWork::new("claude", "test").unwrap()),
            sandbox: SandboxProfile::HarnessDefault,
            grants: LeaseGrants::Opening,
            deadline: LeaseDeadline::UntilEnded,
        },
        actor: ActionContext::new("person", "desktop", "send").unwrap(),
    }
}

#[test]
fn l11_l12_l16_a_lease_an_earlier_run_left_is_accounted_for_from_where_it_stood() {
    let id = || LeaseId::new("old").unwrap();
    let ending = LeaseRecord::Ending {
        lease: id(),
        cause: LeaseEndCause::Stopped,
        actor: None,
    };
    let interrupted = LeaseRecord::Interrupted { lease: id() };
    let ended = LeaseRecord::Ended {
        lease: id(),
        cleanup: LeaseCleanup::NotHeld,
    };
    let cases = [
        // Live: ended as lost, with what the environment says it holds.
        (vec![issued("old", 1)], 2),
        // Ending: the end stands; only the evidence is added.
        (vec![issued("old", 1), ending.clone()], 1),
        // Interrupted: the evidence accounts for it.
        (
            vec![issued("old", 1), ending.clone(), interrupted.clone()],
            1,
        ),
    ];
    for (records, added) in cases {
        let current = held(&records);
        let lease = current.held().unwrap();
        assert!(needs_accounting(lease));
        let accounting = account_earlier(lease, Some(LeaseCleanup::NotHeld));
        assert_eq!(accounting.len(), added);
        let mut folded = Some(current.clone());
        for record in &accounting {
            folded = Some(CurrentLease::apply(folded.as_ref(), record).unwrap());
        }
        assert!(folded.unwrap().held().unwrap().phase().is_final());
        assert_eq!(
            next_revision(Some(&current), &accounting),
            LeaseRevision::new(2).unwrap()
        );
    }
    let live = held(&[issued("old", 1)]);
    assert_eq!(
        account_earlier(live.held().unwrap(), Some(LeaseCleanup::NotHeld))[0],
        LeaseRecord::Ending {
            lease: id(),
            cause: LeaseEndCause::Lost,
            actor: None,
        }
    );
    // Final leases need nothing more.
    for records in [
        vec![issued("old", 1), ending.clone(), ended],
        vec![
            issued("old", 1),
            ending,
            interrupted,
            LeaseRecord::CleanupReported {
                lease: id(),
                cleanup: LeaseCleanup::NotHeld,
            },
        ],
    ] {
        let current = held(&records);
        assert!(!needs_accounting(current.held().unwrap()));
        assert!(account_earlier(current.held().unwrap(), Some(LeaseCleanup::NotHeld)).is_empty());
        assert_eq!(next_revision(Some(&current), &[]).get(), 2);
    }
    assert_eq!(next_revision(None, &[]), LeaseRevision::FIRST);
    // A lease still live with nothing accounting for it gives the revision
    // the fold then refuses, never a fresh one.
    assert_eq!(next_revision(Some(&live), &[]), LeaseRevision::FIRST);
}
