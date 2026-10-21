//! Independent cleanup evidence writers and the explicit erasure handoff.
use super::*;
use crate::conversation::infrastructure::ssh_environment::preparation_environment;
use nessa_sdk::application::agent_execution::providers::ResourceCleanup;
use nessa_sdk::application::agent_execution::sessions::{
    CommittedSession, LeaseRecord, SessionChange, SessionLoad, SessionSaveGeneration,
    SessionSaveReceipt, SessionSaveUnit, SessionStorage, SessionStorageLease, StorageFuture,
};
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use std::sync::atomic::AtomicUsize;

#[tokio::test]
async fn remote_ack_cannot_release_the_sdk_executable_use_owner() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (environment, host) = preparation_environment();
    let (service, storage, metadata) = fixture_with_sessions(
        &root,
        provider.clone(),
        environment.into(),
        crate::conversation_test_support::access_policy(),
        crate::conversation_test_support::claude_erasers(),
    );
    let coordinator = bind_with_capacity(&service, &storage, &root, true, 1).await;
    let target = id();
    opened(&service, &provider, &target).await;
    let lifetime = coordinator
        .active_lifetime_for_session(&conversation_session(&target))
        .unwrap();
    let opening = Arc::downgrade(
        service
            .inner
            .conversations
            .lock()
            .await
            .get(&target)
            .unwrap(),
    );
    for _ in 0..8 {
        provider
            .close_reports
            .lock()
            .unwrap()
            .push_back(CleanupReport::new(
                ResourceCleanup::ReleasePending {
                    physical: CloseOutcome { forced: false },
                    failure: AgentError::AuditFailure,
                },
                Ok(()),
            ));
    }
    let command = || CloseCommand {
        lifetime: lifetime.clone(),
        cause: LifetimeCause::HostClose,
        initiator: Initiator::Host(HostActor::new("person", "panel", "pending-sdk-use").unwrap()),
        external_attachment: false,
        timeout: None,
    };
    let closing = tokio::spawn({
        let coordinator = coordinator.clone();
        let command = command();
        async move { coordinator.end_lifetime(command).await }
    });
    host.wait_end().await;
    assert!(tokio::time::timeout(TURN_HOLD_HANG, closing)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    host.release();
    assert!(
        tokio::time::timeout(TURN_HOLD_HANG, coordinator.end_lifetime(command()))
            .await
            .unwrap()
            .is_err()
    );
    assert!(service.owns_unreleased_resources().await);
    assert!(opening.upgrade().is_some());
    let snapshot = SqliteOwnershipStore::open(&root.join("ownership.sqlite3"))
        .unwrap()
        .read()
        .await
        .unwrap();
    let graph = nessa_sdk::domain::agent_execution::subagents::OwnershipGraph::restore(snapshot);
    assert_eq!(
        graph.physical(&lifetime, &lifetime),
        Some(nessa_sdk::domain::agent_execution::subagents::PhysicalFact::Failed),
        "remote physical release cannot acknowledge the retained SDK executable-use owner"
    );
    provider.close_reports.lock().unwrap().clear();
    tokio::time::timeout(TURN_HOLD_HANG, coordinator.end_lifetime(command()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closed)
    );
    assert!(!service.owns_unreleased_resources().await);
    assert!(opening.upgrade().is_none());
    assert_eq!(host.end_requests(), 1);
    retire(service, storage, metadata).await;
}

