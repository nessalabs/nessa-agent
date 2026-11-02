//! Recovery reserves retained physical ownership against the same live-room port as admission.
use super::*;
use nessa_sdk::application::agent_execution::subagents::{BindResourcesRefusal, LiveRoom};
use nessa_sdk::domain::agent_execution::subagents::KnownMilestone;
use std::sync::atomic::AtomicBool;

struct CountedRoom {
    limit: usize,
    held: AtomicUsize,
    calls: AtomicUsize,
    releases: AtomicUsize,
    panic_at: AtomicUsize,
    panic_after_reserve: AtomicBool,
    panic_release: AtomicBool,
    panic_after_release: AtomicBool,
}

impl CountedRoom {
    fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit,
            held: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            releases: AtomicUsize::new(0),
            panic_at: AtomicUsize::new(0),
            panic_after_reserve: AtomicBool::new(false),
            panic_release: AtomicBool::new(false),
            panic_after_release: AtomicBool::new(false),
        })
    }
}

impl LiveRoom for CountedRoom {
    fn try_reserve(&self) -> bool {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let fault = call == self.panic_at.load(Ordering::SeqCst);
        assert!(
            !fault || self.panic_after_reserve.load(Ordering::SeqCst),
            "room fault before acquisition"
        );
        let mut held = self.held.load(Ordering::SeqCst);
        loop {
            if held == self.limit {
                return false;
            }
            match self
                .held
                .compare_exchange(held, held + 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(actual) => held = actual,
            }
        }
        assert!(!fault, "room fault after acquisition");
        true
    }

    fn release(&self) {
        self.releases.fetch_add(1, Ordering::SeqCst);
        let fault = self.panic_release.load(Ordering::SeqCst);
        assert!(
            !fault || self.panic_after_release.load(Ordering::SeqCst),
            "room fault before release"
        );
        let mut held = self.held.load(Ordering::SeqCst);
        loop {
            let next = held
                .checked_sub(1)
                .expect("cannot release an unreserved slot");
            match self
                .held
                .compare_exchange(held, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(actual) => held = actual,
            }
        }
        assert!(!fault, "room fault after release");
    }
}

fn restore_with(store: Arc<MemoryOwnershipStore>, room: Arc<CountedRoom>) -> World {
    let factory = ScriptFactory::new();
    let audit = ScriptAudit::new();
    let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: store.clone(),
        audit: audit.clone(),
        factory: factory.clone(),
        room,
    });
    World {
        coordinator,
        factory,
        audit,
        store,
    }
}

#[path = "capacity/preparation_recovery.rs"]
mod preparation_recovery;

async fn child(world: &World, root: &AgentLifetimeId, request: &str) -> AgentLifetimeId {
    world
        .coordinator
        .prepare_child(
            world.command(root, request, "retained configuration"),
            SessionId::new(request).unwrap(),
        )
        .await
        .unwrap()
        .child
}

