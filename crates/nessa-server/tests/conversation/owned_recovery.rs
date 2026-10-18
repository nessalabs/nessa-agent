//! Recovery joins the original Closing operation after cleanup ownership binds.

use super::*;
use crate::conversation::application::environment::{LeaseHold, LeaseRelease};
use crate::conversation::infrastructure::DurableOwnershipAudit;
use nessa_sdk::application::agent_execution::subagents::{
    CloseCommand, LiveRoom, OwnershipFailure, OwnershipStore,
};
use nessa_sdk::domain::agent_execution::leases::{LeaseEndCause, LeasePhase};
use nessa_sdk::domain::agent_execution::subagents::{
    HostActor, Initiator, LifetimeCause, LifetimeState,
};
use std::collections::HashSet;
use std::sync::atomic::AtomicUsize;
use std::sync::Mutex;
#[tokio::test]
async fn startup_retries_a_completed_closing_generation_after_rebinding_its_owner() {
    closing_recovery_after_rebind(false).await;
}

#[tokio::test]
async fn startup_keeps_uncertain_cleanup_owned_and_retries_the_same_close_after_confirmation() {
    closing_recovery_after_rebind(true).await;
}

/// Physical provider cleanup happens normally, but this environment withholds
/// its acknowledgement. Later account answers refer only to leases actually
/// ended by this hold; no graph rows or lease records are fabricated.
struct WithheldCleanup {
    inner: Arc<dyn Environment>,
    ended: Arc<Mutex<HashSet<LeaseId>>>,
    answer: AtomicUsize,
    accounts: AtomicUsize,
}
struct WithheldHold {
    inner: Arc<dyn LeaseHold>,
    lease: LeaseId,
    ended: Arc<Mutex<HashSet<LeaseId>>>,
}
impl LeaseHold for WithheldHold {
    fn ends(&self) -> Option<EnvironmentFuture<'static, LeaseEndCause>> {
        self.inner.ends()
    }
    fn end(&self, cause: LeaseEndCause) -> EnvironmentFuture<'_, LeaseRelease> {
        Box::pin(async move {
            let result = self.inner.end(cause).await;
            if matches!(
                result,
                LeaseRelease::ByAgentClose | LeaseRelease::Released(LeaseCleanup::Confirmed { .. })
            ) {
                self.ended.lock().unwrap().insert(self.lease.clone());
            }
            LeaseRelease::Unanswered
        })
    }
}
impl Environment for WithheldCleanup {
    fn declaration(&self) -> EnvironmentDeclaration {
        self.inner.declaration()
    }
    fn open<'a>(
        &'a self,
        lease: &'a LeaseId,
        grant: &'a LeaseTerms,
        binding: Arc<dyn AgentProvider>,
    ) -> EnvironmentFuture<'a, Result<EnvironmentLease, LeaseRefusal>> {
        Box::pin(async move {
            let mut opened = self.inner.open(lease, grant, binding).await?;
            opened.hold = Arc::new(WithheldHold {
                inner: opened.hold,
                lease: lease.clone(),
                ended: self.ended.clone(),
            });
            Ok(opened)
        })
    }
    fn account<'a>(&'a self, lease: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>> {
        Box::pin(async move {
            self.accounts.fetch_add(1, Ordering::SeqCst);
            match self.answer.load(Ordering::SeqCst) {
                0 => None,
                1 => Some(LeaseCleanup::NotHeld),
                _ if self.ended.lock().unwrap().contains(lease) => {
                    Some(LeaseCleanup::Confirmed { forced: false })
                }
                _ => None,
            }
        })
    }
}

