//! Agent participation: attachment close stays recoverable; owned close and disposal seal.
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use nessa_sdk::application::agent_execution::{
    agents::OwnedLifetime, permissions::ActionContext, providers::SessionCloseRequest,
};

use super::{actor, agent, AgentError};

struct RecordingGate {
    scope: Arc<Mutex<()>>,
    sealed: Arc<AtomicBool>,
    disposals: AtomicUsize,
    host_seals: AtomicUsize,
    notes: Mutex<Vec<(bool, bool)>>,
}

impl RecordingGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            scope: Arc::new(Mutex::new(())),
            sealed: Arc::new(AtomicBool::new(false)),
            disposals: AtomicUsize::new(0),
            host_seals: AtomicUsize::new(0),
            notes: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl OwnedLifetime for RecordingGate {
    fn admission_scope(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.scope)
    }
    fn seal(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.sealed)
    }
    fn seal_for_disposal(&self) {
        self.disposals.fetch_add(1, Ordering::SeqCst);
        self.sealed.store(true, Ordering::Release);
    }
    async fn seal_for_host(&self, _actor: &ActionContext) -> Result<(), AgentError> {
        self.host_seals.fetch_add(1, Ordering::SeqCst);
        self.sealed.store(true, Ordering::Release);
        Ok(())
    }
    async fn note_attachment(&self, released: bool, evidence_acknowledged: bool) {
        self.notes
            .lock()
            .expect("notes")
            .push((released, evidence_acknowledged));
    }
    async fn join_descendants(&self) -> Result<(), AgentError> {
        Ok(())
    }
}

