//! Bare SDK roots acknowledge whole executable-use release, not only process exit.
use super::*;
use nessa_sdk::application::agent_execution::{agents::OwnedLifetime, subagents::*};
use nessa_sdk::domain::agent_execution::subagents::*;

struct NoFactory;
#[async_trait::async_trait]
impl ChildFactory for NoFactory {
    async fn prepare(&self, _: PrepareRequest) -> Result<PreparedChild, PrepareFailure> {
        panic!("the bare-root regression never prepares a child")
    }
}
struct Accept;
#[async_trait::async_trait]
impl OwnershipAudit for Accept {
    async fn record(&self, _: &OwnershipEvidence) -> Result<(), PortFailure> {
        Ok(())
    }
}
#[derive(Default)]
struct RootRoom {
    reservations: AtomicUsize,
    releases: AtomicUsize,
}
impl LiveRoom for RootRoom {
    fn try_reserve(&self) -> bool {
        self.reservations.fetch_add(1, Ordering::SeqCst);
        true
    }
    fn release(&self) {
        self.releases.fetch_add(1, Ordering::SeqCst);
    }
}
struct RootFixture {
    agent: Agent,
    backend: Arc<WorkflowBackend>,
    coordinator: OwnershipCoordinator,
    root: AgentLifetimeId,
    gate: Arc<dyn OwnedLifetime>,
    room: Arc<RootRoom>,
}
async fn owned_root() -> RootFixture {
    let room = Arc::new(RootRoom::default());
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
        room: room.clone(),
    });
    let root = coordinator
        .open_root(agent.session_manager().id().clone(), Initiator::Runtime)
        .await
        .unwrap();
    let gate = coordinator.participation(&root).unwrap();
    agent.install_owned_lifetime(gate.clone()).unwrap();
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(close_action()))
        .unwrap();
    bounded(agent.start_attachment(authorization).unwrap().wait())
        .await
        .unwrap();
    RootFixture {
        agent,
        backend,
        coordinator,
        root,
        gate,
        room,
    }
}

