//! Original decision receipt survives both storage representations and atomic refusal.
use super::{history_fixture, Snapshot, SnapshotRef};
use crate::application::agent_execution::{
    permissions::ActionContext,
    sessions::{
        records::continuation::Continuation, DeclineDecision, InvocationSchedulingEvent,
        SessionChange,
    },
};
use crate::domain::agent_execution::executions::{
    InvocationKind, InvocationStage, SchedulingCause,
};

#[test]
fn decline_receipt_checkpoint_semantic_and_atomic_rollback_preserve_original_owner() {
    let mut original = history_fixture(1);
    let record = &mut original.invocations[0];
    record.submission = crate::application::agent_execution::executions::SubmissionMode::Queued;
    let execution_id = record.request.execution_id.clone();
    record.scheduling = vec![
        InvocationSchedulingEvent {
            kind: InvocationKind::Queued,
            target: None,
            before: None,
            stage: InvocationStage::Queued,
            cause: SchedulingCause::Submitted,
            actor: Some(record.actor.clone()),
        },
        InvocationSchedulingEvent {
            kind: InvocationKind::Queued,
            target: None,
            before: Some(InvocationStage::Queued),
            stage: InvocationStage::Cancelled,
            cause: SchedulingCause::Interrupted,
            actor: None,
        },
    ];
    let event = InvocationSchedulingEvent {
        kind: InvocationKind::Queued,
        target: None,
        before: Some(InvocationStage::Cancelled),
        stage: InvocationStage::Cancelled,
        cause: SchedulingCause::NotResumed,
        actor: Some(ActionContext::new("original", "panel", "first-close").unwrap()),
    };
    let prepare = SessionChange::DeclinePrepared {
        execution_id: execution_id.clone(),
        decision: DeclineDecision::new(event.clone(), false),
    };
    let terminal = SessionChange::SchedulingTransition {
        execution_id: execution_id.clone(),
        event,
    };
    let ack = SessionChange::DeclineAcknowledged { execution_id };
    let mut continuation = Continuation::restore(Some(original.clone())).unwrap();
    let bytes = continuation.snapshot_bytes;
    assert!(continuation
        .apply_unit(&[prepare.clone(), ack.clone()])
        .is_err());
    assert_eq!(continuation.snapshot.as_ref(), Some(&original));
    assert_eq!(
        continuation.snapshot_bytes, bytes,
        "refused unit leaves no retained preparation"
    );
    for change in [prepare.clone(), terminal, ack.clone()] {
        let encoded = super::super::semantic::encode_batch(std::slice::from_ref(&change)).unwrap();
        let decoded =
            super::super::semantic::decode_batch(&encoded, &original.provider_context).unwrap();
        assert_eq!(decoded, vec![change]);
        continuation.apply_unit(&decoded).unwrap();
        let snapshot = continuation.snapshot.as_ref().unwrap();
        let checkpoint = serde_json::to_vec(&SnapshotRef(snapshot)).unwrap();
        let restored: Snapshot = serde_json::from_slice(&checkpoint).unwrap();
        assert_eq!(&restored.decode().unwrap(), snapshot);
        let before = snapshot.clone();
        let retained = continuation.snapshot_bytes;
        let refused = if before.invocations[0]
            .decline
            .as_ref()
            .unwrap()
            .acknowledged()
        {
            ack.clone()
        } else {
            prepare.clone()
        };
        assert!(continuation.apply_unit(&[refused]).is_err());
        assert_eq!(continuation.snapshot.as_ref(), Some(&before));
        assert_eq!(continuation.snapshot_bytes, retained);
    }
}

