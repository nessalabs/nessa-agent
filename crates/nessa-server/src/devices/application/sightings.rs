//! When each worker was last seen, and as what: saved, so a worker that is
//! off still shows. Saved as a serving worker reports when its sighting is
//! due, and when its connection ends; at most every [`SAVE_EVERY_MS`] for a
//! worker whose machine has not changed, so many reports write little.
//! Forgotten once its key is no longer paired. Each save and removal runs
//! on a blocking thread, one at a time, and a newer sighting is never
//! replaced by an older one.
use super::WallClock;
use nessa_protocol::lease::MachineReport;
use nessa_sdk::domain::agent_execution::leases::PeerEnvironment;
use std::{
    collections::BTreeMap,
    io,
    sync::{Arc, Mutex, PoisonError},
};

/// How stale a serving worker's saved sighting may grow before it is
/// saved again: a worker that is off was last seen within this of when it
/// is shown to be.
pub(crate) const SAVE_EVERY_MS: u64 = 5 * 60 * 1000;

/// A worker's machine when it was last seen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Sighting {
    pub(crate) machine: MachineReport,
    pub(crate) seen_ms: u64,
}

/// Where sightings are kept. Implemented in infrastructure.
pub(crate) trait SightingStore: Send + Sync {
    /// Every saved sighting. One that cannot be read is left out.
    fn load(&self) -> io::Result<Vec<(PeerEnvironment, Sighting)>>;
    /// Replace `peer`'s sighting.
    fn save(&self, peer: PeerEnvironment, sighting: &Sighting) -> io::Result<()>;
    /// Remove `peer`'s sighting; none is no error.
    fn remove(&self, peer: PeerEnvironment) -> io::Result<()>;
}

/// The saved sightings, read once when the gateway starts and then kept
/// beside the store as each one is saved.
pub(crate) struct WorkerSightings {
    store: Arc<dyn SightingStore>,
    clock: Arc<dyn WallClock>,
    saved: Mutex<BTreeMap<PeerEnvironment, Sighting>>,
    /// Held across each write to the store, so writes land in turn.
    writing: Mutex<()>,
    /// When each forgotten worker was forgotten: a sighting from before
    /// then, saved late, would bring it back.
    forgotten: Mutex<BTreeMap<PeerEnvironment, u64>>,
}

impl WorkerSightings {
    pub(crate) fn new(store: Arc<dyn SightingStore>, clock: Arc<dyn WallClock>) -> Self {
        let saved = store.load().unwrap_or_else(|error| {
            tracing::warn!(?error, "worker sightings could not be read");
            Vec::new()
        });
        Self {
            store,
            clock,
            saved: Mutex::new(saved.into_iter().collect()),
            writing: Mutex::new(()),
            forgotten: Mutex::new(BTreeMap::new()),
        }
    }

    pub(crate) fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }

    pub(crate) fn saved(&self) -> Vec<(PeerEnvironment, Sighting)> {
        self.held()
            .iter()
            .map(|(peer, sighting)| (*peer, sighting.clone()))
            .collect()
    }

    /// `peer` serves as `machine` at `now`: saved when its sighting is due.
    pub(crate) async fn saw(
        self: &Arc<Self>,
        peer: PeerEnvironment,
        machine: &MachineReport,
        now: u64,
    ) {
        let due = match self.held().get(&peer) {
            None => true,
            Some(saved) => {
                !same_machine(&saved.machine, machine)
                    || now >= saved.seen_ms.saturating_add(SAVE_EVERY_MS)
            }
        };
        if due {
            self.keep(peer, machine, now).await;
        }
    }

    /// `peer`'s connection ended at `now`, its last report `machine`:
    /// saved, as of then. Workers connect only to a Unix gateway.
    #[cfg(any(unix, test))]
    pub(crate) async fn left(
        self: &Arc<Self>,
        peer: PeerEnvironment,
        machine: &MachineReport,
        now: u64,
    ) {
        self.keep(peer, machine, now).await;
    }

    /// `peer` is no longer paired: its sighting is removed.
    pub(crate) async fn forget(self: &Arc<Self>, peer: PeerEnvironment) {
        let this = self.clone();
        let now = self.clock.now_ms();
        let _ = tokio::task::spawn_blocking(move || {
            let _writing = this.writing.lock().unwrap_or_else(PoisonError::into_inner);
            this.forgotten
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(peer, now);
            match this.store.remove(peer) {
                Ok(()) => {
                    this.held().remove(&peer);
                }
                Err(error) => tracing::warn!(?error, "a worker sighting could not be removed"),
            }
        })
        .await;
    }

    async fn keep(self: &Arc<Self>, peer: PeerEnvironment, machine: &MachineReport, now: u64) {
        let this = self.clone();
        let sighting = Sighting {
            machine: machine.clone(),
            seen_ms: now,
        };
        let _ = tokio::task::spawn_blocking(move || {
            let _writing = this.writing.lock().unwrap_or_else(PoisonError::into_inner);
            let older = this
                .held()
                .get(&peer)
                .is_some_and(|held| held.seen_ms > sighting.seen_ms);
            let forgotten = this
                .forgotten
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&peer)
                .is_some_and(|at| sighting.seen_ms <= *at);
            if older || forgotten {
                return;
            }
            match this.store.save(peer, &sighting) {
                Ok(()) => {
                    this.held().insert(peer, sighting);
                }
                Err(error) => tracing::warn!(?error, "a worker sighting could not be saved"),
            }
        })
        .await;
    }

    fn held(&self) -> std::sync::MutexGuard<'_, BTreeMap<PeerEnvironment, Sighting>> {
        self.saved.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The same machine, however busy: what it is, not what it is doing.
fn same_machine(left: &MachineReport, right: &MachineReport) -> bool {
    left.hostname == right.hostname
        && left.os == right.os
        && left.os_version == right.os_version
        && left.arch == right.arch
        && left.cpus == right.cpus
        && left.memory_total_bytes == right.memory_total_bytes
        && left.disk_total_bytes == right.disk_total_bytes
}
