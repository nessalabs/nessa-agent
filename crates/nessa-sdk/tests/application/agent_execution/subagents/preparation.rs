//! Empty child preparation uses ordinary ownership, without an invented task receipt.
use super::*;
use nessa_sdk::infrastructure::session_storage::SqliteOwnershipStore;

#[tokio::test]
async fn preparation_attaches_the_named_child_without_submitting_a_turn() {
    let world = World::new(4);
    let root = world.root().await;
    let command = world.command(&root, "prepare", "configuration fingerprint");
    let session = SessionId::new("empty-child").unwrap();
    let ready = world
        .coordinator
        .prepare_child(command.clone(), session.clone())
        .await
        .unwrap();
    assert_eq!(ready.session, session);
    assert_eq!(ready.progress, SpawnProgress::Ready);
    assert!(world
        .audit
        .records
        .lock()
        .unwrap()
        .iter()
        .any(|record| record.after == OwnershipMeaning::Ready
            && record.child_lifetime.as_ref() == Some(&ready.child)));
    assert_eq!(world.factory.submits.load(Ordering::SeqCst), 0);
    let retry = world
        .coordinator
        .prepare_child(command.clone(), session.clone())
        .await
        .unwrap();
    assert_eq!(retry, ready);
    assert_eq!(world.factory.prepares.load(Ordering::SeqCst), 1);
    assert!(matches!(
        world.coordinator.spawn(command.clone()).await,
        Err(OwnershipFailure::Domain(OwnershipError::RequestConflict))
    ));
    assert!(matches!(
        world
            .coordinator
            .prepare_child(command, SessionId::new("other").unwrap())
            .await,
        Err(OwnershipFailure::Domain(OwnershipError::RequestConflict))
    ));
    let restored = resumed(world.store.clone());
    restored.coordinator.resume().await.unwrap();
    assert_eq!(
        restored.coordinator.active_lifetime_for_session(&session),
        Some(ready.child.clone())
    );
    assert!(!restored
        .coordinator
        .participation(&ready.child)
        .unwrap()
        .is_sealed());
    assert_eq!(restored.factory.prepares.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn preparation_remains_owned_during_parent_close() {
    let world = World::new(4);
    let root = world.root().await;
    world.bind_root(&root);
    let command = world.command(&root, "prepare-close", "configuration");
    let ready = world
        .coordinator
        .prepare_child(command, SessionId::new("prepared").unwrap())
        .await
        .unwrap();
    world.close(&root).await.unwrap();
    assert_eq!(
        world.coordinator.lifetime_state(&ready.child),
        Some(LifetimeState::Closed)
    );
    assert_eq!(world.factory.submits.load(Ordering::SeqCst), 0);
    assert_eq!(
        world.factory.children.lock().unwrap()[0]
            .closes
            .load(Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn preparation_survives_its_callers_disconnect_without_dispatching() {
    let world = World::new(4);
    let root = world.root().await;
    let release = Arc::new(Notify::new());
    *world.factory.release.lock().unwrap() = Some(release.clone());
    let command = world.command(&root, "prepare-disconnect", "configuration");
    let session = SessionId::new("disconnect-child").unwrap();
    let coordinator = world.coordinator.clone();
    let submitted = command.clone();
    let target = session.clone();
    let caller = tokio::spawn(async move { coordinator.prepare_child(submitted, target).await });
    world.factory.entered.notified().await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    release.notify_one();
    let ready = world
        .coordinator
        .prepare_child(command, session)
        .await
        .unwrap();
    assert_eq!(ready.progress, SpawnProgress::Ready);
    assert_eq!(world.factory.prepares(), 1);
    assert_eq!(world.factory.submits(), 0);
}

#[tokio::test]
async fn preparation_refuses_reusing_an_active_session_before_the_factory() {
    let world = World::new(4);
    let root = world.root().await;
    let first = world.command(&root, "first-preparation", "configuration");
    let session = SessionId::new("same-session").unwrap();
    world
        .coordinator
        .prepare_child(first, session.clone())
        .await
        .unwrap();
    let second = world.command(&root, "second-preparation", "configuration");
    assert!(matches!(
        world.coordinator.prepare_child(second, session).await,
        Err(OwnershipFailure::Domain(OwnershipError::LifetimeStillOpen))
    ));
    assert_eq!(world.factory.prepares(), 1);
    assert_eq!(world.factory.submits(), 0);
}

#[tokio::test]
async fn preparation_readiness_round_trips_through_the_real_sqlite_owner() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("ownership");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("ownership.sqlite3");
    let store = Arc::new(SqliteOwnershipStore::open(&path).unwrap());
    let world = World::new(4);
    let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: store.clone(),
        audit: world.audit.clone(),
        factory: world.factory.clone(),
        room: Arc::new(LiveCapacity::new(4)),
    });
    let root = coordinator
        .open_root(
            SessionId::new("sqlite-parent").unwrap(),
            Initiator::Host(actor("root")),
        )
        .await
        .unwrap();
    let session = SessionId::new("sqlite-prepared-child").unwrap();
    let command = world.command(&root, "sqlite-preparation", "configuration");
    let ready = coordinator
        .prepare_child(command, session.clone())
        .await
        .unwrap();
    drop(coordinator);
    drop(store);
    let reopened = OwnershipCoordinator::new(OwnershipDependencies {
        store: Arc::new(SqliteOwnershipStore::open(&path).unwrap()),
        audit: world.audit.clone(),
        factory: world.factory.clone(),
        room: Arc::new(LiveCapacity::new(4)),
    });
    reopened.resume().await.unwrap();
    assert_eq!(
        reopened.active_lifetime_for_session(&session),
        Some(ready.child.clone())
    );
    assert!(!reopened.participation(&ready.child).unwrap().is_sealed());
    assert_eq!(world.factory.prepares(), 1);
    assert_eq!(world.factory.submits(), 0);
}

#[tokio::test]
async fn restored_preparation_cannot_alias_its_parent_or_a_different_child_session() {
    for alias_parent in [false, true] {
        let world = World::new(4);
        let root = world.root().await;
        let command = world.command(&root, "correlation", "configuration");
        let ready = world
            .coordinator
            .prepare_child(command, SessionId::new("correlated-child").unwrap())
            .await
            .unwrap();
        let mut saved = world.store.read().await.unwrap();
        if alias_parent {
            let parent_session = saved
                .lifetimes
                .iter()
                .find(|r| r.lifetime_id == root)
                .unwrap()
                .session_id
                .clone();
            saved
                .lifetimes
                .iter_mut()
                .find(|r| r.lifetime_id == ready.child)
                .unwrap()
                .session_id = parent_session.clone();
            saved.spawns[0].child_session = parent_session;
        } else {
            saved.spawns[0].child_session = SessionId::new("unrelated-child").unwrap();
        }
        world.store.write(&saved).await.unwrap();
        let restored = resumed(world.store.clone());
        restored.coordinator.resume().await.unwrap();
        assert!(restored
            .coordinator
            .participation(&ready.child)
            .unwrap()
            .is_sealed());
        assert_eq!(restored.factory.prepares(), 0);
        assert_eq!(restored.factory.submits(), 0);
    }
}

#[tokio::test]
async fn preparation_and_spawn_have_disjoint_bindings_for_crafted_utf8_tasks() {
    for prepare_first in [false, true] {
        let world = World::new(4);
        let root = world.root().await;
        let session = SessionId::new("x").unwrap();
        let description = "configuration";
        let mut canonical = b"nessa.child.preparation\0".to_vec();
        canonical.extend_from_slice(&1_u64.to_be_bytes());
        canonical.extend_from_slice(session.as_str().as_bytes());
        canonical.extend_from_slice(description.as_bytes());
        let forged_task = String::from_utf8(canonical).unwrap();
        let prepare = world.command(&root, "cross-operation", description);
        let spawn = world.command(&root, "cross-operation", &forged_task);
        let result = if prepare_first {
            world
                .coordinator
                .prepare_child(prepare, session)
                .await
                .unwrap();
            world.coordinator.spawn(spawn).await
        } else {
            world.coordinator.spawn(spawn).await.unwrap();
            world.coordinator.prepare_child(prepare, session).await
        };
        assert!(matches!(
            result,
            Err(OwnershipFailure::Domain(OwnershipError::RequestConflict))
        ));
        assert_eq!(world.factory.prepares(), 1);
        assert_eq!(world.factory.submits(), usize::from(!prepare_first));
    }
}

struct ReadyAudit {
    base: Arc<ScriptAudit>,
    failure: Option<PortFailure>,
    entered: Notify,
    release: Option<Arc<Notify>>,
}
#[async_trait]
impl OwnershipAudit for ReadyAudit {
    async fn record(&self, evidence: &OwnershipEvidence) -> Result<(), PortFailure> {
        if evidence.after == OwnershipMeaning::Ready {
            self.entered.notify_one();
            if let Some(release) = &self.release {
                release.notified().await;
            }
            if let Some(failure) = self.failure {
                return Err(failure);
            }
        }
        self.base.record(evidence).await
    }
}
struct ReadyStore {
    base: Arc<MemoryOwnershipStore>,
    failure: Mutex<Option<PortFailure>>,
}
#[async_trait]
impl OwnershipStore for ReadyStore {
    async fn write(&self, snapshot: &OwnershipSnapshot) -> Result<(), PortFailure> {
        if snapshot
            .spawns
            .iter()
            .any(|row| row.progress == SpawnProgress::Ready)
        {
            if let Some(failure) = self.failure.lock().unwrap().take() {
                return Err(failure);
            }
        }
        self.base.write(snapshot).await
    }
    async fn read(&self) -> Result<OwnershipSnapshot, PortFailure> {
        self.base.read().await
    }
}

#[tokio::test]
async fn failed_ready_audit_or_store_keeps_cleanup_and_never_submits() {
    for audit_fails in [false, true] {
        for failure in [PortFailure::Rejected, PortFailure::Uncertain] {
            let mut world = World::new(4);
            world.coordinator = OwnershipCoordinator::new(OwnershipDependencies {
                store: Arc::new(ReadyStore {
                    base: world.store.clone(),
                    failure: Mutex::new((!audit_fails).then_some(failure)),
                }),
                audit: Arc::new(ReadyAudit {
                    base: world.audit.clone(),
                    failure: audit_fails.then_some(failure),
                    entered: Notify::new(),
                    release: None,
                }),
                factory: world.factory.clone(),
                room: Arc::new(LiveCapacity::new(4)),
            });
            let root = world.root().await;
            let command = world.command(&root, "ready-failure", "configuration");
            let session = SessionId::new("unconfirmed-preparation").unwrap();
            let failed = world
                .coordinator
                .prepare_child(command.clone(), session.clone())
                .await;
            if audit_fails {
                assert_eq!(failed, Err(OwnershipFailure::Audit(failure)));
            } else {
                assert_eq!(failed, Err(OwnershipFailure::Store(failure)));
            }
            let child = world.factory.last_child.lock().unwrap().clone().unwrap();
            assert_ne!(
                world.coordinator.lifetime_state(&child),
                Some(LifetimeState::Open)
            );
            assert!(world
                .factory
                .last_gate
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .is_sealed());
            assert!(world
                .coordinator
                .prepare_child(command, session)
                .await
                .is_err());
            assert_eq!(world.factory.prepares(), 1);
            assert_eq!(world.factory.submits(), 0);
            world.close(&child).await.unwrap();
            assert_eq!(
                world.factory.children.lock().unwrap()[0]
                    .closes
                    .load(Ordering::SeqCst),
                1
            );
        }
    }
}

#[tokio::test]
async fn parent_close_during_ready_publication_keeps_the_child_nonrunnable() {
    let mut world = World::new(4);
    let release = Arc::new(Notify::new());
    let audit = Arc::new(ReadyAudit {
        base: world.audit.clone(),
        failure: None,
        entered: Notify::new(),
        release: Some(release.clone()),
    });
    world.coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: world.store.clone(),
        audit: audit.clone(),
        factory: world.factory.clone(),
        room: Arc::new(LiveCapacity::new(4)),
    });
    let root = world.root().await;
    world.bind_root(&root);
    let command = world.command(&root, "close-at-ready", "configuration");
    let coordinator = world.coordinator.clone();
    let preparing = tokio::spawn(async move {
        coordinator
            .prepare_child(command, SessionId::new("close-at-ready-child").unwrap())
            .await
    });
    audit.entered.notified().await;
    let coordinator = world.coordinator.clone();
    let closing_root = root.clone();
    let closing = tokio::spawn(async move {
        coordinator
            .end_lifetime(CloseCommand {
                lifetime: closing_root,
                cause: LifetimeCause::HostClose,
                initiator: Initiator::Host(actor("close")),
                external_attachment: false,
                timeout: None,
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while world.coordinator.lifetime_state(&root) == Some(LifetimeState::Open) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    release.notify_one();
    let result = preparing.await.unwrap();
    if let Ok(receipt) = result {
        assert!(matches!(
            receipt.progress,
            SpawnProgress::Draining { .. } | SpawnProgress::Ended { .. }
        ));
    }
    closing.await.unwrap().unwrap();
    let child = world.factory.last_child.lock().unwrap().clone().unwrap();
    assert_eq!(
        world.coordinator.lifetime_state(&child),
        Some(LifetimeState::Closed)
    );
    assert!(world
        .factory
        .last_gate
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .is_sealed());
    assert_eq!(world.factory.submits(), 0);
}
