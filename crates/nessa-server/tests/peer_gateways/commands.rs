//! The peer commands' turn as a poller cycle gives it back: an owner command
//! that asks for it never finds it taken with no cycle to stop; and closing
//! the commands for shutdown leaves no effect without its outcome.
use super::*;
use crate::peer_gateways::application::{
    PeerAudit, PeerAuditFuture, PeerAuditRecord, PeerConnectFuture,
};
use crate::peer_gateways::infrastructure::audit::DurablePeerAudit;
use crate::peer_gateways::infrastructure::{AUDIT_HEADROOM, AUDIT_QUEUE};
use nessa_auth::application::pairing::{ClientPendingStore, GatewayKeyStore, PrivateKeyMaterial};
use nessa_auth::application::ports::Clock as WallClock;
use nessa_auth::domain::AudienceId;
use nessa_protocol::pairing::socket::WAKE_TICK;
use nessa_protocol::relay::{GatewayKey, DIRECT_HEAD_START};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::watch;

struct NoKey;
impl GatewayKeyStore for NoKey {
    fn restore_gateway_key(
        &self,
        _: &AudienceId,
        _: &dyn WallClock,
    ) -> Result<Option<PrivateKeyMaterial>, PrivateStateError> {
        Ok(None)
    }
    fn save_gateway_key(
        &self,
        _: &PrivateKeyMaterial,
        _: &AudienceId,
        _: &dyn WallClock,
    ) -> Result<(), PrivateStateError> {
        Err(PrivateStateError::Unavailable)
    }
}
struct Wall;
impl WallClock for Wall {
    fn unix_milliseconds(&self) -> u64 {
        0
    }
}
struct Still;
impl MonotonicClock for Still {
    fn elapsed_ms(&self) -> u64 {
        0
    }
}
struct Nothing;
impl PeerAudit for Nothing {
    fn record(&self, _: PeerAuditRecord) -> PeerAuditFuture<'static> {
        Box::pin(async { Ok(()) })
    }
}
impl PeerConnector for Nothing {
    fn connect(&self, _: SocketAddr) -> PeerConnectFuture<'_> {
        Box::pin(async { Err(io::ErrorKind::ConnectionRefused.into()) })
    }
    fn relay<'a>(
        &'a self,
        _: &'a nessa_protocol::relay::RelayAddress,
        _: nessa_protocol::relay::GatewayKey,
    ) -> PeerConnectFuture<'a> {
        Box::pin(async { Err(io::ErrorKind::ConnectionRefused.into()) })
    }
}

fn commands(root: &Path) -> Arc<PeerCommands> {
    commands_with(root, Arc::new(Still), Arc::new(Nothing), Arc::new(Nothing))
}

fn commands_with(
    root: &Path,
    clock: Arc<dyn MonotonicClock>,
    audit: Arc<dyn PeerAudit>,
    connector: Arc<dyn PeerConnector>,
) -> Arc<PeerCommands> {
    nessa_local_storage::create_directory(root).unwrap();
    nessa_local_storage::create_directory_beneath(root, Path::new("peers")).unwrap();
    let records = PeerRecords::open(
        root,
        Path::new("peers"),
        Arc::new(NoKey),
        AudienceId::new("gateway").unwrap(),
        Arc::new(Wall),
    )
    .unwrap();
    Arc::new(PeerCommands::new(
        Arc::new(records),
        clock,
        audit,
        connector,
        Arc::new(|| {
            Box::new(nessa_auth::adapters::pairing::OsEntropy) as Box<dyn EnrollmentEntropy>
        }),
    ))
}