#[tokio::test]
async fn c15_attachment_close_does_not_seal_or_block_reopen() {
    let agent = agent().await;
    let gate = RecordingGate::new();
    agent.install_owned_lifetime(gate.clone()).unwrap();
    assert!(agent.install_owned_lifetime(gate.clone()).is_err());
    agent.close(actor()).await.unwrap();
    assert!(!gate.sealed.load(Ordering::Acquire));
    assert_eq!(gate.disposals.load(Ordering::SeqCst), 0);
    assert_eq!(gate.host_seals.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn c16_owned_close_seals_and_refuses_a_later_attachment() {
    let agent = agent().await;
    let gate = RecordingGate::new();
    agent.install_owned_lifetime(gate.clone()).unwrap();
    agent.end_owned_lifetime(actor()).await.unwrap();
    assert!(gate.sealed.load(Ordering::Acquire));
    assert_eq!(gate.host_seals.load(Ordering::SeqCst), 1);
    assert!(matches!(
        agent.authorize_attachment(
            nessa_sdk::application::agent_execution::agents::AttachmentRequest::CallerRequested(
                actor()
            )
        ),
        Err(AgentError::Closed)
    ));
}

#[tokio::test]
async fn c18_disposal_seals_without_replacing_the_earlier_attachment_cause() {
    let agent = agent().await;
    let gate = RecordingGate::new();
    agent.install_owned_lifetime(gate.clone()).unwrap();
    let first = agent
        .inner
        .lifecycle
        .start_stop(SessionCloseRequest::ExecutionFailed);
    let joined = agent
        .inner
        .lifecycle
        .start_stop(SessionCloseRequest::SessionHandlesDropped);
    assert!(gate.sealed.load(Ordering::Acquire));
    assert_eq!(gate.disposals.load(Ordering::SeqCst), 1);
    assert_eq!(first.request, SessionCloseRequest::ExecutionFailed);
    assert_eq!(joined.request, SessionCloseRequest::ExecutionFailed);
}

mod factory_owned_gate {
    use super::{actor, agent, AgentError, Arc, AtomicUsize, Mutex, Ordering, OwnedLifetime};
    use async_trait::async_trait;
    use nessa_sdk::application::agent_execution::{
        agents::{Agent, AttachmentRequest},
        subagents::{
            ChildFactory, ChildResources, CloseCommand, InitialSubmit, LiveCapacity,
            MemoryOwnershipStore, OwnershipAudit, OwnershipCoordinator, OwnershipDependencies,
            OwnershipFailure, PortFailure, PrepareFailure, PrepareRequest, PreparedChild,
            ResourceReport, SpawnCommand,
        },
    };
    use nessa_sdk::domain::agent_execution::{
        sessions::SessionId,
        subagents::{
            ApprovalPolicy, EvidenceFact, HostActor, Initiator, LifetimeCause, OwnershipEvidence,
            PhysicalFact, PolicyRead, SpawnOrigin, SpawnRequestId, TaskReceiptId,
        },
    };
    struct AcceptOwnership;
    #[async_trait]
    impl OwnershipAudit for AcceptOwnership {
        async fn record(&self, _: &OwnershipEvidence) -> Result<(), PortFailure> {
            Ok(())
        }
    }
    struct AgentResources(Agent, Arc<AtomicUsize>);
    #[async_trait]
    impl ChildResources for AgentResources {
        async fn close(&self, _: &LifetimeCause, _: &Initiator) -> ResourceReport {
            self.1.fetch_add(1, Ordering::SeqCst);
            self.0.close(actor()).await.unwrap();
            ResourceReport {
                physical: PhysicalFact::Released,
                evidence: EvidenceFact::Acknowledged,
            }
        }
    }
    struct Submit;
    #[async_trait]
    impl InitialSubmit for Submit {
        async fn submit(&self, _: &str, _: &SpawnRequestId) -> Result<TaskReceiptId, PortFailure> {
            Ok(TaskReceiptId::new("accepted-receipt").unwrap())
        }
    }
    struct Factory {
        child: Mutex<Option<Agent>>,
        gate: Mutex<Option<Arc<dyn OwnedLifetime>>>,
        failed: bool,
        closes: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl ChildFactory for Factory {
        async fn prepare(&self, request: PrepareRequest) -> Result<PreparedChild, PrepareFailure> {
            let child = agent().await;
            child
                .install_owned_lifetime(request.owned_lifetime.clone())
                .unwrap();
            *self.gate.lock().unwrap() = Some(request.owned_lifetime);
            *self.child.lock().unwrap() = Some(child.clone());
            if self.failed {
                Err(PrepareFailure {
                    failure: PortFailure::Uncertain,
                    cleanup: Some(Arc::new(AgentResources(child, self.closes.clone()))),
                })
            } else {
                Ok(PreparedChild {
                    resources: Arc::new(AgentResources(child, self.closes.clone())),
                    submit: Arc::new(Submit),
                })
            }
        }
    }
    async fn actual_factory_gate(failed: bool) {
        let factory = Arc::new(Factory {
            child: Mutex::new(None),
            gate: Mutex::new(None),
            failed,
            closes: Arc::new(AtomicUsize::new(0)),
        });
        let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
            store: Arc::new(MemoryOwnershipStore::new()),
            audit: Arc::new(AcceptOwnership),
            factory: factory.clone(),
            room: Arc::new(LiveCapacity::new(1)),
        });
        let root = coordinator
            .open_root(
                SessionId::new("parent-session").unwrap(),
                Initiator::Runtime,
            )
            .await
            .unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            coordinator.spawn(SpawnCommand {
                parent: root.clone(),
                request_id: SpawnRequestId::new("child").unwrap(),
                task: "task".into(),
                policy: PolicyRead::Committed(
                    ApprovalPolicy::new("read-only", "ask", "revision").unwrap(),
                ),
                child_supports_policy: true,
                model: None,
                origin: SpawnOrigin::Host(HostActor::new("person", "desktop", "child").unwrap()),
            }),
        )
        .await
        .expect("factory gate startup must settle independently of close");
        if failed {
            assert_eq!(
                result,
                Err(OwnershipFailure::Startup(PortFailure::Uncertain))
            );
        } else {
            result.unwrap();
        }
        let lifetime = coordinator.children(&root, None, 1).unwrap().children[0]
            .lifetime
            .clone();
        let gate = factory.gate.lock().unwrap().clone().unwrap();
        let public = coordinator.participation(&lifetime).unwrap();
        assert!(Arc::ptr_eq(&gate.seal(), &public.seal()));
        assert!(Arc::ptr_eq(
            &gate.admission_scope(),
            &public.admission_scope()
        ));
        assert_eq!(gate.is_sealed(), failed);
        if failed {
            let child = factory.child.lock().unwrap().clone().unwrap();
            assert!(matches!(
                child.authorize_attachment(AttachmentRequest::CallerRequested(actor())),
                Err(AgentError::Closed)
            ));
        }
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            coordinator.end_lifetime(CloseCommand {
                lifetime: lifetime.clone(),
                cause: LifetimeCause::HostClose,
                initiator: Initiator::Runtime,
                external_attachment: false,
                timeout: None,
            }),
        )
        .await
        .expect("physical Agent report must not wait for its own coordinator generation")
        .unwrap();
        assert_eq!(factory.closes.load(Ordering::SeqCst), 1);
        assert_eq!(
            coordinator.lifetime_state(&lifetime),
            Some(nessa_sdk::domain::agent_execution::subagents::LifetimeState::Closed)
        );
        assert!(gate.is_sealed());
        let child = factory.child.lock().unwrap().clone().unwrap();
        assert!(matches!(
            child.authorize_attachment(AttachmentRequest::CallerRequested(actor())),
            Err(AgentError::Closed)
        ));
    }

    #[tokio::test]
    async fn row_34_factory_gate_installs_on_real_agent_and_shared_close_refuses_attachment() {
        actual_factory_gate(false).await;
    }
    #[tokio::test]
    async fn row_35_failed_factory_revokes_stale_real_agent_attachment_authority() {
        actual_factory_gate(true).await;
    }
}
