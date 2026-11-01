//! A refused opening closes the prepared owned Agent without inventing a lease.
use super::*;
use nessa_sdk::application::agent_execution::{sessions::LeaseRecord, subagents::*};
use nessa_sdk::domain::agent_execution::subagents::Initiator;
use nessa_sdk::domain::agent_execution::{leases::*, subagents::*};

#[tokio::test]
async fn owned_close_after_refused_lease_keeps_refusal_without_opening_provider() {
    struct NoFactory;
    #[async_trait::async_trait]
    impl ChildFactory for NoFactory {
        async fn prepare(&self, _: PrepareRequest) -> Result<PreparedChild, PrepareFailure> {
            panic!("this prepared root does not create a child")
        }
    }
    struct Accept;
    #[async_trait::async_trait]
    impl OwnershipAudit for Accept {
        async fn record(&self, _: &OwnershipEvidence) -> Result<(), PortFailure> {
            Ok(())
        }
    }
    let storage = MemoryStorage::default();
    let backend = workflow_backend();
    let agent = Agent::prepare(
        Arc::new(WorkflowProvider(backend.clone())),
        storage.manager().await,
        backend.audit.clone(),
    )
    .await
    .unwrap();
    let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: Arc::new(MemoryOwnershipStore::new()),
        audit: Arc::new(Accept),
        factory: Arc::new(NoFactory),
        room: Arc::new(LiveCapacity::new(1)),
    });
    let root = coordinator
        .open_root(agent.session_manager().id().clone(), Initiator::Runtime)
        .await
        .unwrap();
    let gate = coordinator.participation(&root).unwrap();
    let scope = gate.cleanup_scope().unwrap();
    agent.install_owned_lifetime(gate).unwrap();
    agent
        .session_manager()
        .record_lease(|_| {
            (
                vec![LeaseRecord::Refused {
                    lease: LeaseId::new("never-attached-refused-lease").unwrap(),
                    revision: LeaseRevision::FIRST,
                    terms: LeaseTerms {
                        environment: EnvironmentRef::Here,
                        work: LeaseWork::Agent(AgentWork::new("claude", "sonnet").unwrap()),
                        sandbox: SandboxProfile::HarnessDefault,
                        grants: LeaseGrants::Opening,
                        deadline: LeaseDeadline::UntilEnded,
                    },
                    refusal: LeaseRefusal::EnvironmentUnreachable,
                    actor: actor(),
                }],
                (),
            )
        })
        .await
        .unwrap()
        .saved
        .unwrap();
    let before = storage.snapshot().lease.unwrap();
    assert!(backend.receiver.lock().unwrap().is_some());
    bounded(agent.close_report(actor()))
        .await
        .unwrap()
        .into_result()
        .unwrap();
    assert!(
        backend.receiver.lock().unwrap().is_some(),
        "provider was never opened"
    );
    assert!(backend.executions.lock().unwrap().is_empty());
    assert_eq!(storage.snapshot().lease, Some(before.clone()));
    assert!(!before.sdk_cleanup_acknowledged(&scope));
}