struct CloseWrites {
    inner: Arc<RecordStorage>,
    target: SessionId,
    fail: Arc<AtomicBool>,
    rejected: Arc<AtomicUsize>,
    commit: bool,
    erasure: Option<Arc<ErasurePause>>,
}
struct CloseWriteLease {
    inner: Box<dyn SessionStorageLease>,
    fail: Arc<AtomicBool>,
    rejected: Arc<AtomicUsize>,
    commit: bool,
    erasure: Option<Arc<ErasurePause>>,
}
impl SessionStorage for CloseWrites {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            let inner = self.inner.open(id.clone()).await?;
            if id != self.target {
                return Ok(inner);
            }
            Ok(Box::new(CloseWriteLease {
                inner,
                fail: self.fail.clone(),
                rejected: self.rejected.clone(),
                commit: self.commit,
                erasure: self.erasure.clone(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
    fn read_committed(&self, id: SessionId) -> StorageFuture<'_, Option<CommittedSession>> {
        self.inner.read_committed(id)
    }
}
impl SessionStorageLease for CloseWriteLease {
    fn load(&self) -> StorageFuture<'_, SessionLoad> {
        self.inner.load()
    }
    fn save_changes(
        &self,
        binding: SessionSaveGeneration,
        units: Vec<SessionSaveUnit>,
    ) -> StorageFuture<'_, SessionSaveReceipt> {
        Box::pin(async move {
            let closing = units
                .iter()
                .flat_map(SessionSaveUnit::changes)
                .any(|change| {
                    matches!(
                        change,
                        SessionChange::Lease(
                            LeaseRecord::Ending { .. }
                                | LeaseRecord::Ended { .. }
                                | LeaseRecord::Interrupted { .. }
                                | LeaseRecord::CleanupReported { .. }
                        )
                    )
                });
            if closing && self.fail.load(Ordering::SeqCst) {
                self.rejected.fetch_add(1, Ordering::SeqCst);
                if self.commit {
                    self.inner.save_changes(binding, units).await?;
                }
                return Err(StorageError::Io(
                    "lease close write acknowledgement unavailable".into(),
                ));
            }
            self.inner.save_changes(binding, units).await
        })
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        Box::pin(async {
            if let Some(pause) = &self.erasure {
                pause.entered.notify_one();
                pause.release.notified().await;
            }
            self.inner.erase().await
        })
    }
}
async fn independent_evidence(audit_failure: bool, commit: bool) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (environment, host) = preparation_environment();
    let (mut service, storage, metadata) = fixture_with_sessions(
        &root,
        provider.clone(),
        environment.into(),
        crate::conversation_test_support::access_policy(),
        crate::conversation_test_support::claude_erasers(),
    );
    let target = id();
    let failed = Arc::new(AtomicBool::new(false));
    let rejected = Arc::new(AtomicUsize::new(0));
    {
        let inner = Arc::get_mut(&mut service.inner).unwrap();
        inner.storage = Arc::new(CloseWrites {
            inner: storage.clone(),
            target: conversation_session(&target),
            fail: failed.clone(),
            rejected: rejected.clone(),
            commit,
            erasure: None,
        });
    }
    let coordinator = bind_with_capacity(&service, &storage, &root, true, 1).await;
    opened(&service, &provider, &target).await;
    let lifetime = coordinator
        .active_lifetime_for_session(&conversation_session(&target))
        .unwrap();
    let opening = Arc::downgrade(
        service
            .inner
            .conversations
            .lock()
            .await
            .get(&target)
            .unwrap(),
    );
    let gate = coordinator.participation(&lifetime).unwrap();
    failed.store(!audit_failure, Ordering::SeqCst);
    if audit_failure {
        provider.close_reports.lock().unwrap().push_back(
            CleanupReport::confirmed(CloseOutcome { forced: false })
                .with_audit(Err(AgentError::AuditFailure)),
        );
    }
    let first = Initiator::Host(HostActor::new("person", "panel", "independent-evidence").unwrap());
    let command = |cause, initiator| CloseCommand {
        lifetime: lifetime.clone(),
        cause,
        initiator,
        external_attachment: false,
        timeout: None,
    };
    let closing = tokio::spawn({
        let coordinator = coordinator.clone();
        let command = command(LifetimeCause::HostClose, first.clone());
        async move { coordinator.end_lifetime(command).await }
    });
    host.wait_end().await;
    assert!(tokio::time::timeout(TURN_HOLD_HANG, closing)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    assert_eq!(host.held(), 1);
    assert!(opening.upgrade().is_some());
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closing)
    );
    assert!(if audit_failure {
        provider.close_calls.load(Ordering::SeqCst) == 1
            && provider.close_reports.lock().unwrap().is_empty()
    } else {
        rejected.load(Ordering::SeqCst) > 0
    });
    host.release();
    tokio::time::timeout(TURN_HOLD_HANG, async {
        while host.held() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        coordinator
            .end_lifetime(command(LifetimeCause::Deletion, Initiator::Runtime))
            .await
            .is_err(),
        "environment release cannot acknowledge independent evidence"
    );
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closing)
    );
    assert!(
        opening.upgrade().is_some(),
        "the actual writer survives physical release until evidence acknowledgement"
    );
    let owned = SqliteOwnershipStore::open(&root.join("ownership.sqlite3")).unwrap();
    let graph = nessa_sdk::domain::agent_execution::subagents::OwnershipGraph::restore(
        owned.read().await.unwrap(),
    );
    assert_eq!(
        graph.physical(&lifetime, &lifetime),
        Some(nessa_sdk::domain::agent_execution::subagents::PhysicalFact::Released)
    );
    assert_eq!(
        graph.close_initiator(&lifetime).cloned(),
        Some(first.clone())
    );
    if audit_failure {
        // The SDK's actual confirmed cleanup report still has failed audit.
        // A successful attachment callback cannot acknowledge that owner.
        gate.note_attachment(true, true).await;
        assert!(coordinator
            .end_lifetime(command(LifetimeCause::Deletion, Initiator::Runtime))
            .await
            .is_err());
        assert_eq!(
            coordinator.lifetime_state(&lifetime),
            Some(LifetimeState::Closing)
        );
        assert_eq!(host.end_requests(), 1);
        assert_eq!(provider.close_calls.load(Ordering::SeqCst), 1);
        host.close();
        let Err(ConversationError::Retirement(failures)) = service.shutdown().await else {
            panic!("an unavailable audit acknowledgement must stay an independent failure");
        };
        assert_eq!(failures.len(), 1);
        assert!(matches!(&failures[0].1, AgentError::AuditFailure));
        drop(service);
        storage.shutdown().await.unwrap();
        drop(storage);
        drop(metadata);
        drop(coordinator);
        // Cold physical proof cannot acknowledge the prior SDK audit failure.
        let (service, storage, metadata) = fixture(&root, provider);
        let restored = bind_with_capacity(&service, &storage, &root, true, 1).await;
        assert_eq!(
            restored.lifetime_state(&lifetime),
            Some(LifetimeState::Closing)
        );
        retire(service, storage, metadata).await;
        return;
    }
    failed.store(false, Ordering::SeqCst);
    coordinator
        .end_lifetime(command(LifetimeCause::Deletion, Initiator::Runtime))
        .await
        .unwrap();
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closed)
    );
    assert_eq!(coordinator.close_initiator(&lifetime), Some(first));
    assert!(
        opening.upgrade().is_none(),
        "acknowledged owner releases its exact Slot"
    );
    assert_eq!(host.end_requests(), 1);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 1);
    let snapshot = storage
        .read_committed(conversation_session(&target))
        .await
        .unwrap()
        .unwrap();
    let history = snapshot.snapshot().unwrap().lease.as_ref().unwrap();
    eprintln!(
        "close-evidence audit={audit_failure} commit={commit} records={:?}",
        history.records()
    );
    assert_eq!(
        history
            .records()
            .iter()
            .filter(|record| matches!(record, LeaseRecord::Ending { .. }))
            .count(),
        1
    );
    assert_eq!(
        history
            .records()
            .iter()
            .filter(|record| matches!(record, LeaseRecord::Interrupted { .. }))
            .count(),
        1
    );
    assert_eq!(
        history
            .records()
            .iter()
            .filter(|record| matches!(record, LeaseRecord::CleanupReported { .. }))
            .count(),
        1
    );
    host.close();
    retire(service, storage, metadata).await;
    drop(coordinator);
    let (service, storage, metadata) = fixture(&root, provider);
    let restored = bind_with_capacity(&service, &storage, &root, true, 1).await;
    assert_eq!(
        restored.lifetime_state(&lifetime),
        Some(LifetimeState::Closed)
    );
    retire(service, storage, metadata).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sdk_cleanup_audit_failure_survives_remote_release_callback_and_restart() {
    independent_evidence(true, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rejected_gateway_close_write_survives_remote_release_and_retries_its_owner() {
    independent_evidence(false, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lost_gateway_close_write_ack_survives_remote_release_and_retries_its_owner() {
    independent_evidence(false, true).await;
}

struct ErasurePause {
    entered: Notify,
    release: Notify,
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owned_deletion_hands_failed_records_to_erasure_without_early_ownership_ack() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (environment, host) = preparation_environment();
    let (mut service, storage, metadata) = fixture_with_sessions(
        &root,
        provider.clone(),
        environment.into(),
        crate::conversation_test_support::access_policy(),
        crate::conversation_test_support::claude_erasers(),
    );
    let target = id();
    let failed = Arc::new(AtomicBool::new(false));
    let rejected = Arc::new(AtomicUsize::new(0));
    let pause = Arc::new(ErasurePause {
        entered: Notify::new(),
        release: Notify::new(),
    });
    Arc::get_mut(&mut service.inner).unwrap().storage = Arc::new(CloseWrites {
        inner: storage.clone(),
        target: conversation_session(&target),
        fail: failed.clone(),
        rejected: rejected.clone(),
        commit: false,
        erasure: Some(pause.clone()),
    });
    let coordinator = bind_with_capacity(&service, &storage, &root, true, 1).await;
    opened(&service, &provider, &target).await;
    let lifetime = coordinator
        .active_lifetime_for_session(&conversation_session(&target))
        .unwrap();
    let opening = Arc::downgrade(
        service
            .inner
            .conversations
            .lock()
            .await
            .get(&target)
            .unwrap(),
    );
    failed.store(true, Ordering::SeqCst);
    host.release();
    let deleting = tokio::spawn({
        let service = service.clone();
        let target = target.clone();
        async move {
            service
                .delete(target, caller("delete-owned-record-debt"))
                .await
        }
    });
    tokio::time::timeout(TURN_HOLD_HANG, pause.entered.notified())
        .await
        .unwrap();
    assert!(rejected.load(Ordering::SeqCst) > 0);
    assert!(
        opening.upgrade().is_none(),
        "deletion hands the writer into actual erasure"
    );
    let first =
        Initiator::Host(HostActor::new("person", "panel", "while-erasure-pending").unwrap());
    let result = coordinator
        .end_lifetime(CloseCommand {
            lifetime: lifetime.clone(),
            cause: LifetimeCause::HostClose,
            initiator: first.clone(),
            external_attachment: false,
            timeout: None,
        })
        .await;
    assert!(result.is_err());
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closing)
    );
    pause.release.notify_one();
    assert!(tokio::time::timeout(TURN_HOLD_HANG, deleting)
        .await
        .unwrap()
        .unwrap()
        .unwrap());
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closed)
    );
    assert_eq!(coordinator.close_initiator(&lifetime), Some(first));
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 1);
    assert_eq!(host.end_requests(), 1);
    let erased = storage
        .read_committed(conversation_session(&target))
        .await
        .unwrap();
    assert!(
        erased
            .as_ref()
            .is_none_or(|history| history.snapshot().is_none()),
        "the acknowledged erasure leaves no conversation history"
    );
    host.close();
    retire(service, storage, metadata).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_mode_close_keeps_the_actual_owned_writer_until_save_ack() {
    use crate::conversation::application::{
        ConversationModeRequest, ConversationModeRequestState, ConversationRepository,
    };
    use nessa_protocol::conversation::domain::ConversationApprovalMode;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (environment, host) = preparation_environment();
    let (mut service, storage, metadata) = fixture_with_sessions(
        &root,
        provider.clone(),
        environment.into(),
        crate::conversation_test_support::access_policy(),
        crate::conversation_test_support::claude_erasers(),
    );
    let target = id();
    let failed = Arc::new(AtomicBool::new(false));
    Arc::get_mut(&mut service.inner).unwrap().storage = Arc::new(CloseWrites {
        inner: storage.clone(),
        target: conversation_session(&target),
        fail: failed.clone(),
        rejected: Arc::new(AtomicUsize::new(0)),
        commit: false,
        erasure: None,
    });
    let coordinator = bind_with_capacity(&service, &storage, &root, true, 1).await;
    opened(&service, &provider, &target).await;
    let lifetime = coordinator
        .active_lifetime_for_session(&conversation_session(&target))
        .unwrap();
    let opening = Arc::downgrade(
        service
            .inner
            .conversations
            .lock()
            .await
            .get(&target)
            .unwrap(),
    );
    let mode = caller("pending-mode");
    metadata
        .begin_mode_change(ConversationModeRequest {
            conversation_id: target.clone(),
            organization_id: mode.organization_id.clone(),
            request_id: "pending-mode".into(),
            initiator_principal_id: mode.principal_id.clone(),
            initiator_surface_id: mode.surface_id.clone(),
            prior: ConversationApprovalMode::Ask,
            requested: ConversationApprovalMode::Auto,
            state: ConversationModeRequestState::Pending,
            application: None,
            requested_at_ms: 1,
        })
        .await
        .unwrap();
    failed.store(true, Ordering::SeqCst);
    host.release();
    let closed = service
        .close(target.clone(), caller("pending-close-first"))
        .await;
    assert!(matches!(
        closed,
        Err(ConversationError::Agent(
            AgentError::StorageDuringClose { .. }
        ))
    ));
    assert!(
        opening.upgrade().is_some(),
        "upload handoff cannot drop the owned evidence writer"
    );
    failed.store(false, Ordering::SeqCst);
    coordinator
        .end_lifetime(CloseCommand {
            lifetime: lifetime.clone(),
            cause: LifetimeCause::HostClose,
            initiator: Initiator::Runtime,
            external_attachment: false,
            timeout: None,
        })
        .await
        .unwrap();
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closed)
    );
    assert!(opening.upgrade().is_none());
    let saved = storage
        .read_committed(conversation_session(&target))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.snapshot().unwrap().lease.as_ref().unwrap().ended_by(),
        Some(&caller("pending-close-first").actor().unwrap())
    );
    assert_eq!(host.end_requests(), 1);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 1);
    host.close();
    retire(service, storage, metadata).await;
}
