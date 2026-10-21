//! A granted remote lease survives failed preparation until its real End ACK.
use super::*;
use crate::conversation::infrastructure::ssh_environment::preparation_environment;
use nessa_sdk::application::agent_execution::sessions::{
    CommittedSession, SessionChange, SessionLoad, SessionSaveGeneration, SessionSaveReceipt,
    SessionSaveUnit, SessionStorage, SessionStorageLease, StorageFuture,
};
use nessa_sdk::domain::agent_execution::sessions::SessionId;

struct PreparationWriteFault {
    inner: Arc<RecordStorage>,
    target: SessionId,
    commit: bool,
    fired: Arc<AtomicBool>,
}
impl SessionStorage for PreparationWriteFault {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            let inner = self.inner.open(id.clone()).await?;
            if id != self.target {
                return Ok(inner);
            }
            Ok(Box::new(PreparationWriteLease {
                inner,
                commit: self.commit,
                fired: self.fired.clone(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
    fn read_committed(&self, id: SessionId) -> StorageFuture<'_, Option<CommittedSession>> {
        self.inner.read_committed(id)
    }
}
struct PreparationWriteLease {
    inner: Box<dyn SessionStorageLease>,
    commit: bool,
    fired: Arc<AtomicBool>,
}
impl SessionStorageLease for PreparationWriteLease {
    fn load(&self) -> StorageFuture<'_, SessionLoad> {
        self.inner.load()
    }
    fn save_changes(
        &self,
        binding: SessionSaveGeneration,
        units: Vec<SessionSaveUnit>,
    ) -> StorageFuture<'_, SessionSaveReceipt> {
        Box::pin(async move {
            let opening = units
                .iter()
                .flat_map(SessionSaveUnit::changes)
                .any(|change| matches!(change, SessionChange::Opened { .. }));
            if opening && !self.fired.swap(true, Ordering::SeqCst) {
                if self.commit {
                    self.inner.save_changes(binding, units).await?;
                }
                return Err(StorageError::Io(
                    "preparation write acknowledgement unavailable".into(),
                ));
            }
            self.inner.save_changes(binding, units).await
        })
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.inner.erase()
    }
}
async fn granted_preparation_failure(commit: bool, earlier: bool, public_stop: bool) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (environment, host) = preparation_environment();
    let (mut service, storage, metadata) = fixture_with_sessions(
        &root,
        provider.clone(),
        environment.clone().into(),
        crate::conversation_test_support::access_policy(),
        crate::conversation_test_support::claude_erasers(),
    );
    let target = id();
    let fired = Arc::new(AtomicBool::new(false));
    Arc::get_mut(&mut service.inner).unwrap().storage = Arc::new(PreparationWriteFault {
        inner: storage.clone(),
        target: conversation_session(&target),
        commit,
        fired: fired.clone(),
    });
    let coordinator = bind_with_capacity(&service, &storage, &root, true, 1).await;
    if earlier {
        let old = Arc::new(crate::conversation::application::service::OpeningResources::default());
        old.complete(false, DELETION_BUDGETS.stop).await;
        service
            .ownership_gate(
                &target,
                &caller("earlier-empty-opening").actor().unwrap(),
                Some(old),
            )
            .await
            .unwrap();
    }
    let creating = tokio::spawn({
        let service = service.clone();
        let target = target.clone();
        async move {
            service
                .create(
                    target,
                    caller("preparation-failure"),
                    RequestedConversation::default(),
                )
                .await
        }
    });
    host.wait_end().await;
    let opening = Arc::downgrade(
        service
            .inner
            .conversations
            .lock()
            .await
            .get(&target)
            .unwrap(),
    );
    assert!(fired.load(Ordering::SeqCst));
    assert_eq!(
        host.held(),
        1,
        "the actual remote grant still owns resources"
    );
    let created = tokio::time::timeout(TURN_HOLD_HANG, creating)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            created,
            Err(ConversationError::Agent(
                nessa_sdk::application::agent_execution::agents::AgentError::Storage(
                    StorageError::Io(_)
                )
            ))
        ),
        "{created:?}"
    );
    assert!(
        opening.upgrade().is_some(),
        "the actual unacknowledged grant keeps its Slot"
    );
    let committed = storage
        .read_committed(conversation_session(&target))
        .await
        .unwrap();
    assert_eq!(
        committed
            .as_ref()
            .and_then(CommittedSession::snapshot)
            .is_some(),
        commit
    );
    if let Some(snapshot) = committed.as_ref().and_then(CommittedSession::snapshot) {
        assert!(snapshot.lease.is_none());
    }
    if public_stop {
        let Err(ConversationError::Retirement(failures)) = service.stop_active_agents().await
        else {
            panic!("public stop must retain unanswered preparation cleanup");
        };
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, target.to_string());
        assert!(matches!(failures[0].1, AgentError::CleanupUncertain));
        assert!(service.owns_unreleased_resources().await);
        assert!(opening.upgrade().is_some());
    }
    let lifetime = coordinator
        .active_lifetime_for_session(&conversation_session(&target))
        .unwrap();
    let first = Initiator::Host(HostActor::new("person", "panel", "preparation-close").unwrap());
    let command = |cause, initiator| CloseCommand {
        lifetime: lifetime.clone(),
        cause,
        initiator,
        external_attachment: false,
        timeout: None,
    };
    let closed = coordinator
        .end_lifetime(command(LifetimeCause::HostClose, first.clone()))
        .await;
    assert!(
        matches!(
            closed,
            Err(nessa_sdk::application::agent_execution::subagents::OwnershipFailure::Incomplete)
        ),
        "remote End is still paused: {closed:?}"
    );
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closing)
    );
    assert_eq!(host.held(), 1);
    // Reconstruct the actual durable graph while the physical owner remains
    // live. This is graph restoration, not a second concurrent storage runtime.
    let persisted = SqliteOwnershipStore::open(&root.join("ownership.sqlite3"))
        .unwrap()
        .read()
        .await
        .unwrap();
    let restored =
        nessa_sdk::domain::agent_execution::subagents::OwnershipGraph::restore(persisted);
    assert_eq!(
        restored.lifetime_state(&lifetime),
        Some(LifetimeState::Closing)
    );
    assert_eq!(
        restored.close_cause(&lifetime).cloned(),
        Some(LifetimeCause::HostClose)
    );
    assert_eq!(
        restored.close_initiator(&lifetime).cloned(),
        Some(first.clone())
    );
    host.release();
    tokio::time::timeout(TURN_HOLD_HANG, async {
        while host.held() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    if public_stop {
        service.stop_active_agents().await.unwrap();
        assert!(opening.upgrade().is_none());
        assert!(!service.owns_unreleased_resources().await);
    }
    coordinator
        .end_lifetime(command(LifetimeCause::Deletion, Initiator::Runtime))
        .await
        .unwrap();
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        coordinator.close_cause(&lifetime),
        Some(LifetimeCause::HostClose)
    );
    assert_eq!(coordinator.close_initiator(&lifetime), Some(first.clone()));
    tokio::time::timeout(TURN_HOLD_HANG, async {
        while opening.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
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
    assert_eq!(row.initiator, Some(first));
    assert_eq!(
        host.end_requests(),
        1,
        "retry joins the original remote End"
    );
    host.close();
    retire(service, storage, metadata).await;
    drop(coordinator);
    let (service, storage, metadata) = fixture(&root, provider);
    let restored = bind_with_capacity(&service, &storage, &root, true, 1).await;
    assert_eq!(
        restored.lifetime_state(&lifetime),
        Some(LifetimeState::Closed)
    );
    retire(service, storage, metadata).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_granted_root_rejected_preparation_write_waits_for_real_release() {
    granted_preparation_failure(false, false, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_granted_root_lost_preparation_ack_waits_for_real_release() {
    granted_preparation_failure(true, false, false).await;
}

async fn granted_child_preparation_failure(commit: bool) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (environment, host) = preparation_environment();
    let (mut service, storage, metadata) = fixture_with_sessions(
        &root,
        provider.clone(),
        environment.into(),
        crate::conversation_test_support::access_policy(),
        crate::conversation_test_support::claude_erasers(),
    );
    let target = id();
    let fired = Arc::new(AtomicBool::new(false));
    Arc::get_mut(&mut service.inner).unwrap().storage = Arc::new(PreparationWriteFault {
        inner: storage.clone(),
        target: conversation_session(&target),
        commit,
        fired: fired.clone(),
    });
    let coordinator = bind_with_capacity(&service, &storage, &root, true, 1).await;
    let parent = id();
    opened(&service, &provider, &parent).await;
    let work = hold_turn(&service, &provider, &parent, "parent").await;
    let creating = tokio::spawn({
        let service = service.clone();
        let parent = parent.clone();
        let target = target.clone();
        async move {
            service
                .create_owned_thread(
                    source(&parent, "parent", "failed-remote-child"),
                    target,
                    RequestedConversation::default(),
                )
                .await
        }
    });
    host.wait_end().await;
    let opening = Arc::downgrade(
        service
            .inner
            .conversations
            .lock()
            .await
            .get(&target)
            .unwrap(),
    );
    assert!(fired.load(Ordering::SeqCst));
    assert_eq!(host.held(), 2);
    assert!(tokio::time::timeout(TURN_HOLD_HANG, creating)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    let lifetime = coordinator
        .child_lifetime_for_session(&conversation_session(&target))
        .unwrap();
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closing)
    );
    let first_cause = coordinator.close_cause(&lifetime);
    let first_actor = coordinator.close_initiator(&lifetime);
    assert!(first_cause.is_some());
    assert!(first_actor.is_some());
    let over_capacity = service
        .create_owned_thread(
            source(&parent, "parent", "while-release-pending"),
            id(),
            RequestedConversation::default(),
        )
        .await;
    assert!(
        over_capacity.is_err(),
        "the single child reservation must remain held"
    );
    assert_eq!(host.held(), 2);
    assert!(
        opening.upgrade().is_some(),
        "the failed grant retains its actual owner"
    );
    host.release();
    tokio::time::timeout(TURN_HOLD_HANG, async {
        while host.held() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    coordinator
        .end_lifetime(CloseCommand {
            lifetime: lifetime.clone(),
            cause: LifetimeCause::HostClose,
            initiator: Initiator::Runtime,
            external_attachment: false,
            timeout: None,
        })
        .await
        .unwrap();
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closed)
    );
    assert_eq!(coordinator.close_cause(&lifetime), first_cause);
    assert_eq!(coordinator.close_initiator(&lifetime), first_actor);
    tokio::time::timeout(TURN_HOLD_HANG, async {
        while opening.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(host.end_requests(), 1);
    child(&service, &parent, "parent", "after-real-release").await;
    assert_eq!(
        host.held(),
        2,
        "the single child capacity can be reused after release"
    );
    drop(work);
    host.close();
    retire(service, storage, metadata).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_granted_child_rejected_preparation_write_keeps_capacity_until_release() {
    granted_child_preparation_failure(false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_granted_child_lost_preparation_ack_keeps_capacity_until_release() {
    granted_child_preparation_failure(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_root_waits_for_its_environment_release() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (environment, host) = preparation_environment();
    let (service, storage, metadata) = fixture_with_sessions(
        &root,
        provider.clone(),
        environment.into(),
        crate::conversation_test_support::access_policy(),
        crate::conversation_test_support::claude_erasers(),
    );
    let coordinator = bind_with_capacity(&service, &storage, &root, true, 1).await;
    let target = id();
    opened(&service, &provider, &target).await;
    let lifetime = coordinator
        .active_lifetime_for_session(&conversation_session(&target))
        .unwrap();
    let first = Initiator::Host(HostActor::new("person", "panel", "live-close").unwrap());
    let closing = tokio::spawn({
        let coordinator = coordinator.clone();
        let lifetime = lifetime.clone();
        let first = first.clone();
        async move {
            coordinator
                .end_lifetime(CloseCommand {
                    lifetime,
                    cause: LifetimeCause::HostClose,
                    initiator: first,
                    external_attachment: false,
                    timeout: None,
                })
                .await
        }
    });
    host.wait_end().await;
    let result = tokio::time::timeout(TURN_HOLD_HANG, closing)
        .await
        .unwrap()
        .unwrap();
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
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 1);
    assert_eq!(host.held(), 1);
    host.release();
    tokio::time::timeout(TURN_HOLD_HANG, async {
        while host.held() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    coordinator
        .end_lifetime(CloseCommand {
            lifetime: lifetime.clone(),
            cause: LifetimeCause::Deletion,
            initiator: Initiator::Runtime,
            external_attachment: false,
            timeout: None,
        })
        .await
        .unwrap();
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        coordinator.close_cause(&lifetime),
        Some(LifetimeCause::HostClose)
    );
    assert_eq!(coordinator.close_initiator(&lifetime), Some(first));
    assert_eq!(host.end_requests(), 1);
    host.close();
    retire(service, storage, metadata).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_earlier_root_absence_cannot_hide_a_later_rejected_preparation_grant() {
    granted_preparation_failure(false, true, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_earlier_root_absence_cannot_hide_a_later_lost_ack_grant() {
    granted_preparation_failure(true, true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn public_stop_retains_a_rejected_preparation_grant_until_real_ack() {
    granted_preparation_failure(false, false, true).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn public_stop_retains_a_lost_preparation_ack_grant_until_real_ack() {
    granted_preparation_failure(true, false, true).await;
}