/// Cycles of one peer take and give back that peer's turn as fast as they
/// can while an owner command asks for it over and over: each time it finds
/// the turn free or a cycle to stop, never `Busy`, which would mean the turn
/// was still taken after its cycle had been cleared.
#[test]
fn a_cycle_gives_back_the_turn_as_it_clears_itself() {
    let directory = tempfile::tempdir().unwrap();
    let commands = commands(&directory.path().join("root"));
    let key = DeviceKey::new([7; 32]);
    let done = Arc::new(AtomicBool::new(false));
    let cycles = std::thread::spawn({
        let (commands, done) = (commands.clone(), done.clone());
        move || {
            let cycle = CycleStop::new();
            while !done.load(Ordering::SeqCst) {
                drop(commands.try_cycle_turn(key, &cycle));
            }
        }
    });
    let mut busy = 0;
    for _ in 0..200_000 {
        match commands.try_peer_turn(key) {
            OwnerTurn::Taken(permit) => drop(permit),
            OwnerTurn::Cycle(..) => {}
            OwnerTurn::Busy | OwnerTurn::Closed => busy += 1,
        }
    }
    done.store(true, Ordering::SeqCst);
    cycles.join().unwrap();
    assert_eq!(busy, 0, "the turn was found taken with no cycle to stop");
}

/// Test history must not make a later cycle look like the first after a
/// normal turn removal, whether or not that first cycle was explicitly stopped.
#[test]
fn first_cycle_evidence_outlives_removed_turns_and_replacements() {
    let directory = tempfile::tempdir().unwrap();
    let commands = commands(&directory.path().join("root"));
    let mut previous = None;
    for stopped in [false, true] {
        let key = DeviceKey::new([if stopped { 2 } else { 1 }; 32]);
        assert!(commands.first_read_stop(&key).is_none());
        let first = CycleStop::new();
        let turn = commands.try_cycle_turn(key, &first).unwrap();
        let rejected = CycleStop::new();
        assert!(commands.try_cycle_turn(key, &rejected).is_none());
        assert!(Arc::ptr_eq(
            &commands.first_read_stop(&key).unwrap(),
            first.read()
        ));
        assert!(Arc::ptr_eq(
            &commands.active_read_stop(&key).unwrap(),
            first.read()
        ));
        if stopped {
            first.stop();
        }
        drop(turn);
        assert_eq!(peer_turns(&commands), 0, "test history retained a turn");
        assert!(commands.active_read_stop(&key).is_none());
        let replacement = CycleStop::new();
        let turn = commands.try_cycle_turn(key, &replacement).unwrap();
        let saved = commands.first_read_stop(&key).unwrap();
        assert!(Arc::ptr_eq(&saved, first.read()));
        assert_eq!(saved.is_stopped(), stopped);
        assert!(Arc::ptr_eq(
            &commands.active_read_stop(&key).unwrap(),
            replacement.read()
        ));
        assert!(!Arc::ptr_eq(&saved, replacement.read()));
        assert!(commands.first_read_stop(&DeviceKey::new([3; 32])).is_none());
        if let Some((key, token)) = previous {
            assert!(Arc::ptr_eq(
                &commands.first_read_stop(&key).unwrap(),
                &token
            ));
        }
        previous = Some((key, saved));
        drop(turn);
        assert_eq!(peer_turns(&commands), 0);
    }
}

#[tokio::test]
async fn closed_commands_do_not_record_a_cycle_that_never_entered() {
    let directory = tempfile::tempdir().unwrap();
    let commands = commands(&directory.path().join("root"));
    assert!(commands.close(PREEMPT).await);
    let key = DeviceKey::new([3; 32]);
    assert!(commands.try_cycle_turn(key, &CycleStop::new()).is_none());
    assert!(commands.first_read_stop(&key).is_none());
    assert!(commands.active_read_stop(&key).is_none());
    assert_eq!(peer_turns(&commands), 0);
}

