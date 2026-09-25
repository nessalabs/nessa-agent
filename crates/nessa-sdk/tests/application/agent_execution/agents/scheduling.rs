//! Failed queue settlement stays complete and preserves the first stop owner.
use super::*;
use crate::application::agent_execution::{
    agents::AgentFuture,
    executions::{ExecutionAudit, QueueSettlementRecord},
    providers::{
        AgentProvider, ProviderIdentity, ProviderOpenError, ProviderOpenFuture, ProviderOpenRequest,
    },
    sessions::{
        SessionManager, SessionSnapshot, SessionStorage, SessionStorageLease, StorageFuture,
    },
};
use crate::application::dto::{ModalitiesDto, ModelMetadataDto};
use crate::domain::agent_execution::{
    executions::{QueueMutation, QueueRemovalCause, SchedulingInitiator},
    prompts::{PromptText, UserMessage},
    sessions::SessionId,
};
use crate::domain::{
    effective_capabilities::value_objects::{BindingRestrictions, EffectiveCapabilities},
    model_metadata::{
        entities::ModelMetadata,
        value_objects::{Modalities, ModelFeatures},
    },
};
use crate::infrastructure::session_storage::InMemoryStorage;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use tokio::sync::oneshot;

#[derive(Default)]
struct SettlementPanickingAudit {
    panic_once: AtomicBool,
    settlements: Mutex<Vec<QueueSettlementRecord>>,
}

struct PausingSettlementAudit {
    settlements: Mutex<Vec<QueueSettlementRecord>>,
    gate: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

struct TestProvider;

impl AgentProvider for TestProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("failed-pending", "fixture", "fixture").unwrap()
    }

    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities()
    }

    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async { Err(ProviderOpenError::no_resources(AgentError::Closed)) })
    }
}

fn capabilities() -> &'static EffectiveCapabilities {
    static CAPABILITIES: OnceLock<EffectiveCapabilities> = OnceLock::new();
    CAPABILITIES.get_or_init(|| {
        let text = ModalitiesDto {
            text: true,
            image: false,
            audio: false,
        };
        let model = ModelMetadata::try_from(ModelMetadataDto {
            provider: "anthropic".into(),
            model_id: "fixture".into(),
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
        let text = Modalities::new(true, false, false).unwrap();
        EffectiveCapabilities::new(
            &model,
            BindingRestrictions::new(ModelFeatures::new(text, text, true, false), model.limits()),
            model.limits(),
        )
        .unwrap()
    })
}

impl ExecutionAudit for PausingSettlementAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            if let ExecutionAuditRecord::QueueSettled(record) = record {
                self.settlements.lock().unwrap().push(record);
                let gate = self.gate.lock().unwrap().take();
                if let Some((entered, release)) = gate {
                    entered.send(()).unwrap();
                    release.await.unwrap();
                }
            }
            Ok(())
        })
    }
}

impl ExecutionAudit for SettlementPanickingAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            if let ExecutionAuditRecord::QueueSettled(record) = record {
                self.settlements.lock().unwrap().push(record);
                if !self.panic_once.swap(true, Ordering::SeqCst) {
                    panic!("first queue settlement audit panicked");
                }
            }
            Ok(())
        })
    }
}

struct PanickingQueueStorage {
    inner: InMemoryStorage,
    mutation: Arc<Mutex<Option<QueueMutation>>>,
}

struct PanickingQueueLease {
    inner: Box<dyn SessionStorageLease>,
    mutation: Arc<Mutex<Option<QueueMutation>>>,
}

impl SessionStorage for PanickingQueueStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(PanickingQueueLease {
                inner: self.inner.open(id).await?,
                mutation: self.mutation.clone(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}

impl SessionStorageLease for PanickingQueueLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.inner.load()
    }

    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        let should_panic = self
            .mutation
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|mutation| {
                snapshot
                    .queue_history
                    .last()
                    .is_some_and(|entry| &entry.mutation == mutation)
            });
        if should_panic {
            self.mutation.lock().unwrap().take();
            return Box::pin(async { panic!("queue membership persistence panic") });
        }
        self.inner.save(snapshot)
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.inner.erase()
    }
}

async fn prepared(storage: Arc<dyn SessionStorage>, audit: Arc<dyn ExecutionAudit>) -> Agent {
    let manager = SessionManager::open(None, storage).await.unwrap();
    Agent::prepare(Arc::new(TestProvider), manager, audit)
        .await
        .unwrap()
}

fn request(id: &str) -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new(id).unwrap(),
        user_message: UserMessage::text_only(PromptText::new("queued input").unwrap()),
        estimated_input_tokens: 2,
        reserved_output_tokens: 10,
    }
}

