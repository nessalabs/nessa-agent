//! The poller's cadence: each wait after a cycle, from its pace, its
//! failures in a row, and one draw of entropy. And the status answer a stop
//! does not drop once the worker has committed an ending.
use super::*;

const POLICY: PollPolicy = PollPolicy {
    interval: Duration::from_secs(30),
    backoff_cap: Duration::from_secs(15 * 60),
    read_budget: Duration::from_secs(60),
};
/// The draws that give the shortest, the middle and the longest spread.
const LOW: u32 = 0;
const MIDDLE: u32 = 200;
const HIGH: u32 = 400;

#[test]
fn a_settled_or_waiting_peer_waits_the_interval_and_clears_its_failures() {
    for pace in [Pace::Settled, Pace::Waiting] {
        assert_eq!(next_wait(&POLICY, pace, 5, MIDDLE), (POLICY.interval, 0));
    }
}

#[test]
fn an_incomplete_read_continues_soon_and_a_stopped_one_keeps_its_failures() {
    assert_eq!(
        next_wait(&POLICY, Pace::Continue, 3, MIDDLE),
        (POLICY.interval / 8, 0)
    );
    assert_eq!(
        next_wait(&POLICY, Pace::Stopped, 3, MIDDLE),
        (POLICY.interval / 8, 3)
    );
}

#[test]
fn failures_double_the_wait_up_to_the_cap() {
    let mut failures = 0;
    let mut waits = Vec::new();
    for _ in 0..8 {
        let (wait, after) = next_wait(&POLICY, Pace::Failed, failures, MIDDLE);
        assert_eq!(after, failures + 1);
        failures = after;
        waits.push(wait.as_secs());
    }
    assert_eq!(waits, [60, 120, 240, 480, 900, 900, 900, 900]);
    // Failures saturate rather than overflow the shift.
    assert_eq!(
        next_wait(&POLICY, Pace::Failed, u32::MAX, MIDDLE),
        (POLICY.backoff_cap, u32::MAX)
    );
}

#[test]
fn jitter_spreads_a_wait_over_80_to_120_percent_and_the_cap_still_holds() {
    let low = next_wait(&POLICY, Pace::Settled, 0, LOW).0;
    let high = next_wait(&POLICY, Pace::Settled, 0, HIGH).0;
    assert_eq!(low, Duration::from_secs(24));
    assert_eq!(high, Duration::from_secs(36));
    // Draws wrap rather than leave the range.
    assert_eq!(next_wait(&POLICY, Pace::Settled, 0, HIGH + 401).0, high);
    for draw in [LOW, MIDDLE, HIGH, u32::MAX] {
        for failures in 0..12 {
            let (wait, _) = next_wait(&POLICY, Pace::Failed, failures, draw);
            assert!(wait <= POLICY.backoff_cap, "{wait:?} after {failures}");
        }
    }
    // At the cap, the longest spread is held to it.
    assert_eq!(
        next_wait(&POLICY, Pace::Failed, 10, HIGH).0,
        POLICY.backoff_cap
    );
    assert_eq!(
        next_wait(&POLICY, Pace::Failed, 10, LOW).0,
        POLICY.backoff_cap.mul_f64(0.8)
    );
}

#[test]
fn the_poller_wakes_for_the_soonest_peer_due_and_within_the_interval() {
    let interval = POLICY.interval.as_millis() as u64;
    assert_eq!(
        next_wake(true, [5_000, 3_000].into_iter(), 1_000, POLICY.interval),
        3_000
    );
    assert_eq!(
        next_wake(true, std::iter::empty(), 1_000, POLICY.interval),
        1_000 + interval
    );
    // A peer already past due is read at once.
    assert_eq!(
        next_wake(true, [500].into_iter(), 1_000, POLICY.interval),
        500
    );
}

#[test]
fn records_that_cannot_be_listed_wait_the_interval_not_a_due_already_past() {
    let interval = POLICY.interval.as_millis() as u64;
    assert_eq!(
        next_wake(false, [500].into_iter(), 1_000, POLICY.interval),
        1_000 + interval
    );
}

#[test]
fn a_failing_entropy_source_draws_the_middle() {
    struct Failing;
    impl nessa_auth::adapters::pairing::RngCore for Failing {
        fn next_u32(&mut self) -> u32 {
            0
        }
        fn next_u64(&mut self) -> u64 {
            0
        }
        fn fill_bytes(&mut self, _: &mut [u8]) {}
        fn try_fill_bytes(
            &mut self,
            _: &mut [u8],
        ) -> Result<(), nessa_auth::adapters::pairing::rand::Error> {
            Err(nessa_auth::adapters::pairing::rand::Error::from(
                std::num::NonZeroU32::new(nessa_auth::adapters::pairing::rand::Error::CUSTOM_START)
                    .unwrap(),
            ))
        }
    }
    impl nessa_auth::adapters::pairing::CryptoRng for Failing {}
    let failing: EnrollmentEntropySource =
        Arc::new(|| Box::new(Failing) as Box<dyn super::super::commands::EnrollmentEntropy>);
    assert_eq!(draw(&failing), MIDDLE);
}

