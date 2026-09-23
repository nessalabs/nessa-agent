//! Deliberately unchecked storage proves application validation protects custom adapters.
use super::*;
use nessa_sdk::application::agent_execution::{
    agents::{Agent, AgentFuture, AttachmentRequest},
    executions::{ExecutionAudit, ExecutionAuditRecord},
    providers::{AgentProvider, ProviderOpenError, ProviderOpenFuture},
};
use nessa_sdk::application::dto::{ModalitiesDto, ModelMetadataDto};
use nessa_sdk::domain::{
    common::value_objects::TokenLimits,
    effective_capabilities::value_objects::{BindingRestrictions, EffectiveCapabilities},
    model_metadata::entities::ModelMetadata,
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
    capabilities: EffectiveCapabilities,
    opens: AtomicUsize,
}
impl AgentProvider for OpeningProbe {
    fn identity(&self) -> ProviderIdentity {
        self.identity.clone()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        &self.capabilities
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
struct AcceptingAudit;
impl ExecutionAudit for AcceptingAudit {
    fn record(&self, _record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

fn capabilities() -> EffectiveCapabilities {
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let model = ModelMetadata::try_from(ModelMetadataDto {
        provider: "fixture".into(),
        model_id: "model".into(),
        display_name: "Fixture".into(),
        input: text,
        image_input: None,
        output: text,
        tool_use: true,
        reasoning: false,
        max_context_window_tokens: 1000,
        max_output_tokens: 100,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
    })
    .unwrap();
    let features = model.features();
    let limits = TokenLimits::new(1000, 100).unwrap();
    EffectiveCapabilities::new(&model, BindingRestrictions::new(features, limits), limits).unwrap()
}

pub(super) async fn assert_custom_retention_admission(
    value: SessionSnapshot,
    accepted: bool,
) -> AgentError {
    let saves = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(OpeningProbe {
        identity: value.provider.clone(),
        capabilities: capabilities(),
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
    let prepared = Agent::prepare(provider.clone(), manager, Arc::new(AcceptingAudit)).await;
    let error = if accepted {
        let agent = prepared.expect("valid custom snapshot must prepare");
        let authorization = agent
            .authorize_attachment(AttachmentRequest::CallerRequested(
                ActionContext::new("test", "custom-storage", "attach").unwrap(),
            ))
            .unwrap();
        let error = agent
            .start_attachment(authorization)
            .unwrap()
            .wait()
            .await
            .expect_err("opening probe must reject attachment");
        assert_eq!(error, AgentError::Unsupported("opening probe".into()));
        error
    } else {
        let error = prepared
            .err()
            .expect("invalid snapshot must fail preparation");
        assert!(
            matches!(error.cause(), AgentError::Storage(StorageError::Corrupt(_))),
            "{:?}",
            error.cause()
        );
        error.cause().clone()
    };
    assert_eq!(provider.opens.load(Ordering::SeqCst), usize::from(accepted));
    assert_eq!(saves.load(Ordering::SeqCst), 0);
    let lease = storage.open(value.id.clone()).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
    error
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
        capabilities: capabilities(),
        opens: AtomicUsize::new(0),
    });
    let id = value.id.clone();
    let saves = Arc::new(AtomicUsize::new(0));
    let storage = Arc::new(MovingStorage {
        snapshot: Mutex::new(Some(value)),
        saves: saves.clone(),
    });
    let manager = SessionManager::open(Some(id), storage).await.unwrap();
    let prepared = Agent::prepare(provider.clone(), manager, Arc::new(AcceptingAudit)).await;
    if accepted {
        let agent = prepared.expect("valid moved snapshot must prepare");
        let authorization = agent
            .authorize_attachment(AttachmentRequest::CallerRequested(
                ActionContext::new("test", "moving-storage", "attach").unwrap(),
            ))
            .unwrap();
        assert!(matches!(
            agent.start_attachment(authorization).unwrap().wait().await,
            Err(AgentError::Unsupported(_))
        ));
    } else {
        let error = prepared
            .err()
            .expect("invalid snapshot must fail preparation");
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
            capabilities: capabilities(),
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
        let error = Agent::prepare(provider.clone(), manager, Arc::new(AcceptingAudit))
            .await
            .err()
            .unwrap();
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
