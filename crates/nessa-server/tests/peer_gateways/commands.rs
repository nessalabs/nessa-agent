//! The peer commands' turn as a poller cycle gives it back: an owner command
//! that asks for it never finds it taken with no cycle to stop.
use super::*;
use crate::peer_gateways::application::{PeerAuditFuture, PeerConnectFuture};
use nessa_auth::application::pairing::{GatewayKeyStore, PrivateKeyMaterial};
use nessa_auth::application::ports::Clock as WallClock;
use nessa_auth::domain::AudienceId;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

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
        Arc::new(Nothing),
        Arc::new(Nothing),
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
            OwnerTurn::Busy => busy += 1,
        }
    }
    done.store(true, Ordering::SeqCst);
    cycles.join().unwrap();
    assert_eq!(busy, 0, "the turn was found taken with no cycle to stop");
}
