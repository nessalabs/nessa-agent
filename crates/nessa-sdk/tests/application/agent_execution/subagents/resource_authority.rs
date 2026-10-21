//! Attachment-phase callbacks defer whole-lifetime release to the bound owner.
use super::*;
use nessa_sdk::application::agent_execution::permissions::ActionContext;

struct HeldOwner {
    calls: AtomicUsize,
    entered: Notify,
    release: Notify,
}
#[async_trait]
impl ChildResources for HeldOwner {
    async fn close(&self, _: &LifetimeCause, _: &Initiator) -> ResourceReport {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
        released()
    }
}
fn held_owner() -> Arc<HeldOwner> {
    Arc::new(HeldOwner {
        calls: AtomicUsize::new(0),
        entered: Notify::new(),
        release: Notify::new(),
    })
}
async fn wait_closing(coordinator: &OwnershipCoordinator, root: &AgentLifetimeId) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while coordinator.lifetime_state(root) != Some(LifetimeState::Closing) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn registered_owner_is_authoritative_after_external_attachment_callback() {
    let world = World::new(1);
    let root = world.root().await;
    // Get the original bare gate before binding; binding must revoke that
    // gate's attachment-only authority under the same admission scope.
    let gate = world.coordinator.participation(&root).unwrap();
    let owner = held_owner();
    world
        .coordinator
        .bind_resources(root.clone(), owner.clone())
        .unwrap();
    let closing = tokio::spawn({
        let coordinator = world.coordinator.clone();
        let root = root.clone();
        async move {
            coordinator
                .end_lifetime(CloseCommand {
                    lifetime: root,
                    cause: LifetimeCause::HostClose,
                    initiator: Initiator::Host(actor("external-first")),
                    external_attachment: true,
                    timeout: None,
                })
                .await
        }
    });
    wait_closing(&world.coordinator, &root).await;
    gate.note_attachment(true, true).await;
    tokio::time::timeout(Duration::from_secs(3), owner.entered.notified())
        .await
        .expect("attachment callback must hand the external phase to the actual owner");
    assert_eq!(owner.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Closing)
    );
    assert!(!closing.is_finished());
    owner.release.notify_one();
    tokio::time::timeout(Duration::from_secs(3), closing)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        world.coordinator.close_initiator(&root),
        Some(Initiator::Host(actor("external-first")))
    );
}
#[tokio::test]
async fn bare_sdk_attachment_retains_its_actual_release_authority() {
    let world = World::new(1);
    let root = world.root().await;
    let gate = world.coordinator.participation(&root).unwrap();
    gate.seal_for_host(&ActionContext::new("person", "desktop", "bare-close").unwrap())
        .await
        .unwrap();
    gate.note_attachment(true, true).await;
    tokio::time::timeout(Duration::from_secs(3), gate.join_descendants())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Closed)
    );
}
#[tokio::test]
async fn restored_missing_owner_cannot_gain_attachment_only_authority() {
    let world = World::new(1);
    let root = world.root().await;
    let old = world.coordinator.participation(&root).unwrap();
    world.coordinator.resume().await.unwrap();
    let current = world.coordinator.participation(&root).unwrap();
    current
        .seal_for_host(&ActionContext::new("person", "desktop", "restored-close").unwrap())
        .await
        .unwrap();
    // A delayed callback from a replaced capability has no authority in the
    // restored graph. The current callback also cannot invent missing resources.
    old.note_attachment(true, true).await;
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Closing)
    );
    current.note_attachment(true, true).await;
    assert!(
        tokio::time::timeout(Duration::from_secs(3), current.join_descendants())
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Closing)
    );
}
#[tokio::test]
async fn factory_callback_waits_for_actual_resource_transfer_before_capacity_release() {
    let world = World::new(1);
    let root = world.root().await;
    let release = Arc::new(Notify::new());
    *world.factory.release.lock().unwrap() = Some(release.clone());
    let command = world.command(&root, "preparing", "first");
    let preparing = tokio::spawn({
        let coordinator = world.coordinator.clone();
        async move { coordinator.spawn(command).await }
    });
    world.factory.entered.notified().await;
    let child = world.factory.last_child.lock().unwrap().clone().unwrap();
    let gate = world.factory.last_gate.lock().unwrap().clone().unwrap();
    let closing = tokio::spawn({
        let coordinator = world.coordinator.clone();
        let child = child.clone();
        async move {
            coordinator
                .end_lifetime(CloseCommand {
                    lifetime: child,
                    cause: LifetimeCause::HostClose,
                    initiator: Initiator::Runtime,
                    external_attachment: true,
                    timeout: None,
                })
                .await
        }
    });
    wait_closing(&world.coordinator, &child).await;
    gate.note_attachment(true, true).await;
    assert_eq!(
        world.coordinator.lifetime_state(&child),
        Some(LifetimeState::Closing)
    );
    assert!(!closing.is_finished());
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(3),
            world
                .coordinator
                .spawn(world.command(&root, "still-full", "second")),
        )
        .await
        .expect("capacity refusal must not enter the held factory"),
        Err(OwnershipFailure::Domain(OwnershipError::NoRoom))
    );
    assert_eq!(world.factory.prepares(), 1);
    release.notify_one();
    assert!(tokio::time::timeout(Duration::from_secs(3), preparing)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    tokio::time::timeout(Duration::from_secs(3), closing)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        world.factory.children.lock().unwrap()[0]
            .closes
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        world.coordinator.lifetime_state(&child),
        Some(LifetimeState::Closed)
    );
}