/// Two peers' turns are independent, and the record set is a third: a cycle
/// of one peer does not take the other's turn or the record set.
#[test]
fn each_peer_has_its_own_turn_and_the_record_set_is_separate() {
    let directory = tempfile::tempdir().unwrap();
    let commands = commands(&directory.path().join("root"));
    let (left, right) = (DeviceKey::new([1; 32]), DeviceKey::new([2; 32]));
    let (cycle_left, cycle_right) = (CycleStop::new(), CycleStop::new());
    let turn_left = commands.try_cycle_turn(left, &cycle_left).unwrap();
    let turn_right = commands.try_cycle_turn(right, &cycle_right).unwrap();
    assert!(
        matches!(commands.try_peer_turn(left), OwnerTurn::Cycle(..)),
        "the peer's own cycle holds its turn"
    );
    assert!(
        matches!(commands.try_peer_turn(right), OwnerTurn::Cycle(..)),
        "the other peer's cycle holds its own turn"
    );
    assert!(
        commands.try_record_set().is_ok(),
        "a cycle does not hold the record set"
    );
    drop(turn_left);
    assert!(matches!(commands.try_peer_turn(left), OwnerTurn::Taken(_)));
    assert!(
        matches!(commands.try_peer_turn(right), OwnerTurn::Cycle(..)),
        "giving one peer's turn back leaves the other's cycle holding it"
    );
    drop(turn_right);
}

/// A turn nobody holds is removed. A forgotten peer, and a key that was
/// never a peer, do not keep a semaphore for the life of the process.
#[test]
fn a_peer_turn_is_dropped_when_nothing_holds_it() {
    let directory = tempfile::tempdir().unwrap();
    let commands = commands(&directory.path().join("root"));
    let key = DeviceKey::new([9; 32]);
    let OwnerTurn::Taken(permit) = commands.try_peer_turn(key) else {
        panic!("the turn was not free");
    };
    assert_eq!(peer_turns(&commands), 1);
    commands.end_peer_turn(key, permit);
    assert_eq!(peer_turns(&commands), 0, "a free turn was kept");

    let cycle = CycleStop::new();
    let held = commands.try_cycle_turn(key, &cycle).unwrap();
    assert_eq!(peer_turns(&commands), 1);
    drop(held);
    assert_eq!(peer_turns(&commands), 0, "a cycle left its turn behind");
}

/// The cycle gives the turn back while the command still holds the semaphore
/// it cloned. The wait then ends, because the clock is already past it, and
/// that late acquire is discarded. The entry is removed with the clone.
#[tokio::test(flavor = "current_thread")]
async fn a_peer_turn_is_dropped_when_its_wait_ends_after_the_cycle() {
    let directory = tempfile::tempdir().unwrap();
    let clock = Arc::new(Moved(AtomicU64::new(0)));
    let commands = commands_with(
        &directory.path().join("root"),
        clock.clone(),
        Arc::new(Nothing),
        Arc::new(Nothing),
    );
    let key = DeviceKey::new([9; 32]);
    let cycle = CycleStop::new();
    let held = commands.try_cycle_turn(key, &cycle).unwrap();
    let waiting = tokio::spawn({
        let commands = commands.clone();
        async move { commands.peer_turn(key).await }
    });
    cycle.stopped().await;
    drop(held);
    clock.0.store(u64::MAX, Ordering::SeqCst);
    let ended = tokio::time::timeout(Duration::from_secs(2), waiting)
        .await
        .expect("the wait did not end")
        .expect("the wait task finishes");
    assert_eq!(ended.unwrap_err(), PeerError::Busy);
    assert_eq!(
        peer_turns(&commands),
        0,
        "a turn whose wait ended after its cycle was kept"
    );
}

