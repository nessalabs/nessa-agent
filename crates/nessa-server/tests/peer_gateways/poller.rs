//! The poller's cadence: each wait after a cycle, from its pace, its
//! failures in a row, and one draw of entropy. And the status answer a stop
//! does not drop once the worker has committed an ending, which the audit
//! then records instead of continuing into a read.
use super::super::audit::DurablePeerAudit;
use super::*;
use crate::peer_gateways::application::{
    CacheState, PeerAudit, PeerAuditRecord, PeerAuditUnavailable, PeerState,
};
use futures_util::FutureExt;
use nessa_auth::application::ports::Clock;
use nessa_auth::domain::PrincipalId;
use nessa_auth::domain::{
    pairing::{
        AttemptId, AttemptOutcome, ConsentClass, ConsentIntentId, DisclosedConsent, InvitationId,
        PublicIntent, TerminalCause,
    },
    AudienceId, OrganizationId, Resource, ResourceId,
};
use nessa_protocol::pairing::wire::NativePairingStatus;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

const POLICY: PollPolicy = PollPolicy {
    interval: Duration::from_secs(30),
    backoff_cap: Duration::from_secs(15 * 60),
    read_budget: Duration::from_secs(60),
    reads_at_once: 4,
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
fn a_read_in_flight_wakes_within_a_tick_and_an_idle_poller_waits_its_schedule() {
    assert!(look_again_soon(true, true, 0));
    assert!(look_again_soon(true, true, 30_000));
    // Due, and not started: look again, rather than spin on the same listing.
    assert!(look_again_soon(true, false, 0));
    // Nothing running, and the next peer is in the future: wait that out.
    assert!(!look_again_soon(true, false, 30_000));
    // A list that failed waits the interval, whether or not a read is in flight.
    assert!(!look_again_soon(false, true, 0));
    assert!(!look_again_soon(false, false, 30_000));
}

/// A wake while a read runs is every tick. The directory is scanned when the
/// records change, or once an interval has passed.
#[test]
fn a_schedule_of_no_places_still_reads_one() {
    assert_eq!(read_places(0), 1);
    assert_eq!(read_places(1), 1);
    assert_eq!(read_places(READS_AT_ONCE), READS_AT_ONCE);
}

#[test]
fn a_running_read_lists_when_the_records_change_or_the_interval_passes() {
    let interval = POLICY.interval.as_millis() as u64;
    assert!(!list_peers(false, 0, interval));
    assert!(!list_peers(false, interval - 1, interval));
    assert!(list_peers(false, interval, interval));
    assert!(list_peers(true, 0, interval));
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

/// Issue #266, R3: a peer is `away` only when its relay said it is not
/// registered there, on the status dial or on the read; a relay's
/// `NoAnswer`, a busy peer, stays `unreachable`.
#[test]
fn a_peer_whose_relay_says_it_is_offline_is_listed_away() {
    assert_eq!(sync_state(&Unread::Away), SyncState::Away);
    assert_eq!(
        sync_state(&Unread::Read(ReadFailure::Away)),
        SyncState::Away
    );
    for (refusal, away) in [
        (RelayRefusal::Offline, true),
        (RelayRefusal::NoAnswer, false),
    ] {
        let asked = AskedStatus::dial_failed(refusal.into());
        assert_eq!(asked.away, away, "{refusal:?}");
        assert!(matches!(
            asked.status,
            Some(Err(NativeClientError::Io(
                std::io::ErrorKind::ConnectionRefused
            )))
        ));
    }
    assert!(!AskedStatus::dial_failed(std::io::ErrorKind::TimedOut.into()).away);
}

/// A stop that wins after the status worker has passed its commit point, and
/// before the reply is taken, still keeps that reply's cause on
/// [`AskedStatus`]. The worker blocks until [`asked_status`] has chosen the
/// stop and runs its settle, so the stop wins by the handshake, with no timer
/// deciding the order. Both stops the wait races — the poller's own, and the
/// cycle an owner command stops — take that path, for a denial, a revocation,
/// and an unclaimed attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_after_an_ended_transition_commits_keeps_the_status_cause() {
    let cases = [
        (
            NativePairingStatus::Terminal {
                consent: Box::new(disclosed()),
                cause: TerminalCause::Denied,
            },
            "terminal: denied",
        ),
        (
            NativePairingStatus::Terminal {
                consent: Box::new(disclosed()),
                cause: TerminalCause::CredentialRevoked,
            },
            "terminal: credential_revoked",
        ),
        (
            NativePairingStatus::Unclaimed {
                public: public_intent(),
                outcome: AttemptOutcome::Superseded,
                terminal: None,
            },
            "unclaimed: superseded",
        ),
    ];
    for (pairing, detail) in cases {
        for stop_via_poller in [true, false] {
            let asked = stop_during_status(pairing.clone(), stop_via_poller).await;
            assert!(
                asked.stopped,
                "the stop won while the reply was still pending ({detail}, poller={stop_via_poller})"
            );
            assert_eq!(
                status_cause(asked.transition, asked.status.as_ref()),
                Some(PollerCause::Ended {
                    detail: Some(detail.to_owned())
                }),
                "{detail}, poller={stop_via_poller}"
            );
        }
    }
}

/// The same handshake as the cause test: the stand-in is the status call
/// [`asked_status`] waits on, and it returns only after the stop has won.
async fn stop_during_status(pairing: NativePairingStatus, stop_via_poller: bool) -> AskedStatus {
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
        Ok(pairing)
    });
    let (stop_tx, mut stopped) = watch::channel(false);
    let (cycle_tx, cycle_rx) = tokio::sync::oneshot::channel();
    let settle = async move {
        release_tx
            .send(())
            .expect("the worker is waiting for the reply to be released");
    };
    let waiting = tokio::spawn(async move {
        asked_status(
            async move { worker.await.expect("the status worker finishes") },
            async move {
                let _ = cycle_rx.await;
            },
            &mut stopped,
            settle,
            || (Some(SlotTransition::Ended), Some(SlotOutcome::Landed)),
        )
        .await
    });
    entered_rx
        .await
        .expect("the worker commits before it returns the reply");
    // Hold the stop this iteration does not fire. Dropping the cycle's
    // sender would complete that wait, and the two stops would no longer
    // be distinct.
    let other_stop = if stop_via_poller {
        stop_tx.send(true).expect("the poller stop is signaled");
        Some(cycle_tx)
    } else {
        cycle_tx.send(()).expect("the cycle stop is signaled");
        None
    };
    let asked = tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .expect("the stop settles the reply")
        .expect("the wait task finishes");
    drop(other_stop);
    asked
}