async fn closing_recovery_after_rebind(withheld: bool) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let environment = Arc::new(WithheldCleanup {
        inner: crate::conversation::infrastructure::in_process_environment(),
        ended: Arc::default(),
        answer: AtomicUsize::new(0),
        accounts: AtomicUsize::new(0),
    });
    let environment_port: Arc<dyn Environment> = if withheld {
        environment.clone()
    } else {
        environment.inner.clone()
    };
    let (service, storage, metadata) =
        fixture_in(&root, provider.clone(), environment_port.clone().into());
    let coordinator = bind_with_capacity(&service, &storage, &root, true, 1).await;
    let parent = id();
    opened(&service, &provider, &parent).await;
    let p = hold_turn(&service, &provider, &parent, "parent").await;
    let child_id = child(&service, &parent, "parent", "child").await;
    let lifetime = coordinator
        .active_lifetime_for_session(&conversation_session(&child_id))
        .unwrap();
    drop(p);
    retire(service, storage, metadata).await;
    let command = || CloseCommand {
        lifetime: lifetime.clone(),
        cause: LifetimeCause::HostClose,
        initiator: Initiator::Host(HostActor::new("person", "panel", "host-close").unwrap()),
        external_attachment: false,
        timeout: Some(TURN_HOLD_HANG),
    };
    // The real cleanup owner has gone; the supported SDK close persists its
    // original operation but cannot claim physical cleanup through a dead Weak.
    assert!(matches!(
        coordinator.end_lifetime(command()).await,
        Err(OwnershipFailure::Incomplete)
    ));
    let owned = SqliteOwnershipStore::open(&root.join("ownership.sqlite3")).unwrap();
    let closing = owned.read().await.unwrap();
    let original = closing
        .lifetimes
        .iter()
        .find(|row| row.lifetime_id == lifetime)
        .unwrap()
        .clone();
    assert_eq!(original.state, LifetimeState::Closing);
    assert_eq!(original.cause, Some(LifetimeCause::HostClose));
    drop(coordinator);
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture_in(&root, provider.clone(), environment_port.into());
    if withheld {
        let (committed, _) = service.committed_with_order(&child_id).await.unwrap();
        assert!(
            committed
                .snapshot()
                .unwrap()
                .lease
                .as_ref()
                .unwrap()
                .held()
                .is_some_and(|lease| matches!(
                    lease.phase(),
                    LeasePhase::Interrupted {
                        late_cleanup: None,
                        ..
                    }
                )),
            "the actual lease history retains lost cleanup acknowledgement"
        );
    }
    service.bind_commands(storage.clone());
    let room = Arc::new(LiveCapacity::new(1));
    let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: Arc::new(SqliteOwnershipStore::open(&root.join("ownership.sqlite3")).unwrap()),
        audit: Arc::new(
            DurableOwnershipAudit::new(root.join("ownership-audit"), Arc::new(TestClock)).unwrap(),
        ),
        factory: service.owned_thread_factory(),
        room: room.clone(),
    });
    coordinator.resume().await.unwrap();
    // A public close retry joins/completes the empty-adapter generation. This
    // barrier makes the recovery ordering deterministic without a timing sleep.
    assert!(matches!(
        coordinator.end_lifetime(command()).await,
        Err(OwnershipFailure::Incomplete)
    ));
    service
        .bind_owned_threads(coordinator.clone(), true)
        .unwrap();
    let snapshot = owned.read().await.unwrap();
    assert!(metadata
        .load(&child_id)
        .await
        .unwrap()
        .unwrap()
        .deletion()
        .is_none());
    service.recover_owned_threads(&snapshot).await.unwrap();
    if withheld {
        for answer in [0, 1] {
            environment.answer.store(answer, Ordering::SeqCst);
            service
                .recover_owned_threads(&owned.read().await.unwrap())
                .await
                .unwrap();
            assert_eq!(
                coordinator.lifetime_state(&lifetime),
                Some(LifetimeState::Closing)
            );
            let pending = owned.read().await.unwrap();
            let row = pending
                .lifetimes
                .iter()
                .find(|row| row.lifetime_id == lifetime)
                .unwrap();
            assert_eq!(row.close_operation, original.close_operation);
            assert_eq!(row.cause, original.cause);
            assert_eq!(row.initiator, original.initiator);
            assert!(
                !LiveRoom::try_reserve(room.as_ref()),
                "uncertain cleanup retains its capacity"
            );
        }
        environment.answer.store(2, Ordering::SeqCst);
        service
            .recover_owned_threads(&owned.read().await.unwrap())
            .await
            .unwrap();
    }
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closed)
    );
    let closed = owned.read().await.unwrap();
    let row = closed
        .lifetimes
        .iter()
        .find(|row| row.lifetime_id == lifetime)
        .unwrap();
    assert_eq!(row.close_operation, original.close_operation);
    assert_eq!(row.cause, original.cause);
    assert_eq!(row.initiator, original.initiator);
    assert!(
        LiveRoom::try_reserve(room.as_ref()),
        "confirmed cleanup releases the single child slot"
    );
    LiveRoom::release(room.as_ref());
    let accounts = environment.accounts.load(Ordering::SeqCst);
    let executions = provider.executions.lock().unwrap().len();
    service.recover_owned_threads(&closed).await.unwrap();
    assert_eq!(provider.executions.lock().unwrap().len(), executions);
    assert_eq!(
        environment.accounts.load(Ordering::SeqCst),
        accounts,
        "settled ownership does not replay physical cleanup"
    );
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    retire(service, storage, metadata).await;
}