#[test]
fn a_status_change_is_named_by_the_transition_its_store_began() {
    let failed: Result<(Status, String), NativeClientError> =
        Err(NativeClientError::Io(std::io::ErrorKind::BrokenPipe));
    let ended: Result<(Status, String), NativeClientError> =
        Ok((Status::Ended, "terminal: denied".to_owned()));
    // The enrollment ended and its write then failed, whatever it left.
    assert_eq!(
        status_cause(Some(SlotTransition::Ended), Some(&failed)),
        Some(PollerCause::Ended { detail: None })
    );
    // The credential's save failed: still the approval it was.
    assert_eq!(
        status_cause(Some(SlotTransition::Approved), Some(&failed)),
        Some(PollerCause::Approved)
    );
    // No answer came back: an ending begun with nothing that names it.
    assert_eq!(
        status_cause(Some(SlotTransition::Ended), None),
        Some(PollerCause::Ended { detail: None })
    );
    assert_eq!(
        status_cause(Some(SlotTransition::Ended), Some(&ended)),
        Some(PollerCause::Ended {
            detail: Some("terminal: denied".to_owned())
        })
    );
    // A status that began no transition changed nothing.
    assert_eq!(status_cause(None, Some(&ended)), None);
    assert_eq!(status_cause(None, Some(&failed)), None);
}

#[test]
fn a_full_cache_is_listed_quota_and_a_read_that_saved_nothing_unreachable() {
    assert_eq!(
        sync_state(&Unread::Read(ReadFailure::Quota)),
        SyncState::Quota
    );
    assert_eq!(
        sync_state(&Unread::Read(ReadFailure::Unreachable)),
        SyncState::Unreachable
    );
    assert_eq!(
        sync_state(&Unread::Read(ReadFailure::Cache)),
        SyncState::Failed
    );
}

/// A stop that wins after the status worker has passed its commit point, and
/// before the reply is taken, still records that reply's cause. The worker
/// blocks until [`take_status`] has chosen the stop and runs its settle, so
/// the stop wins by the handshake, with no timer deciding the order. Both
/// stops the wait races — the poller's own, and the cycle an owner command
/// stops — take that path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_after_an_ended_transition_commits_keeps_the_status_cause() {
    for stop_via_poller in [true, false] {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = tokio::task::spawn_blocking(move || {
            entered_tx
                .send(())
                .expect("the wait is still observing the commit");
            // Long enough that a missed settle fails the test instead of
            // hanging the blocking pool. The success path is released at once.
            release_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("settle releases the reply");
            Ok((Status::Ended, "terminal: denied".to_owned()))
        });
        let (stop_tx, mut stopped) = watch::channel(false);
        let (cycle_tx, cycle_rx) = tokio::sync::oneshot::channel();
        let settle = async move {
            release_tx
                .send(())
                .expect("the worker is waiting for the reply to be released");
        };
        let waiting = tokio::spawn(async move {
            take_status(
                async move { worker.await.expect("the status worker finishes") },
                async move {
                    let _ = cycle_rx.await;
                },
                &mut stopped,
                settle,
            )
            .await
        });
        entered_rx
            .await
            .expect("the worker commits before it returns the reply");
        // Hold the stop that this iteration does not fire. Dropping the
        // cycle's sender would complete that wait, and the two stops would
        // no longer be distinct.
        let _idle = if stop_via_poller {
            stop_tx.send(true).expect("the poller stop is signaled");
            Some(cycle_tx)
        } else {
            cycle_tx.send(()).expect("the cycle stop is signaled");
            None
        };
        let (reply, stopped_early) = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("the stop settles the reply")
            .expect("the wait task finishes");
        assert!(
            stopped_early,
            "the stop won while the reply was still pending (poller={stop_via_poller})"
        );
        assert_eq!(
            status_cause(Some(SlotTransition::Ended), Some(&reply)),
            Some(PollerCause::Ended {
                detail: Some("terminal: denied".to_owned())
            }),
            "poller={stop_via_poller}"
        );
        drop(_idle);
    }
}

/// A status step's outcome is the record's store's: a transition whose write
/// failed is refused whatever the status read said, and one that landed
/// succeeded though the status then failed.
#[test]
fn a_status_change_outcome_is_what_its_store_did() {
    assert_eq!(status_outcome(Some(SlotOutcome::Landed)), Ok(()));
    assert_eq!(
        status_outcome(Some(SlotOutcome::NotLanded)),
        Err("peer_unavailable")
    );
    // Begun and never returned: not confirmed.
    assert_eq!(status_outcome(None), Err("peer_unavailable"));
}

/// A read whose worker panics after it withdrew a conversation still reports
/// that conversation, so its withdrawal is audited; the read itself is a
/// cache failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_that_panics_still_reports_what_it_withdrew() {
    let (work, withdrawn) = spawn_read(|withdrawn| {
        withdrawn.push("withdrawn".to_owned());
        std::panic::resume_unwind(Box::new("the read failed part way"))
    });
    let joined = work.await;
    assert!(joined.as_ref().is_err_and(JoinError::is_panic));
    assert_eq!(
        read_ended(joined, &withdrawn),
        (Err(ReadFailure::Cache), vec!["withdrawn".to_owned()])
    );
}