struct EvidenceOwner {
    acknowledged: std::sync::atomic::AtomicBool,
    physically_released: std::sync::atomic::AtomicBool,
    physical_releases: AtomicUsize,
    calls: AtomicUsize,
}
#[async_trait]
impl ChildResources for EvidenceOwner {
    async fn close(&self, _: &LifetimeCause, _: &Initiator) -> ResourceReport {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if !self.physically_released.swap(true, Ordering::SeqCst) {
            self.physical_releases.fetch_add(1, Ordering::SeqCst);
        }
        ResourceReport {
            physical: PhysicalFact::Released,
            evidence: if self.acknowledged.load(Ordering::SeqCst) {
                EvidenceFact::Acknowledged
            } else {
                EvidenceFact::Failed
            },
        }
    }
}
#[tokio::test]
async fn attachment_ack_cannot_mask_resource_evidence_failure_or_prevent_its_retry() {
    let world = World::new(1);
    let root = world.root().await;
    let owner = Arc::new(EvidenceOwner {
        acknowledged: std::sync::atomic::AtomicBool::new(false),
        physically_released: std::sync::atomic::AtomicBool::new(false),
        physical_releases: AtomicUsize::new(0),
        calls: AtomicUsize::new(0),
    });
    world
        .coordinator
        .bind_resources(root.clone(), owner.clone())
        .unwrap();
    let gate = world.coordinator.participation(&root).unwrap();
    gate.seal_for_host(&ActionContext::new("person", "desktop", "first-evidence-close").unwrap())
        .await
        .unwrap();
    gate.note_attachment(true, true).await;
    assert!(
        tokio::time::timeout(Duration::from_secs(3), gate.join_descendants())
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Closing)
    );
    assert_eq!(owner.calls.load(Ordering::SeqCst), 1);
    // A later callback from the already closed attachment cannot acknowledge
    // the resource owner's independently failed evidence.
    gate.note_attachment(true, true).await;
    assert!(
        tokio::time::timeout(Duration::from_secs(3), gate.join_descendants())
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Closing)
    );
    owner.acknowledged.store(true, Ordering::SeqCst);
    world.close(&root).await.unwrap();
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Closed)
    );
    assert_eq!(owner.physical_releases.load(Ordering::SeqCst), 1);
    assert!(owner.calls.load(Ordering::SeqCst) >= 2);
    assert_eq!(
        world.coordinator.close_initiator(&root),
        Some(Initiator::Host(actor("first-evidence-close")))
    );
}