/// A stop after an ending has landed still records that ending's cause, and
/// the cycle's result is `None`, so it does not continue into a read.
#[test]
fn a_stopped_status_records_its_cause_and_does_not_read() {
    let address = "127.0.0.1:9".parse().unwrap();
    let before = PeerHolding {
        record: PeerState::Pending {
            address,
            relay: None,
        },
        cache: CacheState::Absent,
    };
    let after = PeerHolding {
        record: PeerState::Revoked {
            address,
            relay: None,
        },
        cache: CacheState::Absent,
    };
    let recorded = status_record(
        AskedStatus {
            status: Some(Ok((Status::Ended, "terminal: denied".to_owned()))),
            transition: Some(SlotTransition::Ended),
            landed: Some(SlotOutcome::Landed),
            stopped: true,
            away: false,
        },
        &before,
        &after,
    );
    assert_eq!(
        recorded.cause,
        Some(PollerCause::Ended {
            detail: Some("terminal: denied".to_owned())
        })
    );
    assert_eq!(recorded.outcome, Ok(()));
    assert!(recorded.answer.is_none());
}

fn public_intent() -> PublicIntent {
    PublicIntent::new(
        InvitationId::new([1; InvitationId::LENGTH]),
        AttemptId::new([2; AttemptId::LENGTH]),
        ConsentIntentId::new([3; ConsentIntentId::LENGTH]),
        1,
        600_000,
        ConsentClass::PeerRead,
    )
    .unwrap()
}

