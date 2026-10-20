//! A lifetime can seal while the current-main placement owner is awaited.
use super::*;
use crate::conversation::application::{ConversationPlacements, Environments, PlacementError};
use crate::conversation::infrastructure::{in_process_environment, FilePlacements};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicUsize;

struct PausedPlacement {
    inner: FilePlacements,
    pause: AtomicBool,
    entered: Notify,
    release: Notify,
}
impl ConversationPlacements for PausedPlacement {
    fn place<'a>(
        &'a self,
        conversation: &'a ConversationId,
        host: Option<&'a str>,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>> {
        self.inner.place(conversation, host)
    }
    fn placement<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<Option<String>, PlacementError>> {
        Box::pin(async move {
            if self.pause.swap(false, Ordering::SeqCst) {
                self.entered.notify_one();
                self.release.notified().await;
            }
            self.inner.placement(conversation).await
        })
    }
    fn erase<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>> {
        self.inner.erase(conversation)
    }
}

struct CountedLeases {
    inner: Arc<dyn Environment>,
    opened: AtomicUsize,
}
impl Environment for CountedLeases {
    fn declaration(&self) -> EnvironmentDeclaration {
        self.inner.declaration()
    }
    fn open<'a>(
        &'a self,
        lease: &'a LeaseId,
        grant: &'a LeaseTerms,
        provider: Arc<dyn AgentProvider>,
    ) -> EnvironmentFuture<'a, Result<EnvironmentLease, LeaseRefusal>> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        self.inner.open(lease, grant, provider)
    }
    fn account<'a>(&'a self, lease: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>> {
        self.inner.account(lease)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_owned_opening_sealed_during_placement_does_not_acquire_a_lease() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let placements = Arc::new(PausedPlacement {
        inner: FilePlacements::new(root.join("placements")).unwrap(),
        pause: AtomicBool::new(false),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let environment = Arc::new(CountedLeases {
        inner: in_process_environment(),
        opened: AtomicUsize::new(0),
    });
    let (service, storage, metadata) = fixture_in(
        &root,
        provider.clone(),
        Environments::new(environment.clone(), BTreeMap::new(), placements.clone()),
    );
    let coordinator = bind_with_capacity(&service, &storage, &root, true, 1).await;
    let parent = id();
    opened(&service, &provider, &parent).await;
    let work = hold_turn(&service, &provider, &parent, "parent").await;
    let target = id();
    let before = environment.opened.load(Ordering::SeqCst);
    placements.pause.store(true, Ordering::SeqCst);
    let creating = tokio::spawn({
        let service = service.clone();
        let parent = parent.clone();
        let target = target.clone();
        async move {
            service
                .create_owned_thread(
                    source(&parent, "parent", "create-during-placement"),
                    target,
                    RequestedConversation::default(),
                )
                .await
        }
    });
    tokio::time::timeout(TURN_HOLD_HANG, placements.entered.notified())
        .await
        .unwrap();
    let lifetime = coordinator
        .child_lifetime_for_session(&conversation_session(&target))
        .unwrap();
    let closing = tokio::spawn({
        let coordinator = coordinator.clone();
        let lifetime = lifetime.clone();
        async move {
            coordinator
                .end_lifetime(CloseCommand {
                    lifetime,
                    cause: LifetimeCause::HostClose,
                    initiator: Initiator::Host(
                        HostActor::new("person", "panel", "close-during-placement").unwrap(),
                    ),
                    external_attachment: false,
                    timeout: None,
                })
                .await
        }
    });
    tokio::time::timeout(TURN_HOLD_HANG, async {
        while coordinator.lifetime_state(&lifetime) == Some(LifetimeState::Open) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    placements.release.notify_one();
    let created = tokio::time::timeout(TURN_HOLD_HANG, creating)
        .await
        .unwrap()
        .unwrap();
    let closed = tokio::time::timeout(TURN_HOLD_HANG, closing)
        .await
        .unwrap()
        .unwrap();
    assert!(created.is_err());
    assert_eq!(
        environment.opened.load(Ordering::SeqCst),
        before,
        "a lifetime sealed during placement acquired another lease"
    );
    assert!(matches!(closed, Ok(()) | Err(nessa_sdk::application::agent_execution::subagents::OwnershipFailure::Incomplete)), "{closed:?}");
    // A close that observed an in-flight factory joins the existing cleanup
    // owner after its actual opening result has been transferred.
    coordinator
        .end_lifetime(CloseCommand {
            lifetime: lifetime.clone(),
            cause: LifetimeCause::Deletion,
            initiator: Initiator::Host(HostActor::new("other", "other", "later-close").unwrap()),
            external_attachment: false,
            timeout: None,
        })
        .await
        .unwrap();
    let original =
        Initiator::Host(HostActor::new("person", "panel", "close-during-placement").unwrap());
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        coordinator.close_cause(&lifetime),
        Some(LifetimeCause::HostClose)
    );
    assert_eq!(
        coordinator.close_initiator(&lifetime),
        Some(original.clone())
    );
    let snapshot = SqliteOwnershipStore::open(&root.join("ownership.sqlite3"))
        .unwrap()
        .read()
        .await
        .unwrap();
    let row = snapshot
        .lifetimes
        .iter()
        .find(|row| row.lifetime_id == lifetime)
        .unwrap();
    assert_eq!(row.state, LifetimeState::Closed);
    assert_eq!(row.cause, Some(LifetimeCause::HostClose));
    assert_eq!(row.initiator, Some(original));
    child(&service, &parent, "parent", "after-placement-failure").await;
    assert_eq!(
        environment.opened.load(Ordering::SeqCst),
        before + 1,
        "the single reserved child capacity was released"
    );
    drop(work);
    retire(service, storage, metadata).await;
}