/// The queue holds [`AUDIT_HEADROOM`] owner records behind every poller
/// cycle, so an enroll and a forget can both hand an intent over before
/// either is written. One more command waits for a place while the writer
/// is stalled, and is kept once the writer goes on, rather than being
/// refused at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_owner_record_waits_for_the_headroom() {
    let root = tempfile::tempdir().unwrap();
    let peers = root.path().join("root");
    let directory = root.path().join("audit");
    nessa_local_storage::create_directory(&directory).unwrap();
    let stall = HeadroomStall::new();
    let release = HeadroomRelease(stall.clone());
    let audit = Arc::new(DurablePeerAudit::with_writer_hook(
        directory,
        Arc::new(Wall),
        Arc::new(Still),
        AUDIT_QUEUE,
        stall.hook(),
    ));
    let commands = commands_with(
        peers.as_path(),
        Arc::new(Still),
        audit.clone(),
        Arc::new(Nothing),
    );
    // One record for the writer to block on, then enough to leave exactly
    // the headroom free once that record has been taken out of the queue.
    drop(audit.record(owner_intent()));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !stall.reached.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the writer did not take the first record");
    for _ in 0..(AUDIT_QUEUE - AUDIT_HEADROOM) {
        drop(audit.record(owner_intent()));
    }
    let mut held = Vec::new();
    for _ in 0..AUDIT_HEADROOM {
        let commands = commands.clone();
        held.push(tokio::spawn(
            async move { commands.keep(owner_intent()).await },
        ));
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let extra = tokio::spawn({
        let commands = commands.clone();
        async move { commands.keep(owner_intent()).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !extra.is_finished(),
        "an owner record past the headroom was refused while the writer was stalled"
    );
    drop(release);
    for (index, kept) in held.into_iter().enumerate() {
        assert!(
            kept.await.unwrap(),
            "owner record {index} inside the headroom was not kept"
        );
    }
    assert!(
        extra.await.unwrap(),
        "the owner record past the headroom was not kept"
    );
}

/// Past [`AUDIT_DEADLINE`] the command is not kept, and its place stays until
/// the queued record is answered. Releasing the place at the deadline would
/// let another [`AUDIT_HEADROOM`] records be handed over while those are
/// still queued.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_owner_place_stays_until_the_queued_record_leaves() {
    let root = tempfile::tempdir().unwrap();
    let clock = Arc::new(Moved(AtomicU64::new(0)));
    let audit = Arc::new(CountingPending::default());
    let commands = commands_with(
        &root.path().join("root"),
        clock.clone(),
        audit.clone(),
        Arc::new(Nothing),
    );
    let mut held = Vec::new();
    for _ in 0..AUDIT_HEADROOM {
        let commands = commands.clone();
        held.push(tokio::spawn(
            async move { commands.keep(owner_intent()).await },
        ));
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        while audit.calls.load(Ordering::SeqCst) < AUDIT_HEADROOM
            || commands.audit_room.available_permits() != 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the headroom was not taken");
    clock
        .0
        .store(AUDIT_DEADLINE.as_millis() as u64, Ordering::SeqCst);
    for (index, kept) in held.into_iter().enumerate() {
        let answered = tokio::time::timeout(Duration::from_secs(2), kept)
            .await
            .expect("the deadline did not answer the owner command")
            .unwrap();
        assert!(!answered, "owner record {index} was kept past the deadline");
    }
    assert_eq!(
        commands.audit_room.available_permits(),
        0,
        "the owner place was released while its record was still queued"
    );
    let extra = tokio::spawn({
        let commands = commands.clone();
        async move { commands.keep(owner_intent()).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !extra.is_finished(),
        "another owner record ran while the timed-out ones were still queued"
    );
    assert_eq!(
        audit.calls.load(Ordering::SeqCst),
        AUDIT_HEADROOM,
        "another owner record was handed over while the timed-out ones were still queued"
    );
}

/// Answers nothing. Counts each record handed over.
#[derive(Default)]
struct CountingPending {
    calls: AtomicUsize,
}
impl PeerAudit for CountingPending {
    fn record(&self, _: PeerAuditRecord) -> PeerAuditFuture<'static> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::pending())
    }
}

fn owner_intent() -> PeerAuditRecord {
    PeerAuditRecord::ForgetRequested {
        operation: uuid::Uuid::new_v4(),
        initiator: PrincipalId::new("owner").unwrap(),
        peer: DeviceKey::new([4; 32]),
    }
}

/// Blocks the audit writer on its first record until dropped.
struct HeadroomStall {
    reached: AtomicBool,
    open: Mutex<bool>,
    opened: std::sync::Condvar,
}
impl HeadroomStall {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            reached: AtomicBool::new(false),
            open: Mutex::new(false),
            opened: std::sync::Condvar::new(),
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
struct HeadroomRelease(Arc<HeadroomStall>);
impl Drop for HeadroomRelease {
    fn drop(&mut self) {
        *self.0.open.lock().unwrap() = true;
        self.0.opened.notify_all();
    }
}

fn peer_turns(commands: &Arc<PeerCommands>) -> usize {
    commands
        .peers
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .peers
        .len()
}

/// An audit that keeps every record as it is handed over, and answers an
/// intent only once the test opens it.
struct Gated {
    records: Mutex<Vec<PeerAuditRecord>>,
    open: watch::Sender<bool>,
}
impl PeerAudit for Gated {
    fn record(&self, record: PeerAuditRecord) -> PeerAuditFuture<'static> {
        let intent = matches!(
            record,
            PeerAuditRecord::EnrollRequested { .. } | PeerAuditRecord::ForgetRequested { .. }
        );
        self.records.lock().unwrap().push(record);
        let mut open = self.open.subscribe();
        Box::pin(async move {
            if intent {
                let _ = open.wait_for(|open| *open).await;
            }
            Ok(())
        })
    }
}
/// A connector that counts what it is asked to dial.
struct Dials(std::sync::atomic::AtomicUsize);
impl PeerConnector for Dials {
    fn connect(&self, _: SocketAddr) -> PeerConnectFuture<'_> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(io::ErrorKind::ConnectionRefused.into()) })
    }
    fn relay<'a>(
        &'a self,
        _: &'a nessa_protocol::relay::RelayAddress,
        _: nessa_protocol::relay::GatewayKey,
    ) -> PeerConnectFuture<'a> {
        Box::pin(async { Err(io::ErrorKind::ConnectionRefused.into()) })
    }
}

