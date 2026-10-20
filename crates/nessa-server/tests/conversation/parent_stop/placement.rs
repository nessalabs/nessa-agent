//! A lifetime can seal while the current-main placement owner is awaited.
use super::*;
use crate::conversation::application::{ConversationPlacements, Environments, PlacementError};
use crate::conversation::infrastructure::{in_process_environment, FilePlacements};
use nessa_sdk::application::agent_execution::sessions::{
    CommittedCompleteness, CommittedFreshness, CommittedSession, CommittedStatus, SessionStorage,
    SessionStorageLease, StorageFuture,
};
use nessa_sdk::domain::agent_execution::sessions::SessionId;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_root_sealed_during_first_placement_closes_from_its_actual_opening() {
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
    let target = id();
    let before = environment.opened.load(Ordering::SeqCst);
    placements.pause.store(true, Ordering::SeqCst);
    let creating = tokio::spawn({
        let service = service.clone();
        let target = target.clone();
        async move {
            service
                .create(
                    target,
                    caller("create-during-placement"),
                    RequestedConversation::default(),
                )
                .await
        }
    });
    tokio::time::timeout(TURN_HOLD_HANG, placements.entered.notified())
        .await
        .unwrap();
    let opening = Arc::downgrade(
        service
            .inner
            .conversations
            .lock()
            .await
            .get(&target)
            .unwrap(),
    );
    let lifetime = coordinator
        .active_lifetime_for_session(&conversation_session(&target))
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
    let closed = tokio::time::timeout(TURN_HOLD_HANG, closing)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            closed,
            Err(nessa_sdk::application::agent_execution::subagents::OwnershipFailure::Incomplete)
        ),
        "pending root: {closed:?}"
    );
    placements.release.notify_one();
    let created = tokio::time::timeout(TURN_HOLD_HANG, creating)
        .await
        .unwrap()
        .unwrap();
    assert!(created.is_err());
    assert_eq!(
        environment.opened.load(Ordering::SeqCst),
        before,
        "a lifetime sealed during placement acquired another lease"
    );
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    tokio::time::timeout(TURN_HOLD_HANG, async {
        while opening.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // The root adapter retains a lightweight completion receipt, not the Slot.
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
    retire(service, storage, metadata).await;
    drop(coordinator);
    let (service, storage, metadata) = fixture_in(
        &root,
        provider.clone(),
        Environments::new(environment.clone(), BTreeMap::new(), placements.clone()),
    );
    let coordinator = bind_with_capacity(&service, &storage, &root, true, 1).await;
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closed)
    );
    opened(&service, &provider, &target).await;
    let replacement = coordinator
        .active_lifetime_for_session(&conversation_session(&target))
        .unwrap();
    assert_ne!(replacement, lifetime);
    assert_eq!(
        coordinator.lifetime_state(&replacement),
        Some(LifetimeState::Open)
    );
    assert_eq!(
        environment.opened.load(Ordering::SeqCst),
        before + 1,
        "the root can open normally after confirmed absence"
    );
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn a_root_without_completed_nonacquisition_cannot_claim_absence() {
    for fact in [None, Some(None), Some(Some(true))] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("data");
        let provider = Arc::new(ProviderFactory::default());
        let (service, storage, metadata) = fixture(&root, provider.clone());
        let coordinator = bind(&service, &storage, &root, true).await;
        let target = id();
        let acquisition = fact.map(|value| {
            let receipt = Arc::new(std::sync::OnceLock::new());
            if let Some(value) = value {
                receipt.set(value).unwrap();
            }
            receipt
        });
        service
            .ownership_gate(&target, &caller("bind").actor().unwrap(), acquisition)
            .await
            .unwrap();
        let lifetime = coordinator
            .active_lifetime_for_session(&conversation_session(&target))
            .unwrap();
        let result = coordinator
            .end_lifetime(CloseCommand {
                lifetime: lifetime.clone(),
                cause: LifetimeCause::HostClose,
                initiator: Initiator::Host(
                    HostActor::new("person", "panel", "unconfirmed").unwrap(),
                ),
                external_attachment: false,
                timeout: None,
            })
            .await;
        assert!(matches!(result, Err(nessa_sdk::application::agent_execution::subagents::OwnershipFailure::Incomplete)), "{fact:?}: {result:?}");
        assert_eq!(
            coordinator.lifetime_state(&lifetime),
            Some(LifetimeState::Closing)
        );
        assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
        retire(service, storage, metadata).await;
    }
}

