//! Listing the hub's devices: [`Devices`] over its ports. `machine.rs` reads
//! a machine ([`MachineProbe`]); [`LiveWorkers`] reads the paired
//! environments' connections; [`PairedKeys`] says which keys are paired
//! now; [`WorkerSightings`] keeps when each was last seen, over a
//! [`SightingStore`].
mod machine;
mod sightings;

pub(crate) use machine::{read_machine, MachineProbe};
#[cfg(test)]
pub(crate) use machine::MACHINE_READ_LIMIT;
#[cfg(test)]
use sightings::SAVE_EVERY_MS;
pub(crate) use sightings::{Sighting, SightingStore, WorkerSightings};

use nessa_protocol::lease::MachineReport;
use nessa_sdk::domain::agent_execution::leases::PeerEnvironment;
use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    sync::{Arc, OnceLock},
};

/// The most workers one list names: online first, then draining, then the
/// most recently seen. Their reports' text is bounded, so this many fit in
/// one ordinary product response with room to spare; the rest are counted.
pub(crate) const MAX_LISTED_WORKERS: usize = 64;

/// One paired environment's connection as it stands now.
pub(crate) struct LiveWorker {
    pub(crate) peer: PeerEnvironment,
    /// Agent leases its connection carries and whether it is draining;
    /// `None` with no connection that serves leases.
    pub(crate) serving: Option<(usize, bool)>,
    /// The latest report on its current connection.
    pub(crate) machine: Option<MachineReport>,
}

/// The paired environments' connections. Implemented in composition over
/// the gateway's paired environments.
pub(crate) trait LiveWorkers: Send + Sync {
    fn live(&self) -> Vec<LiveWorker>;
}

/// Which keys hold an active device credential now: the credential
/// registry's answer. Implemented in composition.
pub(crate) trait PairedKeys: Send + Sync {
    /// `None` when that cannot be read now: every sighting is kept then.
    fn paired(&self) -> Option<BTreeSet<PeerEnvironment>>;
}

/// The process's wall clock, in Unix milliseconds.
pub(crate) trait WallClock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// Where a worker stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum WorkerState {
    /// Its connection serves leases and takes new work.
    Online,
    /// Its connection serves the leases it holds and takes no new ones.
    Draining,
    /// No connection serves leases now.
    Offline,
}

impl WorkerState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Draining => "draining",
            Self::Offline => "offline",
        }
    }
}

/// One worker, as listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Worker {
    pub(crate) peer: PeerEnvironment,
    pub(crate) state: WorkerState,
    pub(crate) agents: usize,
    pub(crate) machine: Option<MachineReport>,
    pub(crate) last_seen_ms: Option<u64>,
}

/// The hub and its workers.
pub(crate) struct DeviceList {
    pub(crate) hub: Option<MachineReport>,
    /// Where the hub's native listener, which workers dial, is bound;
    /// `None` while native linking is off.
    pub(crate) listening: Option<SocketAddr>,
    /// At most [`MAX_LISTED_WORKERS`]: online first, then draining, then
    /// offline, the most recently seen first within each.
    pub(crate) workers: Vec<Worker>,
    /// Workers past [`MAX_LISTED_WORKERS`], not listed.
    pub(crate) omitted: usize,
}

/// The hub's devices.
pub(crate) struct Devices {
    hub: Arc<dyn MachineProbe>,
    workers: Option<(Arc<dyn LiveWorkers>, Arc<dyn PairedKeys>)>,
    sightings: Arc<WorkerSightings>,
    listening: OnceLock<SocketAddr>,
}

impl Devices {
    /// `workers` is `None` on a gateway no environment may dial.
    pub(crate) fn new(
        hub: Arc<dyn MachineProbe>,
        workers: Option<(Arc<dyn LiveWorkers>, Arc<dyn PairedKeys>)>,
        sightings: Arc<WorkerSightings>,
    ) -> Self {
        Self {
            hub,
            workers,
            sightings,
            listening: OnceLock::new(),
        }
    }

    /// The native listener is bound at `address`: said once, when it is.
    pub(crate) fn listening(&self, address: SocketAddr) {
        let _ = self.listening.set(address);
    }

    /// The hub as it reads now, and the workers connected now or seen
    /// before and still paired. A serving worker's sighting is saved again
    /// when it is due; an unpaired one's is forgotten.
    pub(crate) async fn list(&self) -> DeviceList {
        let hub = read_machine(&self.hub).await;
        let Some((live, paired)) = &self.workers else {
            return DeviceList {
                hub,
                listening: self.listening.get().copied(),
                workers: Vec::new(),
                omitted: 0,
            };
        };
        let mut workers = self.merge(live.live(), paired.paired()).await;
        workers.sort_by(|left, right| {
            left.state
                .cmp(&right.state)
                .then(right.last_seen_ms.cmp(&left.last_seen_ms))
                .then(left.peer.cmp(&right.peer))
        });
        let omitted = workers.len().saturating_sub(MAX_LISTED_WORKERS);
        workers.truncate(MAX_LISTED_WORKERS);
        DeviceList {
            hub,
            listening: self.listening.get().copied(),
            workers,
            omitted,
        }
    }

    async fn merge(
        &self,
        live: Vec<LiveWorker>,
        paired: Option<BTreeSet<PeerEnvironment>>,
    ) -> Vec<Worker> {
        let now = self.sightings.now_ms();
        // A worker whose connection serves was admitted as paired now; any
        // other the registry no longer pairs is forgotten, saved or not.
        let unpaired =
            |peer: &PeerEnvironment| paired.as_ref().is_some_and(|paired| !paired.contains(peer));
        let serving: BTreeSet<PeerEnvironment> = live
            .iter()
            .filter(|worker| worker.serving.is_some())
            .map(|worker| worker.peer)
            .collect();
        let mut workers: BTreeMap<PeerEnvironment, Worker> = BTreeMap::new();
        for (peer, sighting) in self.sightings.saved() {
            if !serving.contains(&peer) && unpaired(&peer) {
                self.sightings.forget(peer).await;
                continue;
            }
            let worker = Worker {
                peer,
                state: WorkerState::Offline,
                agents: 0,
                machine: Some(sighting.machine),
                last_seen_ms: Some(sighting.seen_ms),
            };
            workers.insert(peer, worker);
        }
        for worker in live {
            let saved = workers.remove(&worker.peer);
            let listed = match (worker.serving, worker.machine) {
                (Some((agents, draining)), machine) => {
                    if let Some(machine) = &machine {
                        self.sightings.saw(worker.peer, machine, now).await;
                    }
                    let seen = machine.is_some().then_some(now);
                    Worker {
                        peer: worker.peer,
                        state: if draining {
                            WorkerState::Draining
                        } else {
                            WorkerState::Online
                        },
                        agents,
                        last_seen_ms: seen.or(saved.as_ref().and_then(|s| s.last_seen_ms)),
                        machine: machine.or(saved.and_then(|saved| saved.machine)),
                    }
                }
                // Gone and no longer paired: forgotten above, not listed.
                (None, _) if unpaired(&worker.peer) => continue,
                // Connected but not serving, or gone: what was saved, or
                // its last report if none was.
                (None, machine) => saved.unwrap_or(Worker {
                    peer: worker.peer,
                    state: WorkerState::Offline,
                    agents: 0,
                    machine,
                    last_seen_ms: None,
                }),
            };
            workers.insert(listed.peer, listed);
        }
        workers.into_values().collect()
    }
}

#[cfg(test)]
mod tests;
