//! Empty child preparation uses ordinary ownership, without an invented task receipt.
use super::*;

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
    use nessa_sdk::infrastructure::session_storage::SqliteOwnershipStore;
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