#[tokio::test]
async fn an_earlier_root_absence_cannot_release_a_later_held_lease() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let coordinator = bind(&service, &storage, &root, true).await;
    let target = id();
    let acquisition = Arc::new(std::sync::OnceLock::new());
    acquisition.set(false).unwrap();
    // Model a completed initial no-hold opening, then a genuine later opening
    // on the same still-Open root. The original bound adapter keeps its receipt.
    service
        .ownership_gate(
            &target,
            &caller("first-absence").actor().unwrap(),
            Some(acquisition),
        )
        .await
        .unwrap();
    opened(&service, &provider, &target).await;
    let lifetime = coordinator
        .active_lifetime_for_session(&conversation_session(&target))
        .unwrap();
    let slot = service
        .inner
        .conversations
        .lock()
        .await
        .remove(&target)
        .unwrap();
    let first_actor =
        Initiator::Host(HostActor::new("person", "panel", "close-held-history").unwrap());
    let close = |cause, initiator| CloseCommand {
        lifetime: lifetime.clone(),
        cause,
        initiator,
        external_attachment: false,
        timeout: None,
    };
    let result = coordinator
        .end_lifetime(close(LifetimeCause::HostClose, first_actor.clone()))
        .await;
    assert!(
        matches!(
            result,
            Err(nessa_sdk::application::agent_execution::subagents::OwnershipFailure::Incomplete)
        ),
        "{result:?}"
    );
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closing)
    );
    assert_eq!(
        provider.close_calls.load(Ordering::SeqCst),
        0,
        "cold NotHeld is not physical cleanup"
    );
    // Restore the actual retained owner; only its real provider cleanup releases.
    service
        .inner
        .conversations
        .lock()
        .await
        .insert(target.clone(), slot);
    coordinator
        .end_lifetime(close(LifetimeCause::Deletion, Initiator::Runtime))
        .await
        .unwrap();
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        coordinator.close_cause(&lifetime),
        Some(LifetimeCause::HostClose)
    );
    assert_eq!(coordinator.close_initiator(&lifetime), Some(first_actor));
    retire(service, storage, metadata).await;
}

struct RootReadFault {
    inner: Arc<RecordStorage>,
    mode: AtomicUsize,
}
impl SessionStorage for RootReadFault {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        self.inner.open(id)
    }
    fn read_committed(&self, id: SessionId) -> StorageFuture<'_, Option<CommittedSession>> {
        match self.mode.load(Ordering::SeqCst) {
            1 => Box::pin(async { Err(StorageError::CommittedReadUnavailable) }),
            mode @ (2 | 3) => Box::pin(async move {
                let (downloaded, completeness, freshness) = if mode == 2 {
                    (
                        1,
                        CommittedCompleteness::Partial,
                        CommittedFreshness::Current,
                    )
                } else {
                    (
                        0,
                        CommittedCompleteness::CompleteEmpty,
                        CommittedFreshness::Stale,
                    )
                };
                CommittedSession::new(
                    id,
                    "root-read-control".into(),
                    0,
                    downloaded,
                    1,
                    None,
                    CommittedStatus::new(completeness, freshness),
                )
                .map(Some)
            }),
            _ => self.inner.read_committed(id),
        }
    }
}

#[tokio::test]
async fn a_root_nonacquisition_receipt_cannot_replace_a_failed_history_read() {
    for mode in [1, 2, 3] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("data");
        let provider = Arc::new(ProviderFactory::default());
        let (mut service, storage, metadata) = fixture(&root, provider.clone());
        let fault = Arc::new(RootReadFault {
            inner: storage.clone(),
            mode: AtomicUsize::new(mode),
        });
        Arc::get_mut(&mut service.inner).unwrap().storage = fault.clone();
        let coordinator = bind(&service, &storage, &root, true).await;
        let target = id();
        let acquisition = Arc::new(std::sync::OnceLock::new());
        acquisition.set(false).unwrap();
        service
            .ownership_gate(&target, &caller("bind").actor().unwrap(), Some(acquisition))
            .await
            .unwrap();
        let lifetime = coordinator
            .active_lifetime_for_session(&conversation_session(&target))
            .unwrap();
        let first_actor =
            Initiator::Host(HostActor::new("person", "panel", "read-failure").unwrap());
        let close = |initiator| CloseCommand {
            lifetime: lifetime.clone(),
            cause: LifetimeCause::HostClose,
            initiator,
            external_attachment: false,
            timeout: None,
        };
        let result = coordinator.end_lifetime(close(first_actor.clone())).await;
        assert!(
            matches!(
            result,
            Err(nessa_sdk::application::agent_execution::subagents::OwnershipFailure::Incomplete)
        ),
            "{result:?}"
        );
        assert_eq!(
            coordinator.lifetime_state(&lifetime),
            Some(LifetimeState::Closing)
        );
        fault.mode.store(0, Ordering::SeqCst);
        coordinator
            .end_lifetime(close(Initiator::Runtime))
            .await
            .unwrap();
        assert_eq!(
            coordinator.lifetime_state(&lifetime),
            Some(LifetimeState::Closed)
        );
        assert_eq!(coordinator.close_initiator(&lifetime), Some(first_actor));
        assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
        retire(service, storage, metadata).await;
    }
}
