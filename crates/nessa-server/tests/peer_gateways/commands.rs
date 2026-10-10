//! The peer commands' turn as a poller cycle gives it back: an owner command
//! that asks for it never finds it taken with no cycle to stop; and closing
//! the commands for shutdown leaves no effect without its outcome.
use super::*;
use crate::peer_gateways::application::{PeerAuditFuture, PeerConnectFuture};
use nessa_auth::application::pairing::{GatewayKeyStore, PrivateKeyMaterial};
use nessa_auth::application::ports::Clock as WallClock;
use nessa_auth::domain::AudienceId;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
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
    fn record(&self, _: PeerAuditRecord) -> PeerAuditFuture<'_> {
        Box::pin(async { Ok(()) })
    }
}
impl PeerConnector for Nothing {
    fn connect(&self, _: SocketAddr) -> PeerConnectFuture<'_> {
        Box::pin(async { Err(io::ErrorKind::ConnectionRefused.into()) })
    }
}

fn commands(root: &Path) -> Arc<PeerCommands> {
    commands_with(root, Arc::new(Nothing), Arc::new(Nothing))
}

fn commands_with(
    root: &Path,
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
        Arc::new(Still),
        audit,
        connector,
        Arc::new(|| {
            Box::new(nessa_auth::adapters::pairing::OsEntropy) as Box<dyn EnrollmentEntropy>
        }),
    ))
}

/// Cycles take and give back the turn as fast as they can while an owner
/// command asks for it over and over: each time it finds the turn free or a
/// cycle to stop, never `Busy`, which would mean the turn was still taken
/// after its cycle had been cleared.
#[test]
fn a_cycle_gives_back_the_turn_as_it_clears_itself() {
    let directory = tempfile::tempdir().unwrap();
    let commands = commands(&directory.path().join("root"));
    let done = Arc::new(AtomicBool::new(false));
    let cycles = std::thread::spawn({
        let (commands, done) = (commands.clone(), done.clone());
        move || {
            let cycle = CycleStop::new();
            while !done.load(Ordering::SeqCst) {
                drop(commands.try_cycle_turn(&cycle));
            }
        }
    });
    let mut busy = 0;
    for _ in 0..200_000 {
        match commands.try_owner_turn() {
            OwnerTurn::Taken(permit) => drop(permit),
            OwnerTurn::Cycle(_) => {}
            OwnerTurn::Busy | OwnerTurn::Closed => busy += 1,
        }
    }
    done.store(true, Ordering::SeqCst);
    cycles.join().unwrap();
    assert_eq!(busy, 0, "the turn was found taken with no cycle to stop");
}

/// An audit that keeps every record as it is handed over, and answers an
/// intent only once the test opens it.
struct Gated {
    records: Mutex<Vec<PeerAuditRecord>>,
    open: watch::Sender<bool>,
}
impl PeerAudit for Gated {
    fn record(&self, record: PeerAuditRecord) -> PeerAuditFuture<'_> {
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
    let commands = commands_with(&directory.path().join("root"), audit.clone(), dials.clone());
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
                .enroll("127.0.0.1:9".parse().unwrap(), code, &owner)
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
