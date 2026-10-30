use super::*;
use std::{
    io,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

fn report(hostname: &str, cpu_permille: u32) -> MachineReport {
    MachineReport {
        hostname: hostname.into(),
        os: "linux".into(),
        os_version: "24.04".into(),
        arch: "x86_64".into(),
        cpus: 8,
        cpu_permille,
        memory_total_bytes: 32,
        memory_used_bytes: 8,
        disk_total_bytes: 100,
        disk_used_bytes: 40,
    }
}

fn peer(byte: u8) -> PeerEnvironment {
    PeerEnvironment::new([byte; PeerEnvironment::KEY_BYTES])
}

struct Probe(Option<MachineReport>);
impl MachineProbe for Probe {
    fn read(&self) -> Option<MachineReport> {
        self.0.clone()
    }
}

/// Each worker: its key, what it serves, its report.
type Connection = (
    PeerEnvironment,
    Option<(usize, bool)>,
    Option<MachineReport>,
);

struct Live(Mutex<Vec<Connection>>);
impl LiveWorkers for Live {
    fn live(&self) -> Vec<LiveWorker> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(peer, serving, machine)| LiveWorker {
                peer: *peer,
                serving: *serving,
                machine: machine.clone(),
            })
            .collect()
    }
}

#[derive(Default)]
struct Store {
    saved: Mutex<Vec<(PeerEnvironment, Sighting)>>,
    saves: AtomicU64,
}
impl SightingStore for Store {
    fn load(&self) -> io::Result<Vec<(PeerEnvironment, Sighting)>> {
        Ok(self.saved.lock().unwrap().clone())
    }
    fn save(&self, peer: PeerEnvironment, sighting: &Sighting) -> io::Result<()> {
        self.saves.fetch_add(1, Ordering::SeqCst);
        let mut saved = self.saved.lock().unwrap();
        saved.retain(|(held, _)| *held != peer);
        saved.push((peer, sighting.clone()));
        Ok(())
    }
}

struct Clock(AtomicU64);
impl WallClock for Clock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct Fixture {
    store: Arc<Store>,
    clock: Arc<Clock>,
    live: Arc<Live>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            store: Arc::default(),
            clock: Arc::new(Clock(AtomicU64::new(1_000))),
            live: Arc::new(Live(Mutex::new(Vec::new()))),
        }
    }

    fn devices(&self, hub: Option<MachineReport>) -> Devices {
        Devices::new(
            Arc::new(Probe(hub)),
            Some(self.live.clone()),
            Arc::new(WorkerSightings::new(self.store.clone(), self.clock.clone())),
        )
    }
}

#[tokio::test]
async fn the_hub_is_read_and_no_worker_is_listed_before_one_dials() {
    let fixture = Fixture::new();
    let list = fixture.devices(Some(report("studio", 230))).list().await;
    assert_eq!(list.hub, Some(report("studio", 230)));
    assert!(list.workers.is_empty());
}

#[tokio::test]
async fn a_hub_report_out_of_bounds_is_not_listed() {
    let fixture = Fixture::new();
    let list = fixture.devices(Some(report("studio", 1_001))).list().await;
    assert_eq!(list.hub, None);
}

#[tokio::test]
async fn a_serving_worker_is_online_with_its_report_and_its_sighting_is_saved() {
    let fixture = Fixture::new();
    *fixture.live.0.lock().unwrap() = vec![
        (peer(1), Some((3, false)), Some(report("build-box", 610))),
        (peer(2), Some((0, true)), Some(report("laptop", 10))),
    ];
    let list = fixture.devices(None).list().await;
    assert_eq!(
        list.workers,
        vec![
            Worker {
                peer: peer(1),
                state: WorkerState::Online,
                agents: 3,
                machine: Some(report("build-box", 610)),
                last_seen_ms: Some(1_000),
            },
            Worker {
                peer: peer(2),
                state: WorkerState::Draining,
                agents: 0,
                machine: Some(report("laptop", 10)),
                last_seen_ms: Some(1_000),
            },
        ]
    );
    assert_eq!(fixture.store.saves.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_worker_seen_before_and_not_connected_is_offline_as_last_seen() {
    let fixture = Fixture::new();
    fixture.store.saved.lock().unwrap().push((
        peer(1),
        Sighting {
            machine: report("gaming-pc", 500),
            seen_ms: 400,
        },
    ));
    *fixture.live.0.lock().unwrap() = vec![(peer(1), None, Some(report("gaming-pc", 900)))];
    let list = fixture.devices(None).list().await;
    assert_eq!(
        list.workers,
        vec![Worker {
            peer: peer(1),
            state: WorkerState::Offline,
            agents: 0,
            machine: Some(report("gaming-pc", 500)),
            last_seen_ms: Some(400),
        }]
    );
}

#[tokio::test]
async fn a_serving_worker_before_its_first_report_keeps_what_was_saved() {
    let fixture = Fixture::new();
    fixture.store.saved.lock().unwrap().push((
        peer(1),
        Sighting {
            machine: report("build-box", 500),
            seen_ms: 400,
        },
    ));
    *fixture.live.0.lock().unwrap() = vec![(peer(1), Some((1, false)), None)];
    let list = fixture.devices(None).list().await;
    assert_eq!(list.workers[0].state, WorkerState::Online);
    assert_eq!(list.workers[0].machine, Some(report("build-box", 500)));
    assert_eq!(list.workers[0].last_seen_ms, Some(400));
    assert_eq!(fixture.store.saves.load(Ordering::SeqCst), 0);
}

#[test]
fn a_sighting_is_saved_again_only_when_the_machine_changes_or_it_is_stale() {
    let fixture = Fixture::new();
    let sightings = WorkerSightings::new(fixture.store.clone(), fixture.clock.clone());
    sightings.saw(peer(1), &report("build-box", 100), 1_000);
    // Busier is the same machine.
    sightings.saw(peer(1), &report("build-box", 900), 2_000);
    assert_eq!(fixture.store.saves.load(Ordering::SeqCst), 1);
    sightings.saw(peer(1), &report("renamed", 900), 3_000);
    assert_eq!(fixture.store.saves.load(Ordering::SeqCst), 2);
    sightings.saw(peer(1), &report("renamed", 900), 3_000 + SAVE_EVERY_MS - 1);
    assert_eq!(fixture.store.saves.load(Ordering::SeqCst), 2);
    sightings.saw(peer(1), &report("renamed", 900), 3_000 + SAVE_EVERY_MS);
    assert_eq!(fixture.store.saves.load(Ordering::SeqCst), 3);
}

#[test]
fn a_connection_that_ends_saves_its_last_sighting_now() {
    let fixture = Fixture::new();
    let sightings = WorkerSightings::new(fixture.store.clone(), fixture.clock.clone());
    sightings.saw(peer(1), &report("build-box", 100), 1_000);
    fixture.clock.0.store(1_500, Ordering::SeqCst);
    sightings.left(peer(1), &report("build-box", 300));
    assert_eq!(
        sightings.saved(),
        vec![(
            peer(1),
            Sighting {
                machine: report("build-box", 300),
                seen_ms: 1_500,
            }
        )]
    );
}
