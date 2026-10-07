//! Live pre-factory exclusion and durable absence through an eligible store rejection.
use super::*;
use nessa_sdk::application::agent_execution::subagents::BindResourcesRefusal;
use nessa_sdk::domain::agent_execution::subagents::{
    AbsenceProof, CloseEvidenceDetail, SettlementProof,
};
use std::future::Future;
use tokio::sync::oneshot;

struct HeldRevocationAudit {
    root: AgentLifetimeId,
    intent: Mutex<Option<OwnershipEvidence>>,
    entered: Notify,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}

#[async_trait]
impl OwnershipAudit for HeldRevocationAudit {
    async fn record(&self, record: &OwnershipEvidence) -> Result<(), PortFailure> {
        // Select only the first actual child's revocation intent. The original
        // Reserved audit is accepted, making this a public eligible child.
        let selected = record.parent_lifetime != self.root
            && record.close_operation.is_some()
            && record.close_detail.is_none()
            && record.before == OwnershipMeaning::Open
            && record.after == OwnershipMeaning::Closing
            && record.cause == Some(LifetimeCause::TerminalFailure);
        let release = if selected {
            self.release.lock().unwrap().take()
        } else {
            None
        };
        if let Some(release) = release {
            *self.intent.lock().unwrap() = Some(record.clone());
            self.entered.notify_one();
            release
                .await
                .expect("test releases only this fallback intent");
        }
        Ok(())
    }
}

struct TrackedOwner {
    closes: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
}

impl Drop for TrackedOwner {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl ChildResources for TrackedOwner {
    async fn close(&self, _: &LifetimeCause, _: &Initiator) -> ResourceReport {
        self.closes.fetch_add(1, Ordering::SeqCst);
        released()
    }
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .expect("live pre-factory exclusion journey must finish")
}

#[tokio::test]
async fn eligible_store_rejection_keeps_live_flight_until_durable_absence_excludes_binding() {
    let world = World::new(1);
    let root = bounded(world.root()).await;
    // Reuse the acknowledged root snapshot but replace only the audit seam.
    // The positive root control distinguishes absence exclusion from private
    // identity rejection; the child Reserved audit below returns real Ok.
    let (release, held) = oneshot::channel();
    let audit = Arc::new(HeldRevocationAudit {
        root: root.clone(),
        intent: Mutex::new(None),
        entered: Notify::new(),
        release: Mutex::new(Some(held)),
    });
    let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: world.store.clone(),
        audit: audit.clone(),
        factory: world.factory.clone(),
        room: Arc::new(LiveCapacity::new(1)),
    });
    bounded(coordinator.resume()).await.unwrap();
    assert_eq!(coordinator.lifetime_state(&root), Some(LifetimeState::Open));
    assert!(!coordinator.participation(&root).unwrap().is_sealed());
    let command = world.command(&root, "flight-absence", "task");
    world.store.fail_next_write(PortFailure::Rejected);
    let first = tokio::spawn({
        let coordinator = coordinator.clone();
        let command = command.clone();
        async move { coordinator.spawn(command).await }
    });
    bounded(audit.entered.notified()).await;
    let intent = audit.intent.lock().unwrap().clone().unwrap();
    let child = intent.parent_lifetime.clone();
    assert_ne!(child, root);
    assert_eq!(intent.initiator, Initiator::Runtime);
    assert!(intent.close_operation.is_some());
    assert_eq!(
        coordinator.lifetime_state(&child),
        Some(LifetimeState::Closing)
    );
    assert_eq!(coordinator.lifetime_state(&root), Some(LifetimeState::Open));
    let closes = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let owner: Arc<dyn ChildResources> = Arc::new(TrackedOwner {
        closes: closes.clone(),
        drops: drops.clone(),
    });
    let weak = Arc::downgrade(&owner);
    let refused = coordinator
        .bind_resources(child.clone(), owner.clone())
        .unwrap_err();
    assert_eq!(refused.reason, BindResourcesRefusal::FactoryInFlight);
    assert!(Arc::ptr_eq(&refused.resources, &owner));
    drop(refused);
    assert_eq!(closes.load(Ordering::SeqCst), 0);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert_eq!(world.factory.prepares(), 0);
    // This root-only durable copy contains no invented empty-map absence.
    assert!(bounded(world.store.read())
        .await
        .unwrap()
        .settlements
        .is_empty());
    release.send(()).unwrap();
    let failure = bounded(first).await.unwrap();
    assert_eq!(failure, Err(OwnershipFailure::Store(PortFailure::Rejected)));
    assert!(coordinator.participation(&child).is_none());
    let refused = coordinator
        .bind_resources(child.clone(), owner.clone())
        .unwrap_err();
    assert_eq!(refused.reason, BindResourcesRefusal::Released);
    assert!(Arc::ptr_eq(&refused.resources, &owner));
    drop(refused);
    let saved = bounded(world.store.read()).await.unwrap();
    let absence = saved
        .settlements
        .iter()
        .find(|row| row.target == child)
        .unwrap()
        .clone();
    let SettlementProof::Absence(proof) = &absence.proof else {
        panic!("THIS admitted flight must save actual pre-factory absence");
    };
    assert_eq!(
        proof.proof(),
        &AbsenceProof::AdmissionFailedBeforeFactory(command.request_id.clone())
    );
    assert_eq!(proof.record().close_operation, intent.close_operation);
    assert_eq!(proof.record().cause, intent.cause);
    assert_eq!(proof.record().initiator, intent.initiator);
    assert_eq!(
        proof.record().close_detail,
        Some(CloseEvidenceDetail::Absence(proof.proof().clone()))
    );
    assert_eq!(
        saved
            .spawns
            .iter()
            .find(|row| row.child_lifetime == child)
            .unwrap()
            .binding
            .request_id,
        command.request_id
    );
    assert_eq!(OwnershipGraph::restore(saved.clone()).refusal(), None);
    assert_eq!(bounded(coordinator.spawn(command)).await, failure);
    assert_eq!(world.factory.prepares(), 0);
    assert_eq!(world.factory.submits(), 0);
    drop(coordinator);

    let restored_factory = ScriptFactory::new();
    let restored = OwnershipCoordinator::new(OwnershipDependencies {
        store: world.store.clone(),
        audit: ScriptAudit::new(),
        factory: restored_factory.clone(),
        room: Arc::new(LiveCapacity::new(1)),
    });
    bounded(restored.resume()).await.unwrap();
    // Check synchronously before a background reconciliation can move Closing
    // to Closed: these refusals come from retained proof, not Closed state.
    assert_eq!(
        restored.lifetime_state(&child),
        Some(LifetimeState::Closing)
    );
    assert!(restored.participation(&child).is_none());
    let refused = restored
        .bind_resources(child.clone(), owner.clone())
        .unwrap_err();
    assert_eq!(refused.reason, BindResourcesRefusal::Released);
    assert!(Arc::ptr_eq(&refused.resources, &owner));
    drop(refused);
    assert_eq!(restored.lifetime_state(&root), Some(LifetimeState::Open));
    assert!(!restored.participation(&root).unwrap().is_sealed());
    let retained = bounded(world.store.read()).await.unwrap();
    assert_eq!(
        retained
            .settlements
            .iter()
            .find(|row| row.target == child)
            .unwrap(),
        &absence
    );
    assert_eq!(restored_factory.prepares(), 0);
    assert_eq!(restored_factory.submits(), 0);
    assert_eq!(closes.load(Ordering::SeqCst), 0);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(owner);
    assert!(
        weak.upgrade().is_none(),
        "all rejected transfers returned the caller's sole owner"
    );
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