fn disclosed() -> DisclosedConsent {
    let public = public_intent();
    DisclosedConsent::new(
        public,
        AudienceId::new("gateway").unwrap(),
        public
            .class()
            .grant(Resource::new(
                OrganizationId::new("org").unwrap(),
                ResourceId::new("gateway").unwrap(),
            ))
            .unwrap(),
    )
    .unwrap()
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

/// Two cycles on the reset path each hand over [`RECORDS_PER_CYCLE`] records.
/// A queue of 64, the old depth, refuses that burst while the writer is
/// stalled on the first record. The queue the gateway builds holds every
/// cycle that can run at once plus the owner headroom, and keeps them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_queue_of_64_refuses_two_full_cycles_and_the_sized_queue_keeps_every_cycle_and_an_owner_record(
) {
    let two_cycles = 2 * RECORDS_PER_CYCLE;
    assert!(
        two_cycles > 64,
        "two full cycles no longer overflow the old queue of 64"
    );
    let burst = READS_AT_ONCE * RECORDS_PER_CYCLE + AUDIT_HEADROOM;
    assert!(burst <= AUDIT_QUEUE);
    assert!(
        refused_while_stalled(64, two_cycles).await > 0,
        "a queue of 64 accepted two full cycles"
    );
    assert_eq!(
        refused_while_stalled(AUDIT_QUEUE, burst).await,
        0,
        "a cycle or the owner record was refused while the writer was stalled"
    );
}

/// How many of `count` records a stalled writer of `capacity` refuses.
/// The writer blocks on the first record, so the queue cannot drain.
async fn refused_while_stalled(capacity: usize, count: usize) -> usize {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("audit");
    nessa_local_storage::create_directory(&directory).unwrap();
    let stall = Stall::new();
    let release = Release(stall.clone());
    let audit = DurablePeerAudit::with_writer_hook(
        directory,
        Arc::new(Ticks(AtomicU64::new(1))),
        Arc::new(Manual(AtomicU64::new(0))),
        capacity,
        stall.hook(),
    );
    let queued: Vec<_> = (0..count).map(|_| audit.record(owner_record())).collect();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !stall.reached.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the writer did not take the first record");
    let mut refused = 0;
    for record in queued {
        if matches!(record.now_or_never(), Some(Err(PeerAuditUnavailable))) {
            refused += 1;
        }
    }
    drop(release);
    assert!(
        tokio::time::timeout(
            Duration::from_secs(30),
            audit.drained(Duration::from_secs(20))
        )
        .await
        .is_ok(),
        "the writer did not finish"
    );
    refused
}

/// Blocks the writer on the first record until dropped.
struct Stall {
    reached: AtomicBool,
    open: Mutex<bool>,
    opened: Condvar,
}
impl Stall {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            reached: AtomicBool::new(false),
            open: Mutex::new(false),
            opened: Condvar::new(),
        })
    }
    fn hook(self: &Arc<Self>) -> Arc<dyn Fn(u64) + Send + Sync> {
        let stall = Arc::clone(self);
        Arc::new(move |sequence| {
            if sequence != 1 {
                return;
            }
            stall.reached.store(true, Ordering::SeqCst);
            let mut open = stall.open.lock().unwrap();
            while !*open {
                open = stall.opened.wait(open).unwrap();
            }
        })
    }
}
struct Release(Arc<Stall>);
impl Drop for Release {
    fn drop(&mut self) {
        *self.0.open.lock().unwrap() = true;
        self.0.opened.notify_all();
    }
}
struct Ticks(AtomicU64);
impl Clock for Ticks {
    fn unix_milliseconds(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
}
struct Manual(AtomicU64);
impl nessa_protocol::clock::Clock for Manual {
    fn elapsed_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
fn owner_record() -> PeerAuditRecord {
    PeerAuditRecord::ForgetRequested {
        operation: uuid::Uuid::new_v4(),
        initiator: PrincipalId::new("owner").unwrap(),
        peer: DeviceKey::new([7; 32]),
    }
}
