//! Public record-storage contract exercised through the SDK port.

use nessa_sdk::{
    application::agent_execution::{
        providers::ProviderIdentity,
        sessions::{ProviderContext, SessionChange, SessionSnapshot, SessionStorage, StorageError},
    },
    domain::agent_execution::sessions::SessionId,
    infrastructure::session_storage::RecordStorage,
};
use std::sync::Arc;

fn opened(id: &SessionId) -> (SessionChange, SessionSnapshot) {
    let provider = ProviderIdentity::new("fixture", "model", "workspace").unwrap();
    (
        SessionChange::Opened {
            id: id.clone(),
            provider: provider.clone(),
            context: ProviderContext::Absent,
        },
        SessionSnapshot {
            id: id.clone(),
            provider,
            provider_context: ProviderContext::Absent,
            invocations: Vec::new(),
            queue_history: Vec::new(),
        },
    )
}

#[tokio::test]
async fn record_storage_reopens_the_same_semantic_history() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    storage.initialize().await.unwrap();
    let id = SessionId::new("conversation").unwrap();
    assert!(storage.open_existing(id.clone()).await.unwrap().is_none());
    let lease = storage.open(id.clone()).await.unwrap();
    let (change, snapshot) = opened(&id);
    assert_eq!(
        lease.save(snapshot.clone()).await,
        Err(StorageError::ChangesRequired)
    );
    lease
        .save_changes(snapshot.clone(), vec![change])
        .await
        .unwrap();
    drop(lease);
    let restored = storage.open_existing(id).await.unwrap().unwrap();
    assert_eq!(restored.load().await.unwrap(), Some(snapshot));
    drop(restored);
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn record_storage_lease_excludes_second_manager_and_reset_erases_history() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    let id = SessionId::new("conversation").unwrap();
    let first = storage.open(id.clone()).await.unwrap();
    assert!(matches!(
        storage.open(id.clone()).await,
        Err(StorageError::Busy)
    ));
    let (change, snapshot) = opened(&id);
    first.save_changes(snapshot, vec![change]).await.unwrap();
    first.erase().await.unwrap();
    assert_eq!(first.load().await.unwrap(), None);
    drop(first);
    assert_eq!(
        storage
            .open_existing(id)
            .await
            .unwrap()
            .unwrap()
            .load()
            .await
            .unwrap(),
        None
    );
}