#[test]
fn sdk_cleanup_acknowledgement_requires_exact_lease_target_and_atomic_history() {
    use crate::application::agent_execution::agents::OwnedCleanupScope;
    use crate::application::agent_execution::sessions::{CurrentLease, LeaseRecord};
    use crate::domain::agent_execution::leases::{
        AgentWork, EnvironmentRef, LeaseDeadline, LeaseEndCause, LeaseGrants, LeaseId,
        LeaseRevision, LeaseTerms, LeaseWork, SandboxProfile,
    };
    let id = |value: &str| LeaseId::new(value).unwrap();
    let issued = |lease: &str, revision: u64| LeaseRecord::Issued {
        lease: id(lease),
        revision: LeaseRevision::new(revision).unwrap(),
        terms: LeaseTerms {
            environment: EnvironmentRef::Here,
            work: LeaseWork::Agent(AgentWork::new("claude", "sonnet").unwrap()),
            sandbox: SandboxProfile::HarnessDefault,
            grants: LeaseGrants::Opening,
            deadline: LeaseDeadline::UntilEnded,
        },
        actor: ActionContext::new("person", "panel", "open").unwrap(),
    };
    let ending = |lease: &str, cause| LeaseRecord::Ending {
        lease: id(lease),
        cause,
        actor: None,
    };
    let fold = |records: &[LeaseRecord]| {
        records
            .iter()
            .try_fold(None, |current, record| {
                CurrentLease::apply(current.as_ref(), record).map(Some)
            })
            .map(|current| current.unwrap())
    };
    use crate::application::agent_execution::sessions::{
        records::continuation::Continuation, SessionChange,
    };
    use crate::domain::agent_execution::{sessions::SessionId, subagents::AgentLifetimeId};
    use crate::infrastructure::session_storage::snapshot::{
        checkpoint::{history_fixture, Snapshot, SnapshotRef},
        decode_semantic_batch, encode_semantic_batch,
    };
    let lifetime = AgentLifetimeId::new("target").unwrap();
    let scope = OwnedCleanupScope::new(lifetime.clone(), SessionId::new("conversation").unwrap());
    let proof = LeaseRecord::SdkCleanupAcknowledged {
        lease: id("lease-1"),
        lifetime,
    };
    assert!(
        fold(&[issued("lease-1", 1), proof.clone()])
            .unwrap()
            .sdk_cleanup_acknowledged(&scope),
        "SDK cleanup proof is independent of a later remote End"
    );
    let before = fold(&[
        issued("lease-1", 1),
        ending("lease-1", LeaseEndCause::Closed),
    ])
    .unwrap();
    let mut snapshot = history_fixture(0);
    snapshot.lease = Some(before);
    let mut continuation = Continuation::restore(Some(snapshot.clone())).unwrap();
    let bytes = continuation.snapshot_bytes;
    let bad = LeaseRecord::SdkCleanupAcknowledged {
        lease: id("another-lease"),
        lifetime: scope.lifetime().clone(),
    };
    // Refuse foreign identity before an existing ACK can also reject a
    // duplicate lifetime. The original two-record rollback oracle stays below.
    assert!(
        continuation
            .apply_unit(&[SessionChange::Lease(bad.clone())])
            .is_err(),
        "a first SDK cleanup ACK cannot name another lease"
    );
    assert_eq!(continuation.snapshot.as_ref(), Some(&snapshot));
    assert_eq!(continuation.snapshot_bytes, bytes);
    assert!(continuation
        .apply_unit(&[
            SessionChange::Lease(proof.clone()),
            SessionChange::Lease(bad)
        ])
        .is_err());
    assert_eq!(continuation.snapshot.as_ref(), Some(&snapshot));
    assert_eq!(continuation.snapshot_bytes, bytes);
    let change = SessionChange::Lease(proof);
    let encoded = encode_semantic_batch(std::slice::from_ref(&change)).unwrap();
    let decoded = decode_semantic_batch(&encoded, &snapshot.provider_context).unwrap();
    assert_eq!(decoded, vec![change]);
    continuation.apply_unit(&decoded).unwrap();
    let acknowledged = continuation.snapshot.as_ref().unwrap();
    let current = acknowledged.lease.as_ref().unwrap();
    assert!(current.sdk_cleanup_acknowledged(&scope));
    let wrong = OwnedCleanupScope::new(
        AgentLifetimeId::new("other-target").unwrap(),
        scope.session().clone(),
    );
    assert!(!current.sdk_cleanup_acknowledged(&wrong));
    let renewed = fold(&[
        issued("lease-1", 1),
        ending("lease-1", LeaseEndCause::Closed),
        acknowledged
            .lease
            .as_ref()
            .unwrap()
            .records()
            .last()
            .unwrap()
            .clone(),
        LeaseRecord::Ended {
            lease: id("lease-1"),
            cleanup: crate::domain::agent_execution::leases::LeaseCleanup::Confirmed {
                forced: false,
            },
        },
        issued("lease-2", 2),
    ])
    .unwrap();
    assert!(
        !renewed.sdk_cleanup_acknowledged(&scope),
        "a previous lease's real cleanup cannot release a new acquisition"
    );
    let encoded = serde_json::to_vec(&SnapshotRef(acknowledged)).unwrap();
    let decoded: Snapshot = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(&decoded.decode().unwrap(), acknowledged);
    assert!(continuation
        .apply_unit(&[SessionChange::Lease(
            acknowledged
                .lease
                .as_ref()
                .unwrap()
                .records()
                .last()
                .unwrap()
                .clone()
        )])
        .is_err());

    let retired = SessionChange::Lease(LeaseRecord::SdkCleanupRetired {
        lease: id("lease-1"),
        lifetime: scope.lifetime().clone(),
    });
    let before = continuation.snapshot.clone();
    let bad_retirement = SessionChange::Lease(LeaseRecord::SdkCleanupRetired {
        lease: id("wrong-lease"),
        lifetime: scope.lifetime().clone(),
    });
    assert!(continuation.apply_unit(&[bad_retirement]).is_err());
    assert_eq!(
        continuation.snapshot, before,
        "wrong-lease retirement is atomic"
    );
    let encoded = encode_semantic_batch(std::slice::from_ref(&retired)).unwrap();
    let decoded = decode_semantic_batch(&encoded, &snapshot.provider_context).unwrap();
    assert_eq!(decoded, vec![retired.clone()]);
    continuation.apply_unit(&decoded).unwrap();
    let checkpoint =
        serde_json::to_vec(&SnapshotRef(continuation.snapshot.as_ref().unwrap())).unwrap();
    let restored: Snapshot = serde_json::from_slice(&checkpoint).unwrap();
    let restored = restored.decode().unwrap();
    assert!(
        !restored
            .lease
            .as_ref()
            .unwrap()
            .sdk_cleanup_acknowledged(&scope),
        "restart cannot recover the retired acquisition's proof"
    );
    let mut restarted = Continuation::restore(Some(restored)).unwrap();
    assert!(
        restarted.apply_unit(&[retired]).is_err(),
        "retirement needs a current ACK"
    );
    restarted
        .apply_unit(&[SessionChange::Lease(LeaseRecord::SdkCleanupAcknowledged {
            lease: id("lease-1"),
            lifetime: scope.lifetime().clone(),
        })])
        .unwrap();
    assert!(restarted
        .snapshot
        .as_ref()
        .unwrap()
        .lease
        .as_ref()
        .unwrap()
        .sdk_cleanup_acknowledged(&scope));
}

