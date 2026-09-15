//! Deliberately unchecked storage proves application validation protects custom adapters.
use super::*;
use nessa_sdk::application::agent_execution::{
    agents::Agent,
    providers::{AgentProvider, ProviderOpenError, ProviderOpenFuture},
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

struct UncheckedStorage {
    exclusion: InMemoryStorage,
    snapshot: SessionSnapshot,
    saves: Arc<AtomicUsize>,
}
struct UncheckedLease {
    _exclusive: Box<dyn SessionStorageLease>,
    snapshot: SessionSnapshot,
    saves: Arc<AtomicUsize>,
}
impl SessionStorage for UncheckedStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(UncheckedLease {
                _exclusive: self.exclusion.open(id).await?,
                snapshot: self.snapshot.clone(),
                saves: self.saves.clone(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for UncheckedLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        Box::pin(async { Ok(Some(self.snapshot.clone())) })
    }
    fn save(&self, _: SessionSnapshot) -> StorageFuture<'_, ()> {
        self.saves.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(StorageError::Io("unexpected rewrite".into())) })
    }
}
struct OpeningProbe {
    identity: ProviderIdentity,
    opens: AtomicUsize,
}
impl AgentProvider for OpeningProbe {
    fn identity(&self) -> ProviderIdentity {
        self.identity.clone()
    }
    fn open(&self, _: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Err(ProviderOpenError::no_resources(AgentError::Unsupported(
                "opening probe".into(),
            )))
        })
    }
}

pub(super) async fn assert_custom_retention_admission(
    value: SessionSnapshot,
    accepted: bool,
) -> AgentError {
    let saves = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(OpeningProbe {
        identity: value.provider.clone(),
        opens: AtomicUsize::new(0),
    });
    let storage = Arc::new(UncheckedStorage {
        exclusion: InMemoryStorage::new(),
        snapshot: value.clone(),
        saves: saves.clone(),
    });
    let manager = SessionManager::open(Some(value.id.clone()), storage.clone())
        .await
        .unwrap();
    let error = Agent::new(provider.clone(), manager).await.err().unwrap();
    if accepted {
        assert_eq!(
            error.cause(),
            &AgentError::Unsupported("opening probe".into())
        );
    } else {
        assert!(
            matches!(error.cause(), AgentError::Storage(StorageError::Corrupt(_))),
            "{:?}",
            error.cause()
        );
    }
    assert_eq!(provider.opens.load(Ordering::SeqCst), usize::from(accepted));
    assert_eq!(saves.load(Ordering::SeqCst), 0);
    let lease = storage.open(value.id.clone()).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
    error.cause().clone()
}

// Unlike the usual fixture, this lease transfers the exact snapshot allocation.
// Cloning inside load would erase the oversized-capacity input we must exercise.
struct MovingStorage {
    snapshot: Mutex<Option<SessionSnapshot>>,
    saves: Arc<AtomicUsize>,
}
struct MovingLease {
    snapshot: Mutex<Option<SessionSnapshot>>,
    saves: Arc<AtomicUsize>,
}
impl SessionStorage for MovingStorage {
    fn open(&self, _: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async {
            Ok(Box::new(MovingLease {
                snapshot: Mutex::new(self.snapshot.lock().unwrap().take()),
                saves: self.saves.clone(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for MovingLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        Box::pin(async { Ok(self.snapshot.lock().unwrap().take()) })
    }
    fn save(&self, _: SessionSnapshot) -> StorageFuture<'_, ()> {
        self.saves.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(StorageError::Io("unexpected rewrite".into())) })
    }
}

pub(super) async fn assert_moved_retention_admission(value: SessionSnapshot, accepted: bool) {
    let provider = Arc::new(OpeningProbe {
        identity: value.provider.clone(),
        opens: AtomicUsize::new(0),
    });
    let id = value.id.clone();
    let saves = Arc::new(AtomicUsize::new(0));
    let storage = Arc::new(MovingStorage {
        snapshot: Mutex::new(Some(value)),
        saves: saves.clone(),
    });
    let manager = SessionManager::open(Some(id), storage).await.unwrap();
    let error = Agent::new(provider.clone(), manager).await.err().unwrap();
    if accepted {
        assert!(matches!(error.cause(), AgentError::Unsupported(_)));
    } else {
        assert!(
            matches!(error.cause(), AgentError::Storage(StorageError::Corrupt(_))),
            "{:?}",
            error.cause()
        );
    }
    assert_eq!(provider.opens.load(Ordering::SeqCst), usize::from(accepted));
    assert_eq!(saves.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn provider_identity_mismatch_from_custom_storage_prevents_open_and_rewrite() {
    for field in 0..3 {
        let value = snapshot("provider-identity-mismatch");
        let mut components = [
            value.provider.name(),
            value.provider.model_id(),
            value.provider.context(),
        ];
        components[field] = "different";
        let saves = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(OpeningProbe {
            identity: ProviderIdentity::new(components[0], components[1], components[2]).unwrap(),
            opens: AtomicUsize::new(0),
        });
        let storage = Arc::new(UncheckedStorage {
            exclusion: InMemoryStorage::new(),
            snapshot: value.clone(),
            saves: saves.clone(),
        });
        let manager = SessionManager::open(Some(value.id.clone()), storage.clone())
            .await
            .unwrap();
        let error = Agent::new(provider.clone(), manager).await.err().unwrap();
        assert_eq!(
            error.cause(),
            &AgentError::Storage(StorageError::IdentityMismatch)
        );
        assert_eq!(provider.opens.load(Ordering::SeqCst), 0);
        assert_eq!(saves.load(Ordering::SeqCst), 0);
        let lease = storage.open(value.id.clone()).await.unwrap();
        assert_same(&lease.load().await.unwrap().unwrap(), &value);
    }
}