#[tokio::test]
async fn restored_child_reserves_once_reopens_and_releases_capacity_once() {
    let original = World::new(1);
    let root = original.root().await;
    let retained = child(&original, &root, "retained").await;
    let store = original.store.clone();
    drop(original);
    let room = CountedRoom::new(1);
    let restored = restore_with(store, room.clone());
    restored.coordinator.resume().await.unwrap();
    restored.coordinator.resume().await.unwrap();
    assert_eq!(room.calls.load(Ordering::SeqCst), 1);
    assert_eq!(room.held.load(Ordering::SeqCst), 1);
    assert_eq!(
        restored.factory.prepares(),
        0,
        "resume never replays preparation"
    );
    let resources = Arc::new(ScriptResources {
        closes: AtomicUsize::new(0),
        report: released(),
        hold: None,
    });
    restored
        .coordinator
        .bind_resources(retained.clone(), resources.clone())
        .unwrap();
    assert!(!restored
        .coordinator
        .participation(&retained)
        .unwrap()
        .is_sealed());
    assert_eq!(
        restored
            .coordinator
            .spawn(restored.command(&root, "blocked", "task"))
            .await,
        Err(OwnershipFailure::Domain(OwnershipError::NoRoom))
    );
    assert_eq!(restored.factory.prepares(), 0);
    restored.close(&retained).await.unwrap();
    restored.close(&retained).await.unwrap();
    assert_eq!(resources.closes.load(Ordering::SeqCst), 1);
    assert_eq!(room.releases.load(Ordering::SeqCst), 1);
    assert_eq!(room.held.load(Ordering::SeqCst), 0);
    restored.coordinator.resume().await.unwrap();
    assert_eq!(room.held.load(Ordering::SeqCst), 0);
    assert_eq!(room.releases.load(Ordering::SeqCst), 1);
    child(&restored, &root, "replacement").await;
    assert_eq!(room.held.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn insufficient_restored_capacity_rolls_back_only_new_reservations() {
    let original = World::new(2);
    let root = original.root().await;
    child(&original, &root, "first").await;
    let first_snapshot = original.store.read().await.unwrap();
    child(&original, &root, "second").await;
    let both = original.store.read().await.unwrap();
    let store = original.store.clone();
    drop(original);
    let room = CountedRoom::new(1);
    let restored = restore_with(store.clone(), room.clone());
    assert_eq!(
        restored.coordinator.resume().await,
        Err(OwnershipFailure::Domain(OwnershipError::NoRoom))
    );
    assert_eq!(room.held.load(Ordering::SeqCst), 0);
    assert_eq!(room.releases.load(Ordering::SeqCst), 1);
    assert_eq!(restored.coordinator.lifetime_state(&root), None);
    assert_eq!(store.read().await.unwrap(), both);
    store.write(&first_snapshot).await.unwrap();
    restored.coordinator.resume().await.unwrap();
    assert_eq!(room.held.load(Ordering::SeqCst), 1);
    store.write(&both).await.unwrap();
    assert_eq!(
        restored.coordinator.resume().await,
        Err(OwnershipFailure::Domain(OwnershipError::NoRoom))
    );
    assert_eq!(room.held.load(Ordering::SeqCst), 1);
    assert_eq!(room.releases.load(Ordering::SeqCst), 1);
    assert_eq!(
        restored
            .coordinator
            .children(&root, None, 10)
            .unwrap()
            .children
            .len(),
        1
    );
}

#[tokio::test]
async fn panicked_restoration_rolls_back_observed_acquisitions_without_installing() {
    let original = World::new(2);
    let root = original.root().await;
    child(&original, &root, "first").await;
    child(&original, &root, "second").await;
    let store = original.store.clone();
    drop(original);
    let room = CountedRoom::new(2);
    room.panic_at.store(2, Ordering::SeqCst);
    let restored = restore_with(store, room.clone());
    assert_eq!(
        restored.coordinator.resume().await,
        Err(OwnershipFailure::Startup(PortFailure::Uncertain))
    );
    assert_eq!(room.held.load(Ordering::SeqCst), 0);
    assert_eq!(room.releases.load(Ordering::SeqCst), 1);
    assert_eq!(restored.coordinator.lifetime_state(&root), None);
    room.panic_at.store(0, Ordering::SeqCst);
    restored.coordinator.resume().await.unwrap();
    assert_eq!(room.held.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn physical_release_with_evidence_debt_does_not_reserve_on_restart() {
    let original = World::new(1);
    let root = original.root().await;
    // A genuine cleanup result releases capacity even when evidence remains owed.
    original
        .factory
        .reports
        .lock()
        .unwrap()
        .push_back(ResourceReport {
            physical: PhysicalFact::Released,
            evidence: EvidenceFact::Failed,
        });
    let retained = child(&original, &root, "released").await;
    assert!(original.close(&retained).await.is_err());
    assert_eq!(
        original.coordinator.lifetime_state(&retained),
        Some(LifetimeState::Closing)
    );
    let store = original.store.clone();
    drop(original);
    let room = CountedRoom::new(1);
    let restored = restore_with(store, room.clone());
    restored.coordinator.resume().await.unwrap();
    assert_eq!(room.calls.load(Ordering::SeqCst), 0);
    assert!(restored
        .coordinator
        .bind_resources(
            retained,
            Arc::new(ScriptResources {
                closes: AtomicUsize::new(0),
                report: released(),
                hold: None,
            })
        )
        .is_err());
    child(&restored, &root, "replacement").await;
    assert_eq!(room.held.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn refused_history_cannot_release_prior_reservations() {
    let original = World::new(1);
    let root = original.root().await;
    child(&original, &root, "retained").await;
    let store = original.store.clone();
    drop(original);
    let room = CountedRoom::new(1);
    let restored = restore_with(store.clone(), room.clone());
    restored.coordinator.resume().await.unwrap();
    let mut corrupt = store.read().await.unwrap();
    // Duplicate identity invalidates the snapshot; its apparent disappearance of
    // children cannot authorize releasing the already-held reservation.
    corrupt.spawns.clear();
    corrupt.lifetimes.push(corrupt.lifetimes[0].clone());
    store.write(&corrupt).await.unwrap();
    restored.coordinator.resume().await.unwrap();
    assert_eq!(room.held.load(Ordering::SeqCst), 1);
    assert_eq!(room.releases.load(Ordering::SeqCst), 0);
    assert!(restored
        .coordinator
        .participation(&root)
        .unwrap()
        .is_sealed());
    assert!(restored
        .coordinator
        .spawn(restored.command(&root, "refused", "task"))
        .await
        .is_err());
    assert_eq!(restored.factory.prepares(), 0);
}

#[tokio::test]
async fn lost_reservation_ack_never_releases_the_unknown_acquisition() {
    let original = World::new(2);
    let root = original.root().await;
    child(&original, &root, "first").await;
    child(&original, &root, "second").await;
    let room = CountedRoom::new(2);
    room.panic_at.store(2, Ordering::SeqCst);
    room.panic_after_reserve.store(true, Ordering::SeqCst);
    let restored = restore_with(original.store.clone(), room.clone());
    assert_eq!(
        restored.coordinator.resume().await,
        Err(OwnershipFailure::Startup(PortFailure::Uncertain))
    );
    assert_eq!(
        room.held.load(Ordering::SeqCst),
        1,
        "unknown acquisition remains conservative debt"
    );
    assert_eq!(
        room.releases.load(Ordering::SeqCst),
        1,
        "only observed acquisition is rolled back"
    );
    assert_eq!(restored.coordinator.lifetime_state(&root), None);
}

#[tokio::test]
async fn rollback_release_fault_is_uncertain_without_an_extra_release() {
    let original = World::new(2);
    let root = original.root().await;
    child(&original, &root, "first").await;
    child(&original, &root, "second").await;
    for after_effect in [false, true] {
        let room = CountedRoom::new(1);
        room.panic_release.store(true, Ordering::SeqCst);
        room.panic_after_release
            .store(after_effect, Ordering::SeqCst);
        let restored = restore_with(original.store.clone(), room.clone());
        assert_eq!(
            restored.coordinator.resume().await,
            Err(OwnershipFailure::Startup(PortFailure::Uncertain))
        );
        assert_eq!(room.held.load(Ordering::SeqCst), usize::from(!after_effect));
        assert_eq!(room.releases.load(Ordering::SeqCst), 1);
        assert_eq!(restored.coordinator.lifetime_state(&root), None);
    }
}

#[tokio::test]
async fn preparation_absence_is_free_but_uncertain_preparation_retains_capacity() {
    for failure in [PortFailure::Rejected, PortFailure::Uncertain] {
        let original = World::new(1);
        let root = original.root().await;
        *original.factory.fail.lock().unwrap() = Some(PrepareFailure {
            failure,
            cleanup: None,
        });
        assert!(original
            .coordinator
            .prepare_child(
                original.command(&root, "failed", "task"),
                SessionId::new("failed").unwrap()
            )
            .await
            .is_err());
        let snapshot = original.store.read().await.unwrap();
        let graph = OwnershipGraph::restore(snapshot.clone());
        assert!(graph.refusal().is_none());
        let absent = graph.has_absence(&snapshot.spawns[0].child_lifetime);
        assert_eq!(absent, failure == PortFailure::Rejected);
        let room = CountedRoom::new(1);
        let restored = restore_with(original.store.clone(), room.clone());
        restored.coordinator.resume().await.unwrap();
        assert_eq!(room.held.load(Ordering::SeqCst), usize::from(!absent));
        assert_eq!(restored.factory.prepares(), 0);
    }
}

#[tokio::test]
async fn ended_before_preparation_is_not_restored_as_a_live_reservation() {
    // Exercise the existing domain's terminal pre-effect admission, with no
    // factory handoff or resource claim. Open lifetime identity is retained.
    let original = World::new(1);
    let root = original.root().await;
    let mut graph = OwnershipGraph::restore(original.store.read().await.unwrap());
    let request = SpawnRequestId::new("ended-before-prepare").unwrap();
    let lifetime = AgentLifetimeId::new("ended-child").unwrap();
    let _admission = graph
        .admit_spawn(SpawnAdmission {
            child_lifetime: lifetime.clone(),
            child_session: SessionId::new("ended-session").unwrap(),
            binding: binding(&root, &request, "configuration"),
            live_room: true,
        })
        .unwrap();
    let _failure = graph
        .advance_spawn(
            &request,
            SpawnProgress::StartupFailed {
                known: KnownMilestone::Reserved,
            },
        )
        .unwrap();
    let _ended = graph
        .advance_spawn(
            &request,
            SpawnProgress::Ended {
                known: KnownMilestone::Reserved,
            },
        )
        .unwrap();
    original.store.write(&graph.snapshot()).await.unwrap();
    let room = CountedRoom::new(1);
    let restored = restore_with(original.store.clone(), room.clone());
    restored.coordinator.resume().await.unwrap();
    assert_eq!(
        restored.coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Open)
    );
    assert_eq!(room.held.load(Ordering::SeqCst), 0);
    let refused = restored
        .coordinator
        .bind_resources(
            lifetime,
            Arc::new(ScriptResources {
                closes: AtomicUsize::new(0),
                report: released(),
                hold: None,
            }),
        )
        .unwrap_err();
    assert_eq!(refused.reason, BindResourcesRefusal::Released);
    child(&restored, &root, "replacement").await;
    assert_eq!(room.held.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn obsolete_reservation_release_fault_installs_validated_history_but_returns_uncertain() {
    for after_effect in [false, true] {
        let original = World::new(1);
        let root = original.root().await;
        let retained = child(&original, &root, "retained").await;
        let room = CountedRoom::new(1);
        let restored = restore_with(original.store.clone(), room.clone());
        restored.coordinator.resume().await.unwrap();
        assert_eq!(room.held.load(Ordering::SeqCst), 1);
        // Only the actual original owner has resources. Its sequential cleanup
        // publishes real physical release before the quiescent reload.
        original.close(&retained).await.unwrap();
        room.panic_release.store(true, Ordering::SeqCst);
        room.panic_after_release
            .store(after_effect, Ordering::SeqCst);
        assert_eq!(
            restored.coordinator.resume().await,
            Err(OwnershipFailure::Startup(PortFailure::Uncertain))
        );
        assert_eq!(
            restored.coordinator.lifetime_state(&retained),
            Some(LifetimeState::Closed)
        );
        assert_eq!(room.held.load(Ordering::SeqCst), usize::from(!after_effect));
        assert_eq!(room.releases.load(Ordering::SeqCst), 1);
        room.panic_release.store(false, Ordering::SeqCst);
        restored.coordinator.resume().await.unwrap();
        assert_eq!(
            room.releases.load(Ordering::SeqCst),
            1,
            "uncertain quota release must never be retried as an invented extra release"
        );
    }
}

#[tokio::test]
async fn restored_evidence_owner_uses_existing_slot_without_reopening_admission_or_capacity() {
    let original = World::new(1);
    let root = original.root().await;
    original
        .factory
        .reports
        .lock()
        .unwrap()
        .push_back(ResourceReport {
            physical: PhysicalFact::Released,
            evidence: EvidenceFact::Failed,
        });
    let retained = child(&original, &root, "released-with-evidence").await;
    assert!(original.close(&retained).await.is_err());
    let store = original.store.clone();
    drop(original);
    let room = CountedRoom::new(1);
    let restored = restore_with(store, room.clone());
    restored.coordinator.resume().await.unwrap();
    let owner = Arc::new(ScriptResources {
        closes: AtomicUsize::new(0),
        report: released(),
        hold: None,
    });
    assert!(restored
        .coordinator
        .bind_resources(retained.clone(), owner.clone())
        .is_err());
    assert!(restored.coordinator.participation(&retained).is_none());
    restored
        .coordinator
        .restore_closing_resources(retained.clone(), owner.clone())
        .unwrap();
    assert!(
        restored.coordinator.participation(&retained).is_none(),
        "evidence restoration gives no attachment authority"
    );
    assert!(
        restored
            .coordinator
            .restore_closing_resources(retained.clone(), owner.clone())
            .is_err(),
        "the exact registered owner is not replaced"
    );
    assert_eq!(room.calls.load(Ordering::SeqCst), 0);
    restored.close(&retained).await.unwrap();
    assert_eq!(owner.closes.load(Ordering::SeqCst), 1);
    assert_eq!(
        restored.coordinator.lifetime_state(&retained),
        Some(LifetimeState::Closed)
    );
    assert!(restored
        .coordinator
        .restore_closing_resources(retained, owner)
        .is_err());
    let refusal = restored
        .coordinator
        .restore_closing_resources(
            root.clone(),
            Arc::new(ScriptResources {
                closes: AtomicUsize::new(0),
                report: released(),
                hold: None,
            }),
        )
        .unwrap_err();
    assert_eq!(refusal.reason, BindResourcesRefusal::NotClosing);
    child(&restored, &root, "replacement").await;
    assert_eq!(room.held.load(Ordering::SeqCst), 1);
}