#[test]
fn cancellation_receipt_semantic_checkpoint_and_refusal_keep_original_input() {
    use crate::application::agent_execution::sessions::CancellationDecision;
    let mut original = history_fixture(1);
    let record = &mut original.invocations[0];
    record.submission = crate::application::agent_execution::executions::SubmissionMode::Queued;
    record.scheduling = vec![InvocationSchedulingEvent {
        kind: InvocationKind::Queued,
        target: None,
        before: None,
        stage: InvocationStage::Queued,
        cause: SchedulingCause::Submitted,
        actor: Some(record.actor.clone()),
    }];
    let id = record.request.execution_id.clone();
    let event = InvocationSchedulingEvent {
        kind: InvocationKind::Queued,
        target: None,
        before: Some(InvocationStage::Queued),
        stage: InvocationStage::Cancelled,
        cause: SchedulingCause::SessionClosed,
        actor: Some(ActionContext::new("first-person", "panel", "original-cancel").unwrap()),
    };
    let prep = SessionChange::CancellationPrepared {
        execution_id: id.clone(),
        decision: CancellationDecision::new(event.clone(), false),
    };
    let ack = SessionChange::CancellationAcknowledged {
        execution_id: id.clone(),
    };
    let mut state = Continuation::restore(Some(original.clone())).unwrap();
    assert!(state.apply_unit(std::slice::from_ref(&ack)).is_err());
    assert_eq!(state.snapshot.as_ref(), Some(&original));
    for change in [
        prep.clone(),
        ack.clone(),
        SessionChange::SchedulingTransition {
            execution_id: id,
            event: event.clone(),
        },
    ] {
        let encoded = super::super::semantic::encode_batch(std::slice::from_ref(&change)).unwrap();
        let decoded =
            super::super::semantic::decode_batch(&encoded, &original.provider_context).unwrap();
        assert_eq!(decoded, vec![change]);
        state.apply_unit(&decoded).unwrap();
        let snapshot = state.snapshot.as_ref().unwrap();
        let bytes = serde_json::to_vec(&SnapshotRef(snapshot)).unwrap();
        let restored: Snapshot = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(&restored.decode().unwrap(), snapshot);
    }
    let before = state.snapshot.clone();
    let bytes = state.snapshot_bytes;
    assert!(state.apply_unit(&[ack, prep]).is_err());
    assert_eq!(state.snapshot, before);
    assert_eq!(state.snapshot_bytes, bytes);
    let receipt = state
        .snapshot
        .unwrap()
        .invocations
        .remove(0)
        .cancellation_delivery
        .unwrap();
    assert_eq!(receipt.event(), &event);
    assert!(receipt.acknowledged());
}