/// MAJOR 1 of the shutdown review: an enroll and a forget whose intents were
/// handed over before shutdown closes the commands. Closing waits for both;
/// each then finds the turn closed and answers `peer_unavailable` with its
/// outcome kept, having read, dialed and removed nothing. When `close`
/// returns, both outcomes are already with the audit, so closing the audit
/// after it leaves no effect without its outcome.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_admitted_before_shutdown_ends_with_its_outcome_and_no_effect() {
    let directory = tempfile::tempdir().unwrap();
    let audit = Arc::new(Gated {
        records: Mutex::new(Vec::new()),
        open: watch::channel(false).0,
    });
    let dials = Arc::new(Dials(0.into()));
    let commands = commands_with(
        &directory.path().join("root"),
        Arc::new(Still),
        audit.clone(),
        dials.clone(),
    );
    let owner = PrincipalId::new("owner").unwrap();
    let forget = tokio::spawn({
        let (commands, owner) = (commands.clone(), owner.clone());
        async move { commands.forget(DeviceKey::new([5; 32]), &owner).await }
    });
    let enroll = tokio::spawn({
        let (commands, owner) = (commands.clone(), owner.clone());
        async move {
            let code = nessa_auth::adapters::pairing::ManualCode::parse(b"ABCD-2345").unwrap();
            commands
                .enroll("127.0.0.1:9".parse().unwrap(), None, code, &owner)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(30), async {
        while audit.records.lock().unwrap().len() < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();

    // The clock never moves: close returns only once both have ended.
    let close = tokio::spawn({
        let commands = commands.clone();
        async move { commands.close(Duration::from_secs(5)).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!close.is_finished(), "close waits for running commands");
    audit.open.send_replace(true);
    assert!(tokio::time::timeout(Duration::from_secs(30), close)
        .await
        .unwrap()
        .unwrap());
    let finished: Vec<_> = audit
        .records
        .lock()
        .unwrap()
        .iter()
        .filter_map(|record| match record {
            PeerAuditRecord::EnrollFinished {
                before, outcome, ..
            }
            | PeerAuditRecord::ForgetFinished {
                before, outcome, ..
            } => Some((before.clone(), *outcome)),
            _ => None,
        })
        .collect();
    assert_eq!(
        finished,
        [
            (PeerState::NotRead, Err("peer_unavailable")),
            (PeerState::NotRead, Err("peer_unavailable"))
        ],
        "both outcomes kept before close returned, neither read a record"
    );
    assert_eq!(forget.await.unwrap().err(), Some(PeerError::Unavailable));
    assert_eq!(enroll.await.unwrap().err(), Some(PeerError::Unavailable));
    assert_eq!(dials.0.load(Ordering::SeqCst), 0, "nothing dialed");
}

/// A command asked once the commands are closed is refused before its
/// intent: it leaves no record at all, so no intent can be left without its
/// outcome when the audit closes after.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_asked_after_close_is_refused_before_its_intent() {
    let directory = tempfile::tempdir().unwrap();
    let audit = Arc::new(Gated {
        records: Mutex::new(Vec::new()),
        open: watch::channel(true).0,
    });
    let dials = Arc::new(Dials(0.into()));
    let commands = commands_with(
        &directory.path().join("root"),
        Arc::new(Still),
        audit.clone(),
        dials.clone(),
    );
    assert!(commands.close(Duration::from_secs(5)).await);
    let owner = PrincipalId::new("owner").unwrap();
    assert_eq!(
        commands.forget(DeviceKey::new([5; 32]), &owner).await.err(),
        Some(PeerError::Unavailable)
    );
    let code = nessa_auth::adapters::pairing::ManualCode::parse(b"ABCD-2345").unwrap();
    assert_eq!(
        commands
            .enroll("127.0.0.1:9".parse().unwrap(), None, code, &owner)
            .await
            .err(),
        Some(PeerError::Unavailable)
    );
    assert!(audit.records.lock().unwrap().is_empty(), "no intent kept");
    assert_eq!(dials.0.load(Ordering::SeqCst), 0);
}

/// A connect that answers, refused, only once the test opens it.
struct HeldDial {
    dialed: AtomicBool,
    open: watch::Sender<bool>,
}
impl PeerConnector for HeldDial {
    fn connect(&self, _: SocketAddr) -> PeerConnectFuture<'_> {
        self.dialed.store(true, Ordering::SeqCst);
        let mut open = self.open.subscribe();
        Box::pin(async move {
            let _ = open.wait_for(|open| *open).await;
            Err(io::ErrorKind::ConnectionRefused.into())
        })
    }
    fn relay<'a>(
        &'a self,
        _: &'a nessa_protocol::relay::RelayAddress,
        _: nessa_protocol::relay::GatewayKey,
    ) -> PeerConnectFuture<'a> {
        Box::pin(async { Err(io::ErrorKind::ConnectionRefused.into()) })
    }
}

/// A command already holding the turn when the commands close is waited
/// for: its outcome is kept before `close` returns, here when it ends well
/// within `PREEMPT` (the test's clock never moves).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_waits_for_a_command_holding_the_turn_and_keeps_its_outcome() {
    let directory = tempfile::tempdir().unwrap();
    let audit = Arc::new(Gated {
        records: Mutex::new(Vec::new()),
        open: watch::channel(true).0,
    });
    let dial = Arc::new(HeldDial {
        dialed: AtomicBool::new(false),
        open: watch::channel(false).0,
    });
    let commands = commands_with(
        &directory.path().join("root"),
        Arc::new(Still),
        audit.clone(),
        dial.clone(),
    );
    let enroll = tokio::spawn({
        let commands = commands.clone();
        async move {
            let code = nessa_auth::adapters::pairing::ManualCode::parse(b"ABCD-2345").unwrap();
            commands
                .enroll(
                    "127.0.0.1:9".parse().unwrap(),
                    None,
                    code,
                    &PrincipalId::new("owner").unwrap(),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(30), async {
        while !dial.dialed.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    // The enrollment holds the turn and is dialing.
    let close = tokio::spawn({
        let commands = commands.clone();
        async move { commands.close(PREEMPT).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !close.is_finished(),
        "close waits for the command holding the turn"
    );
    dial.open.send_replace(true);
    assert!(tokio::time::timeout(Duration::from_secs(30), close)
        .await
        .unwrap()
        .unwrap());
    let outcomes: Vec<_> = audit
        .records
        .lock()
        .unwrap()
        .iter()
        .filter_map(|record| match record {
            PeerAuditRecord::EnrollFinished { outcome, .. } => Some(*outcome),
            _ => None,
        })
        .collect();
    assert_eq!(
        outcomes,
        [Err("peer_unreachable")],
        "kept before close returned"
    );
    assert_eq!(enroll.await.unwrap().err(), Some(PeerError::Unreachable));
}

/// A gateway key store holding one fixed seed.
struct Seed([u8; 32]);
impl GatewayKeyStore for Seed {
    fn restore_gateway_key(
        &self,
        _: &AudienceId,
        _: &dyn WallClock,
    ) -> Result<Option<PrivateKeyMaterial>, PrivateStateError> {
        Ok(Some(PrivateKeyMaterial::new(zeroize::Zeroizing::new(
            self.0,
        ))))
    }
    fn save_gateway_key(
        &self,
        _: &PrivateKeyMaterial,
        _: &AudienceId,
        _: &dyn WallClock,
    ) -> Result<(), PrivateStateError> {
        Err(PrivateStateError::Unavailable)
    }
}

/// What a [`ThroughRelay`] direct connect does.
#[derive(Clone, Copy, PartialEq)]
enum Direct {
    Connects,
    Refuses,
    NeverAnswers,
}

/// A connector whose address does what `direct` says, and whose relay
/// answers a stream of its own; either is a stream to `listener`. It notes
/// each relay and key it was asked for.
struct ThroughRelay {
    direct: Direct,
    asked: Mutex<Vec<(String, GatewayKey)>>,
    listener: std::net::TcpListener,
}
impl ThroughRelay {
    fn new(direct: Direct) -> Arc<Self> {
        Arc::new(Self {
            direct,
            asked: Mutex::new(Vec::new()),
            listener: std::net::TcpListener::bind("127.0.0.1:0").unwrap(),
        })
    }
    fn asked(&self) -> Vec<(String, GatewayKey)> {
        self.asked.lock().unwrap().clone()
    }
}
impl PeerConnector for ThroughRelay {
    fn connect(&self, _: SocketAddr) -> PeerConnectFuture<'_> {
        let stream = match self.direct {
            Direct::Connects => TcpStream::connect(self.listener.local_addr().unwrap()),
            Direct::Refuses => Err(io::ErrorKind::ConnectionRefused.into()),
            Direct::NeverAnswers => return Box::pin(std::future::pending()),
        };
        Box::pin(async move { stream })
    }
    fn relay<'a>(&'a self, relay: &'a RelayAddress, key: GatewayKey) -> PeerConnectFuture<'a> {
        self.asked
            .lock()
            .unwrap()
            .push((relay.as_str().to_owned(), key));
        let stream = TcpStream::connect(self.listener.local_addr().unwrap());
        Box::pin(async move { stream })
    }
}

/// A readable record for a peer reached at `relay.example:443` as well as
/// its address, the records holding it, and the key a relay is asked for.
fn relayed_record() -> (tempfile::TempDir, Arc<PeerRecords>, PeerRecord, GatewayKey) {
    use nessa_auth::adapters::pairing::{NativeIdentity, OsEntropy};
    use nessa_auth::domain::pairing::{
        AttemptId, ConsentClass, ConsentIntentId, InvitationId, PublicIntent,
    };
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("root");
    nessa_local_storage::create_directory(&root).unwrap();
    nessa_local_storage::create_directory_beneath(&root, Path::new("peers")).unwrap();
    let seed = [9; 32];
    let records = Arc::new(
        PeerRecords::open(
            &root,
            Path::new("peers"),
            Arc::new(Seed(seed)),
            AudienceId::new("gateway").unwrap(),
            Arc::new(Wall),
        )
        .unwrap(),
    );
    let peer = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let pin = peer.public_spki();
    let relay = RelayAddress::try_from("relay.example:443".to_owned()).unwrap();
    let intent = PublicIntent::new(
        InvitationId::new([1; InvitationId::LENGTH]),
        AttemptId::new([2; AttemptId::LENGTH]),
        ConsentIntentId::new([3; ConsentIntentId::LENGTH]),
        1,
        600_000,
        ConsentClass::PeerRead,
    )
    .unwrap();
    records
        .enrolling("127.0.0.1:7443".parse().unwrap(), Some(relay))
        .save_pending(
            &PrivateKeyMaterial::new(zeroize::Zeroizing::new(seed)),
            &pin,
            intent,
            None,
        )
        .unwrap();
    let Ok([PeerEntry::Readable(record)]) = <[PeerEntry; 1]>::try_from(records.list().unwrap())
    else {
        panic!("one readable record");
    };
    let key: GatewayKey = pin[12..].try_into().unwrap();
    (directory, records, *record, key)
}

fn relay_commands(
    records: &Arc<PeerRecords>,
    clock: Arc<dyn MonotonicClock>,
    connector: Arc<ThroughRelay>,
) -> PeerCommands {
    PeerCommands::new(
        records.clone(),
        clock,
        Arc::new(Nothing),
        connector,
        Arc::new(|| {
            Box::new(nessa_auth::adapters::pairing::OsEntropy) as Box<dyn EnrollmentEntropy>
        }),
    )
}

/// A peer's status dial takes its relay leg from the injected connector, as
/// it does its direct one, and asks it for the peer's pinned key; a direct
/// connection that comes first never asks it at all.
#[tokio::test]
async fn a_status_dial_asks_the_injected_connector_for_the_peers_relay() {
    let (_directory, records, record, key) = relayed_record();
    for direct in [Direct::Refuses, Direct::Connects] {
        let connector = ThroughRelay::new(direct);
        let commands = relay_commands(&records, Arc::new(Still), connector.clone());
        let stream = commands.dial_peer(&record).await.unwrap();
        assert_eq!(
            stream.peer_addr().unwrap(),
            connector.listener.local_addr().unwrap()
        );
        if direct == Direct::Connects {
            assert!(
                connector.asked().is_empty(),
                "a direct winner never asks the relay"
            );
        } else {
            assert_eq!(
                connector.asked(),
                vec![("relay.example:443".to_owned(), key)]
            );
        }
    }
}

/// A monotonic clock the test moves.
struct Moved(AtomicU64);
impl MonotonicClock for Moved {
    fn elapsed_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// The direct connect's head start is timed by the injected clock, as the
/// connect deadline is: moving it past the head start starts the relay leg
/// at once, and while it stands still the relay is never asked, however
/// long the direct connect takes on the runtime's own clock.
#[tokio::test]
async fn the_relay_leg_starts_when_the_injected_clock_passes_the_head_start() {
    let (_directory, records, record, key) = relayed_record();
    let connector = ThroughRelay::new(Direct::NeverAnswers);
    let still = relay_commands(&records, Arc::new(Still), connector.clone());
    let waited = tokio::time::timeout(2 * DIRECT_HEAD_START, still.dial_peer(&record)).await;
    assert!(waited.is_err(), "the direct connect still waits");
    assert!(
        connector.asked().is_empty(),
        "the head start has not passed"
    );

    let clock = Arc::new(Moved(AtomicU64::new(0)));
    let moved = relay_commands(&records, clock.clone(), connector.clone());
    let head_start = u64::try_from(DIRECT_HEAD_START.as_millis()).unwrap();
    let dialed = tokio::time::timeout(DIRECT_HEAD_START / 2, async {
        tokio::join!(moved.dial_peer(&record), async {
            tokio::time::sleep(WAKE_TICK).await;
            clock.0.store(head_start, Ordering::SeqCst);
        })
        .0
    })
    .await
    .expect("started by the injected clock, not the runtime's");
    assert_eq!(
        dialed.unwrap().peer_addr().unwrap(),
        connector.listener.local_addr().unwrap()
    );
    assert_eq!(
        connector.asked(),
        vec![("relay.example:443".to_owned(), key)]
    );
}