fn actor() -> ActionContext {
    ActionContext::new("caller", "test", "failed-pending").unwrap()
}

#[tokio::test]
async fn failed_attachment_settlement_panics_do_not_discard_the_queue_tail() {
    let mutation = Arc::new(Mutex::new(None));
    let storage = Arc::new(PanickingQueueStorage {
        inner: InMemoryStorage::new(),
        mutation: mutation.clone(),
    });
    let audit = Arc::new(SettlementPanickingAudit::default());
    let agent = prepared(storage, audit.clone()).await;
    let _invocation = agent.inner.invocation.lock().await;
    let first_id = ExecutionId::new("failed-attachment-first").unwrap();
    let first = agent
        .enqueue(request(first_id.as_str()), actor())
        .await
        .unwrap();
    let second = agent
        .enqueue(request("failed-attachment-second"), actor())
        .await
        .unwrap();
    mutation.lock().unwrap().replace(QueueMutation::Removed {
        id: first_id,
        cause: QueueRemovalCause::DispatchFailed,
    });
    let failure = AgentError::Protocol("automatic attachment failed".into());
    let mut scheduler = agent.inner.scheduler.lock().await;
    let aggregate = agent
        .settle_failed_pending(&mut scheduler, failure.clone())
        .await
        .unwrap_err();
    drop(scheduler);

    let first = first.wait().await;
    let second = second.wait().await;
    assert!(format!("{first:?}").contains("automatic attachment failed"));
    assert!(format!("{first:?}").contains("AuditFailure"));
    assert!(format!("{first:?}").contains("persistence panicked"));
    assert_eq!(second, Err(failure));
    assert!(format!("{aggregate:?}").contains("AuditFailure"));
    assert!(format!("{aggregate:?}").contains("persistence panicked"));
    assert_eq!(audit.settlements.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn automatic_failure_claim_precedes_close_while_settlement_audit_waits() {
    let (entered, waiting) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let audit = Arc::new(PausingSettlementAudit {
        settlements: Mutex::new(Vec::new()),
        gate: Mutex::new(Some((entered, released))),
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit.clone()).await;
    let _invocation = agent.inner.invocation.lock().await;
    let queued = agent
        .enqueue(request("failure-before-close"), actor())
        .await
        .unwrap();
    let failure = AgentError::Protocol("automatic attachment failed".into());
    let settlement = tokio::spawn({
        let agent = agent.clone();
        let failure = failure.clone();
        async move {
            let mut scheduler = agent.inner.scheduler.lock().await;
            agent.settle_failed_pending(&mut scheduler, failure).await
        }
    });
    waiting.await.unwrap();
    let closer = ActionContext::new("closer", "test", "close-after-claim").unwrap();
    let attempt = agent.start_shutdown(SessionCloseRequest::Explicit(closer));
    release.send(()).unwrap();

    assert_eq!(settlement.await.unwrap(), Ok(()));
    assert_eq!(queued.wait().await, Err(failure));
    {
        let records = audit.settlements.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].cause(), SchedulingCause::DispatchFailed);
        assert_eq!(records[0].initiator(), SchedulingInitiator::Automatic);
        assert_eq!(records[0].submitted_by(), &actor());
        assert_eq!(records[0].initiated_by(), None);
    }
    agent.inner.lifecycle.complete_stop(&attempt).await;
}

#[tokio::test]
async fn explicit_close_precedes_unclaimed_automatic_failure_settlement() {
    let audit = Arc::new(PausingSettlementAudit {
        settlements: Mutex::new(Vec::new()),
        gate: Mutex::new(None),
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit.clone()).await;
    let _invocation = agent.inner.invocation.lock().await;
    let submitted_by = actor();
    let queued = agent
        .enqueue(request("close-before-failure"), submitted_by.clone())
        .await
        .unwrap();
    let closer = ActionContext::new("closer", "test", "close-before-claim").unwrap();
    let attempt = agent.start_shutdown(SessionCloseRequest::Explicit(closer.clone()));
    let mut scheduler = agent.inner.scheduler.lock().await;
    assert_eq!(
        agent
            .settle_failed_pending(
                &mut scheduler,
                AgentError::Protocol("later automatic attachment failure".into()),
            )
            .await,
        Ok(())
    );
    drop(scheduler);

    assert_eq!(queued.wait().await, Err(AgentError::Closed));
    {
        let records = audit.settlements.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].cause(), SchedulingCause::SessionClosed);
        assert_eq!(records[0].initiator(), SchedulingInitiator::Caller);
        assert_eq!(records[0].submitted_by(), &submitted_by);
        assert_eq!(records[0].initiated_by(), Some(&closer));
    }
    agent.inner.lifecycle.complete_stop(&attempt).await;
}
