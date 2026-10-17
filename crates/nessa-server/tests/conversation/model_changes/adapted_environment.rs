//! An injected execution place changes binding identity without opening another native session.
use super::*;
use crate::conversation::application::{
    Environment, EnvironmentDeclaration, EnvironmentFuture, EnvironmentLease, GrantedTerms,
    LeaseHold, LeaseRelease,
};
use nessa_sdk::application::agent_execution::providers::HarnessHostIdentity;
use nessa_sdk::application::agent_execution::providers::{
    AgentProvider, ApprovalMode, ProviderIdentity, ProviderOpenFuture, ProviderOpenRequest,
};
use nessa_sdk::application::agent_execution::traces::LocalTraceStore;
use nessa_sdk::domain::agent_execution::leases::{
    EnvironmentRef, LeaseCleanup, LeaseEndCause, LeaseId, LeaseRefusal, LeaseTerms, SandboxProfiles,
};
use nessa_sdk::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use nessa_sdk::domain::model_metadata::value_objects::EffortLevel;
use std::sync::atomic::AtomicUsize;

struct AdaptedProvider(Arc<dyn AgentProvider>);
impl AgentProvider for AdaptedProvider {
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        self.0.harness_host_identity()
    }
    fn identity(&self) -> ProviderIdentity {
        let inner = self.0.identity();
        ProviderIdentity::new(
            inner.name(),
            inner.model_id(),
            format!("adapted:{}", inner.context()),
        )
        .unwrap()
    }
    fn model_change_context(&self) -> Option<String> {
        self.0
            .model_change_context()
            .map(|context| format!("same-place:{context}"))
    }
    fn approval_mode(&self) -> Option<ApprovalMode> {
        self.0.approval_mode()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        self.0.capabilities()
    }
    fn with_model_configuration(
        &self,
        mode: Option<ApprovalMode>,
        effort: Option<EffortLevel>,
        traces: Option<Arc<LocalTraceStore>>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        Ok(Arc::new(Self(
            self.0.with_model_configuration(mode, effort, traces)?,
        )))
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        self.0.open(request)
    }
}
struct AdaptedHold(Arc<AtomicUsize>);
impl LeaseHold for AdaptedHold {
    fn model_provider(
        &self,
        binding: Arc<dyn AgentProvider>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(AdaptedProvider(binding)))
    }
    fn ends(&self) -> Option<EnvironmentFuture<'static, LeaseEndCause>> {
        None
    }
    fn end(&self, _cause: LeaseEndCause) -> EnvironmentFuture<'_, LeaseRelease> {
        Box::pin(async { LeaseRelease::ByAgentClose })
    }
}
struct AdaptedEnvironment {
    opens: Arc<AtomicUsize>,
    targets: Arc<AtomicUsize>,
}
impl Environment for AdaptedEnvironment {
    fn declaration(&self) -> EnvironmentDeclaration {
        EnvironmentDeclaration {
            environment: EnvironmentRef::Here,
            sandbox: SandboxProfiles::HARNESS_DEFAULT,
        }
    }
    fn open<'a>(
        &'a self,
        _lease: &'a LeaseId,
        grant: &'a LeaseTerms,
        binding: Arc<dyn AgentProvider>,
    ) -> EnvironmentFuture<'a, Result<EnvironmentLease, LeaseRefusal>> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        let sandbox = grant.sandbox;
        let targets = self.targets.clone();
        Box::pin(async move {
            Ok(EnvironmentLease {
                provider: Arc::new(AdaptedProvider(binding)),
                hold: Arc::new(AdaptedHold(targets)),
                granted: GrantedTerms {
                    sandbox,
                    timeout_ms: None,
                    lease_ms: None,
                },
                workspace: None,
            })
        })
    }
    fn account<'a>(&'a self, _lease: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>> {
        Box::pin(async { Some(LeaseCleanup::NotHeld) })
    }
}

#[tokio::test]
async fn recovery_joins_captured_adapted_identities_instead_of_raw_registry_factories() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("metadata.sqlite3");
    let metadata = Arc::new(
        LocalConversationStore::open(&path)
            .unwrap()
            .with_access_policy(access_policy()),
    );
    let (mut service, provider, _, _, _) = mode_fixture();
    let opens = Arc::new(AtomicUsize::new(0));
    let targets = Arc::new(AtomicUsize::new(0));
    let inner = Arc::get_mut(&mut service.inner).unwrap();
    inner.metadata = metadata.clone();
    inner.environment = (Arc::new(AdaptedEnvironment {
        opens: opens.clone(),
        targets: targets.clone(),
    }) as Arc<dyn Environment>)
        .into();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(id.clone(), caller(), RequestedConversation::default())
        .await
        .unwrap();
    service.read(id.clone(), caller()).await.unwrap();
    let raw = nessa_local_database::rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch("CREATE TRIGGER refuse_adapted_terminal BEFORE UPDATE OF state ON model_requests BEGIN SELECT RAISE(ABORT,'controlled terminal failure'); END;").unwrap();
    let actor = caller();
    let target = ConversationModelId::new("second-model").unwrap();
    assert!(matches!(
        service
            .set_model(id.clone(), actor.clone(), target.clone())
            .await,
        Err(ConversationError::ModelChangeUncertain)
    ));
    let request = metadata.pending_model_change(&id).await.unwrap().unwrap();
    assert_eq!(request.prior_identity.context(), "adapted:test");
    assert_eq!(request.requested_identity.context(), "adapted:test");
    assert_ne!(request.requested_identity.context(), "test");
    let live = service.live_now(&id, &caller()).await.unwrap().unwrap();
    let snapshot = live.agent.session_manager().snapshot().await.unwrap();
    let native = snapshot.model_change.unwrap();
    assert_eq!(native.change().before(), &request.prior_identity);
    assert_eq!(native.change().after(), &request.requested_identity);
    raw.execute_batch("DROP TRIGGER refuse_adapted_terminal;")
        .unwrap();
    let view = service.read(id.clone(), caller()).await.unwrap();
    assert_eq!(view.selection.unwrap().model, "second-model");
    assert!(view.model_change.is_none());
    assert_eq!(targets.load(Ordering::SeqCst), 1);
    assert_eq!(opens.load(Ordering::SeqCst), 1);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.model_updates.lock().unwrap().len(), 1);
    assert_eq!(
        service
            .set_model(id, actor, target)
            .await
            .unwrap()
            .model
            .as_str(),
        "second-model"
    );
    service.shutdown().await.unwrap();
}