#[tokio::test]
async fn bare_owned_root_cannot_acknowledge_pending_executable_use_release() {
    let RootFixture {
        agent,
        backend,
        coordinator,
        root,
        gate,
        room,
    } = owned_root().await;
    assert!(agent.may_hold_provider_resources());
    assert_eq!(backend.shutdowns.lock().unwrap().len(), 0);
    *backend.cleanup.lock().unwrap() = CleanupReport::new(
        ResourceCleanup::ReleasePending {
            physical: CloseOutcome { forced: false },
            failure: AgentError::Configuration("executable-use release remains pending".into()),
        },
        Ok(()),
    );
    let first = bounded(agent.end_owned_lifetime(close_action())).await;
    let pending_state = coordinator.lifetime_state(&root);
    let pending_scope = gate.cleanup_scope().is_some();
    let pending_resources = agent.may_hold_provider_resources();
    let pending_backend_closed = *backend.closed.borrow();
    let pending_calls = backend.shutdowns.lock().unwrap().len();
    eprintln!(
        "bare-root pending: result={first:?}, graph={pending_state:?}, scope={pending_scope}, resources={pending_resources}, backend_closed={pending_backend_closed}, close_calls={pending_calls}"
    );
    // Preserve the original probe's real retry before its captured-state assertion.
    *backend.cleanup.lock().unwrap() = CleanupReport::confirmed(CloseOutcome { forced: false });
    let retry = bounded(agent.end_owned_lifetime(close_action())).await;
    let final_state = coordinator.lifetime_state(&root);
    eprintln!("bare-root actual retry: result={retry:?}, graph={final_state:?}");
    assert!(
        first.is_err(),
        "pending executable-use release is not a successful close"
    );
    assert!(
        pending_resources,
        "the real provider owner remains retained"
    );
    assert!(
        !pending_backend_closed,
        "the fixture has not acknowledged complete backend release"
    );
    assert_eq!(
        pending_calls, 1,
        "the first close polls this backend exactly once"
    );
    assert_eq!(
        pending_state,
        Some(LifetimeState::Closing),
        "physical process cleanup cannot close an owned root while executable-use release is pending"
    );
    assert!(
        pending_scope,
        "pending ownership retains its exact cleanup scope"
    );
    assert!(
        retry.is_ok(),
        "actual acknowledged release completes the original owner: {retry:?}"
    );
    assert_eq!(final_state, Some(LifetimeState::Closed));
    assert!(!agent.may_hold_provider_resources());
    assert!(*backend.closed.borrow());
    assert_eq!(backend.shutdowns.lock().unwrap().len(), 2);
    // Roots do not reserve child capacity. Do not infer child-capacity behavior.
    assert_eq!(room.reservations.load(Ordering::SeqCst), 0);
    assert_eq!(room.releases.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn bare_root_pending_release_waits_for_real_receipt_in_both_close_modes() {
    for suspend in [false, true] {
        for forced in [false, true] {
            for audit_failed in [false, true] {
                let RootFixture {
                    agent,
                    backend,
                    coordinator,
                    root,
                    gate,
                    room,
                } = owned_root().await;
                bounded(gate.seal_for_host(&close_action())).await.unwrap();
                let audit = if audit_failed {
                    Err(AgentError::AuditFailure)
                } else {
                    Ok(())
                };
                let pending = CleanupReport::new(
                    ResourceCleanup::ReleasePending {
                        physical: CloseOutcome { forced },
                        failure: AgentError::Configuration(
                            "executable-use release remains pending".into(),
                        ),
                    },
                    audit.clone(),
                );
                *backend.cleanup.lock().unwrap() = pending.clone();
                let first = if suspend {
                    bounded(agent.suspend_report(close_action())).await.unwrap()
                } else {
                    bounded(agent.close_report(close_action())).await.unwrap()
                };
                assert_eq!(first.cleanup(), &pending);
                assert!(first.cleanup().physical_outcome().is_some());
                assert!(!first.cleanup().is_confirmed());
                assert_eq!(
                    coordinator.lifetime_state(&root),
                    Some(LifetimeState::Closing)
                );
                assert!(gate.cleanup_scope().is_some());
                assert!(agent.may_hold_provider_resources());
                assert!(!*backend.closed.borrow());

                // This same backend withholds the actual release receipt. Merely
                // requesting its retry does not release the graph or SDK owner.
                let (acknowledge, held) = oneshot::channel();
                *backend.cleanup_gate.lock().unwrap() = Some(held);
                *backend.cleanup.lock().unwrap() = CleanupReport::new(
                    ResourceCleanup::Confirmed(CloseOutcome { forced }),
                    audit.clone(),
                );
                let mut retry = Box::pin(async {
                    if suspend {
                        agent.suspend_report(close_action()).await
                    } else {
                        agent.close_report(close_action()).await
                    }
                });
                bounded(async {
                    tokio::select! {
                        _ = backend.cleanup_entered.notified() => {},
                        _ = retry.as_mut() => panic!("cleanup cannot finish before its actual receipt"),
                    }
                }).await;
                assert_eq!(
                    coordinator.lifetime_state(&root),
                    Some(LifetimeState::Closing)
                );
                assert!(gate.cleanup_scope().is_some());
                assert!(agent.may_hold_provider_resources());
                assert!(!*backend.closed.borrow());
                assert_eq!(backend.shutdowns.lock().unwrap().len(), 2);
                acknowledge.send(()).unwrap();
                let retry = bounded(retry).await.unwrap();
                assert_eq!(
                    retry.cleanup().resources(),
                    &ResourceCleanup::Confirmed(CloseOutcome { forced })
                );
                assert_eq!(retry.cleanup().audit(), &audit);
                assert!(!agent.may_hold_provider_resources());
                assert!(*backend.closed.borrow());
                assert_eq!(
                    coordinator.lifetime_state(&root),
                    Some(if audit_failed {
                        LifetimeState::Closing
                    } else {
                        LifetimeState::Closed
                    }),
                    "a true release ACK cannot erase independent audit failure"
                );
                assert_eq!(gate.cleanup_scope().is_some(), audit_failed);
                assert_eq!(backend.shutdowns.lock().unwrap().len(), 2);
                assert_eq!(room.reservations.load(Ordering::SeqCst), 0);
                assert_eq!(room.releases.load(Ordering::SeqCst), 0);
            }
        }
    }
}

#[tokio::test]
async fn bare_root_unconfirmed_cleanup_does_not_gain_release_from_diagnostics() {
    for forced in [false, true] {
        let RootFixture {
            agent,
            backend,
            coordinator,
            root,
            gate,
            ..
        } = owned_root().await;
        bounded(gate.seal_for_host(&close_action())).await.unwrap();
        let diagnostic = AgentError::StorageDuringClose {
            error: nessa_sdk::application::agent_execution::sessions::StorageError::Io(
                "missing ACK".into(),
            ),
            cleanup_result: Box::new(Ok(CloseOutcome { forced })),
        };
        *backend.cleanup.lock().unwrap() =
            CleanupReport::new(ResourceCleanup::Unconfirmed(diagnostic), Ok(()));
        let first = bounded(agent.close_report(close_action())).await.unwrap();
        assert!(!first.cleanup().is_confirmed());
        assert_eq!(
            coordinator.lifetime_state(&root),
            Some(LifetimeState::Closing)
        );
        assert!(gate.cleanup_scope().is_some());
        assert!(agent.may_hold_provider_resources());
        *backend.cleanup.lock().unwrap() = CleanupReport::confirmed(CloseOutcome { forced });
        let retry = bounded(agent.close_report(close_action())).await.unwrap();
        assert!(retry.cleanup().is_confirmed());
        assert_eq!(
            coordinator.lifetime_state(&root),
            Some(LifetimeState::Closed)
        );
        assert!(!agent.may_hold_provider_resources());
        assert_eq!(backend.shutdowns.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn ordinary_suspend_does_not_implicitly_seal_a_bare_owned_root() {
    let RootFixture {
        agent,
        backend,
        coordinator,
        root,
        gate,
        ..
    } = owned_root().await;
    *backend.cleanup.lock().unwrap() = CleanupReport::new(
        ResourceCleanup::ReleasePending {
            physical: CloseOutcome { forced: false },
            failure: AgentError::Configuration("executable-use release remains pending".into()),
        },
        Ok(()),
    );
    let first = bounded(agent.suspend_report(close_action())).await.unwrap();
    assert!(!first.cleanup().is_confirmed());
    assert_eq!(coordinator.lifetime_state(&root), Some(LifetimeState::Open));
    assert!(!gate.is_sealed());
    assert!(agent.may_hold_provider_resources());
    *backend.cleanup.lock().unwrap() = CleanupReport::confirmed(CloseOutcome { forced: false });
    let retry = bounded(agent.suspend_report(close_action())).await.unwrap();
    assert!(retry.cleanup().is_confirmed());
    assert_eq!(coordinator.lifetime_state(&root), Some(LifetimeState::Open));
    assert!(!agent.may_hold_provider_resources());
    bounded(agent.end_owned_lifetime(close_action()))
        .await
        .unwrap();
    assert_eq!(
        coordinator.lifetime_state(&root),
        Some(LifetimeState::Closed)
    );
}
