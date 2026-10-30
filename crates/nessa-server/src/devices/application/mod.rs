//! Listing the hub's devices: [`Devices`] over its ports. `machine.rs` reads
//! a machine ([`MachineProbe`]); [`LiveWorkers`] reads the paired
//! environments' connections; [`WorkerSightings`] keeps when each was last
//! seen, over a [`SightingStore`].
mod machine;
mod sightings;

pub(crate) use machine::MachineProbe;
pub(crate) use sightings::{Sighting, SightingStore, WorkerSightings, SAVE_EVERY_MS};

use nessa_protocol::lease::MachineReport;
use nessa_sdk::domain::agent_execution::leases::PeerEnvironment;
use std::{collections::BTreeMap, sync::Arc};

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

/// The process's wall clock, in Unix milliseconds.
pub(crate) trait WallClock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// Where a worker stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// By environment name.
    pub(crate) workers: Vec<Worker>,
}

/// The hub's devices.
pub(crate) struct Devices {
    hub: Arc<dyn MachineProbe>,
    workers: Option<Arc<dyn LiveWorkers>>,
    sightings: Arc<WorkerSightings>,
}

impl Devices {
    /// `workers` is `None` on a gateway no environment may dial.
    pub(crate) fn new(
        hub: Arc<dyn MachineProbe>,
        workers: Option<Arc<dyn LiveWorkers>>,
        sightings: Arc<WorkerSightings>,
    ) -> Self {
        Self {
            hub,
            workers,
            sightings,
        }
    }

    /// The hub as it reads now, and every worker connected now or seen
    /// before. A serving worker's sighting is saved again when it is due.
    pub(crate) async fn list(&self) -> DeviceList {
        let probe = self.hub.clone();
        let hub = tokio::task::spawn_blocking(move || probe.read())
            .await
            .ok()
            .flatten()
            .filter(MachineReport::is_bounded);
        let live = self
            .workers
            .as_ref()
            .map(|workers| workers.live())
            .unwrap_or_default();
        DeviceList {
            hub,
            workers: self.merge(live),
        }
    }

    fn merge(&self, live: Vec<LiveWorker>) -> Vec<Worker> {
        let now = self.sightings.now_ms();
        let mut workers: BTreeMap<PeerEnvironment, Worker> = self
            .sightings
            .saved()
            .into_iter()
            .map(|(peer, sighting)| {
                let worker = Worker {
                    peer,
                    state: WorkerState::Offline,
                    agents: 0,
                    machine: Some(sighting.machine),
                    last_seen_ms: Some(sighting.seen_ms),
                };
                (peer, worker)
            })
            .collect();
        for worker in live {
            let saved = workers.remove(&worker.peer);
            let listed = match (worker.serving, worker.machine) {
                (Some((agents, draining)), machine) => {
                    if let Some(machine) = &machine {
                        self.sightings.saw(worker.peer, machine, now);
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
