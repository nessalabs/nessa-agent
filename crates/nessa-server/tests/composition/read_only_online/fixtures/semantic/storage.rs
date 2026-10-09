//! Observe actual producer candidates only after the real writer confirms their save.
use nessa_sdk::application::agent_execution::providers::ProviderIdentity;
use nessa_sdk::application::agent_execution::sessions::{
    CommittedSession, SessionChange, SessionLoad, SessionSaveGeneration, SessionSaveReceipt,
    SessionSaveUnit, SessionSnapshot, SessionStorage, SessionStorageLease, StorageError,
    StorageFuture,
};
use nessa_sdk::domain::agent_execution::sessions::{
    ExecutionSessionId, ProviderContext, SessionId,
};
use nessa_sdk::infrastructure::session_storage::RecordStorage;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ConfirmedSnapshot {
    pub(super) binding: SessionSaveGeneration,
    pub(super) snapshot: SessionSnapshot,
}

pub(super) struct RecordingStorage {
    records: Arc<RecordStorage>,
    confirmed: Arc<Mutex<Option<ConfirmedSnapshot>>>,
    changed: Arc<Notify>,
}
impl RecordingStorage {
    pub(super) fn new(records: Arc<RecordStorage>) -> Self {
        Self {
            records,
            confirmed: Arc::new(Mutex::new(None)),
            changed: Arc::new(Notify::new()),
        }
    }
    pub(super) fn records(&self) -> &RecordStorage {
        &self.records
    }
    pub(super) fn confirmed(&self) -> Option<ConfirmedSnapshot> {
        self.confirmed.lock().unwrap().clone()
    }
    /// Wakes a waiter after a confirmed snapshot is stored or cleared.
    pub(super) fn changed(&self) -> tokio::sync::futures::Notified<'_> {
        self.changed.notified()
    }
    fn lease(&self, inner: Box<dyn SessionStorageLease>) -> Box<dyn SessionStorageLease> {
        Box::new(RecordingLease {
            inner,
            confirmed: self.confirmed.clone(),
            changed: self.changed.clone(),
        })
    }
}
impl SessionStorage for RecordingStorage {
    fn read_committed(&self, id: SessionId) -> StorageFuture<'_, Option<CommittedSession>> {
        self.records.read_committed(id)
    }
    fn shutdown(&self) -> StorageFuture<'_, ()> {
        self.records.shutdown()
    }
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move { Ok(self.lease(self.records.open(id).await?)) })
    }
    fn open_existing(
        &self,
        id: SessionId,
    ) -> StorageFuture<'_, Option<Box<dyn SessionStorageLease>>> {
        Box::pin(async move {
            Ok(self
                .records
                .open_existing(id)
                .await?
                .map(|inner| self.lease(inner)))
        })
    }
}
struct RecordingLease {
    inner: Box<dyn SessionStorageLease>,
    confirmed: Arc<Mutex<Option<ConfirmedSnapshot>>>,
    changed: Arc<Notify>,
}
impl SessionStorageLease for RecordingLease {
    fn load(&self) -> StorageFuture<'_, SessionLoad> {
        self.inner.load()
    }
    fn save_changes(
        &self,
        binding: SessionSaveGeneration,
        snapshot: SessionSnapshot,
        units: Vec<SessionSaveUnit>,
    ) -> StorageFuture<'_, SessionSaveReceipt> {
        Box::pin(async move {
            let candidate = snapshot.clone();
            let receipt = self.inner.save_changes(binding, snapshot, units).await?;
            *self.confirmed.lock().unwrap() = Some(ConfirmedSnapshot {
                binding: receipt.next().clone(),
                snapshot: candidate,
            });
            self.changed.notify_one();
            Ok(receipt)
        })
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.inner.erase().await?;
            *self.confirmed.lock().unwrap() = None;
            self.changed.notify_one();
            Ok(())
        })
    }
}

#[tokio::test]
async fn confirmed_producer_snapshot_changes_only_after_successful_save() {
    let directory = tempfile::tempdir().unwrap();
    let records = Arc::new(RecordStorage::new(directory.path().join("records")).unwrap());
    let producer = RecordingStorage::new(records);
    let id = SessionId::new("confirmed-producer").unwrap();
    let lease = producer.open(id.clone()).await.unwrap();
    let provider = ProviderIdentity::new("fixture", "model", "workspace").unwrap();
    let snapshot = SessionSnapshot {
        id: id.clone(),
        provider: provider.clone(),
        provider_context: ProviderContext::Absent,
        invocations: vec![],
        queue_history: vec![],
        lease: None,
    };
    let receipt = lease
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            snapshot.clone(),
            vec![SessionSaveUnit::new(vec![SessionChange::Opened {
                id,
                provider,
                context: ProviderContext::Absent,
            }])
            .unwrap()],
        )
        .await
        .unwrap();
    let confirmed = producer.confirmed().unwrap();
    assert_eq!(confirmed.snapshot, snapshot);
    assert_eq!(confirmed.binding, *receipt.next());
    let requested = ProviderContext::Recorded(ExecutionSessionId::new("requested").unwrap());
    let different = ProviderContext::Recorded(ExecutionSessionId::new("contradictory").unwrap());
    let invalid = SessionSnapshot {
        provider_context: requested,
        ..snapshot
    };
    assert!(matches!(
        lease
            .save_changes(
                receipt.next().clone(),
                invalid,
                vec![SessionSaveUnit::new(vec![SessionChange::ProviderContext {
                    before: ProviderContext::Absent,
                    after: different
                }])
                .unwrap()]
            )
            .await,
        Err(StorageError::Corrupt(_))
    ));
    assert_eq!(producer.confirmed(), Some(confirmed));
    lease.erase().await.unwrap();
    assert_eq!(producer.confirmed(), None);
    drop(lease);
    producer.shutdown().await.unwrap();
}
