//! A saved session's restoration identity moved by one durable fact, folded,
//! persisted, replayed and restored.
use super::support::*;
use nessa_sdk::infrastructure::session_storage::{RecordStorage, RuntimeMessageCommitClock};

/// A provider that is only ever prepared: nothing here opens one.
struct Identified(ProviderIdentity);
impl AgentProvider for Identified {
    fn identity(&self) -> ProviderIdentity {
        self.0.clone()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async { Err(ProviderOpenError::no_resources(AgentError::Closed)) })
    }
}

fn identity(context: &str) -> ProviderIdentity {
    ProviderIdentity::new("fixture", "model", context).unwrap()
}

fn saved(provider: ProviderIdentity) -> SessionSnapshot {
    SessionSnapshot {
        id: SessionId::new("conversation").unwrap(),
        provider,
        provider_context: ProviderContext::Absent,
        invocations: Vec::new(),
        queue_history: Vec::new(),
    }
}

fn unit(before: ProviderIdentity, after: ProviderIdentity) -> Vec<SessionSaveUnit> {
    vec![SessionSaveUnit::new(vec![SessionChange::ProviderIdentity { before, after }]).unwrap()]
}

async fn prepared(
    storage: Arc<dyn SessionStorage>,
    id: &SessionId,
    provider: ProviderIdentity,
) -> Result<Agent, AgentError> {
    let manager = SessionManager::open(
        Some(id.clone()),
        storage,
        Arc::new(RuntimeMessageCommitClock::new()),
    )
    .await
    .map_err(AgentError::Storage)?;
    Agent::prepare(
        Arc::new(Identified(provider)),
        manager,
        Arc::new(AcceptingAudit),
    )
    .await
    .map_err(|error| error.cause().clone())
}

#[test]
fn a_provider_identity_change_that_does_not_continue_the_published_one_is_refused() {
    let published = saved(identity("before"));
    // `before` is not what is published.
    assert!(matches!(
        SessionSaveUnit::validate_plan(
            Some(&published),
            &unit(identity("other"), identity("after")),
            &saved(identity("after")),
        ),
        Err(StorageError::Corrupt(_))
    ));
    // It changes nothing.
    assert!(matches!(
        SessionSaveUnit::validate_plan(
            Some(&published),
            &unit(identity("before"), identity("before")),
            &published,
        ),
        Err(StorageError::Corrupt(_))
    ));
    // There is nothing yet to move.
    assert!(matches!(
        SessionSaveUnit::validate_plan(
            None,
            &unit(identity("before"), identity("after")),
            &saved(identity("after")),
        ),
        Err(StorageError::Corrupt(_))
    ));
    // The candidate must agree with the move, and nothing else may change.
    assert!(matches!(
        SessionSaveUnit::validate_plan(
            Some(&published),
            &unit(identity("before"), identity("after")),
            &published,
        ),
        Err(StorageError::Corrupt(_))
    ));
    SessionSaveUnit::validate_plan(
        Some(&published),
        &unit(identity("before"), identity("after")),
        &saved(identity("after")),
    )
    .unwrap();
}

#[tokio::test]
async fn a_refused_provider_identity_change_publishes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let storage: Arc<dyn SessionStorage> =
        Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    let id = SessionId::new("conversation").unwrap();
    prepared(storage.clone(), &id, identity("before"))
        .await
        .unwrap()
        .close(close_action())
        .await
        .unwrap();
    let lease = storage.open_existing(id.clone()).await.unwrap().unwrap();
    let binding = lease.load().await.unwrap().binding().clone();
    let refused = lease
        .save_changes(
            binding,
            saved(identity("after")),
            unit(identity("other"), identity("after")),
        )
        .await;
    assert!(matches!(refused, Err(StorageError::Corrupt(_))));
    let load = lease.load().await.unwrap();
    assert_eq!(load.state(), SessionLoadState::Published);
    assert_eq!(load.snapshot().unwrap().provider, identity("before"));
}

#[tokio::test]
async fn a_moved_identity_is_replayed_from_record_storage_and_restores() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions");
    let id = SessionId::new("conversation").unwrap();
    {
        let storage: Arc<dyn SessionStorage> = Arc::new(RecordStorage::new(&path).unwrap());
        prepared(storage.clone(), &id, identity("before"))
            .await
            .unwrap()
            .close(close_action())
            .await
            .unwrap();
        let lease = storage.open_existing(id.clone()).await.unwrap().unwrap();
        let saved = SavedProviderIdentity::load(lease.as_ref(), &id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.provider(), &identity("before"));
        assert_eq!(saved.state(), SessionLoadState::Published);
        saved
            .move_to(lease.as_ref(), identity("after"))
            .await
            .unwrap();
        drop(lease);
        storage.shutdown().await.unwrap();
    }
    // A new storage over the same files: the snapshot is rebuilt by replay.
    let storage: Arc<dyn SessionStorage> = Arc::new(RecordStorage::new(&path).unwrap());
    let lease = storage.open_existing(id.clone()).await.unwrap().unwrap();
    let replayed = SavedProviderIdentity::load(lease.as_ref(), &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replayed.provider(), &identity("after"));
    drop(replayed);
    drop(lease);
    assert!(matches!(
        prepared(storage.clone(), &id, identity("before")).await,
        Err(AgentError::Storage(StorageError::IdentityMismatch))
    ));
    prepared(storage.clone(), &id, identity("after"))
        .await
        .unwrap()
        .close(close_action())
        .await
        .unwrap();
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn moving_to_the_published_identity_is_refused_by_the_fold() {
    let directory = tempfile::tempdir().unwrap();
    let storage: Arc<dyn SessionStorage> =
        Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    let id = SessionId::new("conversation").unwrap();
    prepared(storage.clone(), &id, identity("before"))
        .await
        .unwrap()
        .close(close_action())
        .await
        .unwrap();
    let lease = storage.open_existing(id.clone()).await.unwrap().unwrap();
    let saved = SavedProviderIdentity::load(lease.as_ref(), &id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        saved.move_to(lease.as_ref(), identity("before")).await,
        Err(StorageError::Corrupt(_))
    ));
}

#[tokio::test]
async fn a_session_never_saved_has_no_identity_to_move() {
    let directory = tempfile::tempdir().unwrap();
    let storage: Arc<dyn SessionStorage> =
        Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    let id = SessionId::new("conversation").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    assert!(SavedProviderIdentity::load(lease.as_ref(), &id)
        .await
        .unwrap()
        .is_none());
}