#[tokio::test]
async fn cancellation_receipt_rejected_and_lost_history_acks_rejoin_the_same_writer_before_restart()
{
    use crate::application::agent_execution::providers::{
        AgentProvider, HarnessHostIdentity, ProviderIdentity, ProviderOpenFuture,
        ProviderOpenRequest,
    };
    use crate::application::agent_execution::sessions::{
        SessionLoad, SessionManager, SessionSaveGeneration, SessionSaveReceipt, SessionSaveUnit,
        SessionStorage, SessionStorageLease, StorageError, StorageFuture,
    };
    use crate::domain::effective_capabilities::value_objects::EffectiveCapabilities;
    use crate::infrastructure::session_storage::{InMemoryStorage, RuntimeMessageCommitClock};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    struct HistoryOnlyProvider(ProviderIdentity);
    impl AgentProvider for HistoryOnlyProvider {
        fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
            HarnessHostIdentity::local()
        }
        fn identity(&self) -> ProviderIdentity {
            self.0.clone()
        }
        fn capabilities(&self) -> &EffectiveCapabilities {
            unreachable!("history preparation never asks for execution capabilities")
        }
        fn open(&self, _: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
            unreachable!("history preparation never opens a provider")
        }
    }
    struct FaultStorage {
        inner: Arc<InMemoryStorage>,
        mode: Arc<AtomicUsize>,
    }
    struct FaultLease {
        inner: Box<dyn SessionStorageLease>,
        mode: Arc<AtomicUsize>,
    }
    impl SessionStorage for FaultStorage {
        fn open(
            &self,
            id: crate::domain::agent_execution::sessions::SessionId,
        ) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
            Box::pin(async move {
                Ok(Box::new(FaultLease {
                    inner: self.inner.open(id).await?,
                    mode: self.mode.clone(),
                }) as Box<dyn SessionStorageLease>)
            })
        }
    }
    impl SessionStorageLease for FaultLease {
        fn load(&self) -> StorageFuture<'_, SessionLoad> {
            self.inner.load()
        }
        fn erase(&self) -> StorageFuture<'_, ()> {
            self.inner.erase()
        }
        fn save_changes(
            &self,
            binding: SessionSaveGeneration,
            units: Vec<SessionSaveUnit>,
        ) -> StorageFuture<'_, SessionSaveReceipt> {
            Box::pin(async move {
                let mode = self.mode.load(Ordering::SeqCst);
                let targeted = units
                    .iter()
                    .flat_map(SessionSaveUnit::changes)
                    .any(|change| match change {
                        SessionChange::CancellationPrepared { .. } => mode == 1 || mode == 4,
                        SessionChange::CancellationAcknowledged { .. } => mode == 2 || mode == 3,
                        _ => false,
                    });
                if targeted {
                    if mode == 3 || mode == 4 {
                        self.inner.save_changes(binding, units).await?;
                    }
                    return Err(StorageError::Io(
                        "controlled cancellation receipt ACK failure".into(),
                    ));
                }
                self.inner.save_changes(binding, units).await
            })
        }
    }
    for mode in [1, 4, 2, 3] {
        let mut original = history_fixture(1);
        let record = &mut original.invocations[0];
        record.submission = crate::application::agent_execution::executions::SubmissionMode::Queued;
        record.scheduling = vec![InvocationSchedulingEvent {
            kind: InvocationKind::Queued,
            target: None,
            before: None,
            stage: InvocationStage::Queued,
            cause: SchedulingCause::Submitted,
            actor: Some(record.actor.clone()),
        }];
        let id = original.id.clone();
        let provider = HistoryOnlyProvider(original.provider.clone());
        let inner = Arc::new(InMemoryStorage::new());
        let lease = inner.open(id.clone()).await.unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                vec![SessionSaveUnit::new(vec![
                    SessionChange::Opened {
                        id: id.clone(),
                        provider: original.provider.clone(),
                        context: original.provider_context.clone(),
                    },
                    SessionChange::InputAccepted(Box::new(original.invocations.remove(0))),
                ])
                .unwrap()],
            )
            .await
            .unwrap();
        drop(lease);
        let switch = Arc::new(AtomicUsize::new(0));
        let fault = Arc::new(FaultStorage {
            inner: inner.clone(),
            mode: switch.clone(),
        });
        let mut manager = SessionManager::open(
            Some(id.clone()),
            fault,
            Arc::new(RuntimeMessageCommitClock::new()),
        )
        .await
        .unwrap();
        manager.prepare_for_lease_record(&provider).await.unwrap();
        let event = InvocationSchedulingEvent {
            kind: InvocationKind::Queued,
            target: None,
            before: Some(InvocationStage::Queued),
            stage: InvocationStage::Cancelled,
            cause: SchedulingCause::SessionClosed,
            actor: Some(ActionContext::new("first-person", "panel", "original-cancel").unwrap()),
        };
        if mode == 1 || mode == 4 {
            switch.store(mode, Ordering::SeqCst);
            assert!(manager
                .prepare_cancellation(0, event.clone())
                .await
                .is_err());
            let committed = inner.read_committed(id.clone()).await.unwrap().unwrap();
            assert_eq!(
                committed.snapshot().unwrap().invocations[0]
                    .cancellation_delivery
                    .is_some(),
                mode == 4
            );
            switch.store(0, Ordering::SeqCst);
            manager.flush_cancellation_receipts().await.unwrap();
            let retained = manager
                .prepare_cancellation(
                    0,
                    InvocationSchedulingEvent {
                        actor: Some(ActionContext::new("other", "panel", "retry").unwrap()),
                        ..event.clone()
                    },
                )
                .await
                .unwrap();
            assert_eq!(retained.event(), &event);
            assert!(!retained.acknowledged());
        } else {
            manager
                .prepare_cancellation(0, event.clone())
                .await
                .unwrap();
            switch.store(mode, Ordering::SeqCst);
            assert!(manager.acknowledge_cancellation(0, true).await.is_err());
            let committed = inner.read_committed(id.clone()).await.unwrap().unwrap();
            assert_eq!(
                committed.snapshot().unwrap().invocations[0]
                    .cancellation_delivery
                    .as_ref()
                    .unwrap()
                    .acknowledged(),
                mode == 3
            );
            switch.store(0, Ordering::SeqCst);
            manager.flush_cancellation_receipts().await.unwrap();
        }
        drop(manager);
        let mut restored =
            SessionManager::open(Some(id), inner, Arc::new(RuntimeMessageCommitClock::new()))
                .await
                .unwrap();
        restored.prepare_for_lease_record(&provider).await.unwrap();
        let snapshot = restored.snapshot().await.unwrap();
        let receipt = snapshot.invocations[0]
            .cancellation_delivery
            .as_ref()
            .unwrap();
        assert_eq!(receipt.event(), &event);
        assert_eq!(receipt.acknowledged(), mode == 2 || mode == 3);
    }
}
