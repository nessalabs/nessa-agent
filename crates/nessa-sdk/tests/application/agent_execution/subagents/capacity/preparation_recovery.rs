//! Verified pre-initialization absence preserves the existing quota and close owners.
use super::*;

async fn unstarted_preparation(
    room: Arc<CountedRoom>,
    prepared: bool,
) -> (World, AgentLifetimeId, SpawnRequestId, AgentLifetimeId) {
    let mut graph = OwnershipGraph::new();
    let root = AgentLifetimeId::new("root-life").unwrap();
    let request = SpawnRequestId::new("unstarted").unwrap();
    let child = AgentLifetimeId::new("unstarted-child").unwrap();
    let _opened = graph
        .open_root(
            SessionId::new("root-session").unwrap(),
            root.clone(),
            Initiator::Runtime,
        )
        .unwrap();
    let _reserved = graph
        .admit_spawn(SpawnAdmission {
            child_lifetime: child.clone(),
            child_session: SessionId::new("unstarted-session").unwrap(),
            binding: binding(&root, &request, "unstarted"),
            live_room: true,
        })
        .unwrap();
    if prepared {
        let _prepared = graph
            .advance_spawn(&request, SpawnProgress::Prepared)
            .unwrap();
    }
    let store = Arc::new(MemoryOwnershipStore::new());
    store.write(&graph.snapshot()).await.unwrap();
    let world = restore_with(store, room);
    world.coordinator.resume().await.unwrap();
    (world, root, request, child)
}

#[tokio::test]
async fn verified_unstarted_preparation_releases_once_and_survives_retry_and_restart() {
    let room = CountedRoom::new(1);
    let (world, root, request, child) = unstarted_preparation(room.clone(), false).await;
    assert_eq!(room.held.load(Ordering::SeqCst), 1);
    world
        .coordinator
        .reject_unstarted_preparation(&request, &child)
        .await
        .unwrap();
    assert_eq!(
        world.coordinator.lifetime_state(&child),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Open)
    );
    assert_eq!(room.held.load(Ordering::SeqCst), 0);
    assert_eq!(room.releases.load(Ordering::SeqCst), 1);
    let settled = world.store.read().await.unwrap();
    assert!(settled.settlements.iter().any(|row| row.target == child
        && matches!(
            row.proof,
            nessa_sdk::domain::agent_execution::subagents::SettlementProof::Absence(_)
        )));
    world
        .coordinator
        .reject_unstarted_preparation(&request, &child)
        .await
        .unwrap();
    assert_eq!(room.releases.load(Ordering::SeqCst), 1);
    let restored = restore_with(world.store.clone(), room.clone());
    restored.coordinator.resume().await.unwrap();
    restored
        .coordinator
        .reject_unstarted_preparation(&request, &child)
        .await
        .unwrap();
    assert_eq!(room.releases.load(Ordering::SeqCst), 1);
    assert_eq!(restored.factory.prepares(), 0);
    assert_eq!(restored.factory.submits(), 0);
    assert!(room.try_reserve());
    room.release();
}

#[tokio::test]
async fn unstarted_recovery_refuses_foreign_identity_later_progress_and_bound_resources() {
    for prepared in [false, true] {
        let room = CountedRoom::new(1);
        let (world, root, request, child) = unstarted_preparation(room.clone(), prepared).await;
        let before = world.store.read().await.unwrap();
        let mut cases = vec![
            (SpawnRequestId::new("other-request").unwrap(), child.clone()),
            (
                request.clone(),
                AgentLifetimeId::new("other-child").unwrap(),
            ),
            (request.clone(), root.clone()),
        ];
        if prepared {
            cases.push((request.clone(), child.clone()));
        }
        for (attempt_request, attempt_child) in cases {
            assert!(matches!(
                world
                    .coordinator
                    .reject_unstarted_preparation(&attempt_request, &attempt_child)
                    .await,
                Err(OwnershipFailure::Domain(OwnershipError::StaleOutcome))
            ));
            assert_eq!(
                world.coordinator.lifetime_state(&root),
                Some(LifetimeState::Open)
            );
            assert_eq!(
                world.coordinator.lifetime_state(&child),
                Some(LifetimeState::Open)
            );
        }
        assert_eq!(world.store.read().await.unwrap(), before);
        assert_eq!(room.held.load(Ordering::SeqCst), 1);
        assert_eq!(room.releases.load(Ordering::SeqCst), 0);
        assert_eq!(world.factory.prepares(), 0);
        if !prepared {
            let owner = Arc::new(ScriptResources {
                closes: AtomicUsize::new(0),
                report: ResourceReport {
                    physical: PhysicalFact::Released,
                    evidence: EvidenceFact::Acknowledged,
                },
                hold: None,
            });
            world
                .coordinator
                .bind_resources(child.clone(), owner.clone())
                .unwrap();
            assert!(world
                .coordinator
                .reject_unstarted_preparation(&request, &child)
                .await
                .is_err());
            assert_eq!(world.store.read().await.unwrap(), before);
            assert_eq!(owner.closes.load(Ordering::SeqCst), 0);
            world.close(&child).await.unwrap();
        }
    }
}

#[tokio::test]
async fn unstarted_recovery_refuses_an_actual_live_factory_flight() {
    let world = World::new(1);
    let root = world.root().await;
    let release = Arc::new(Notify::new());
    *world.factory.release.lock().unwrap() = Some(release.clone());
    let command = world.command(&root, "in-flight", "live preparation");
    let request = command.request_id.clone();
    let coordinator = world.coordinator.clone();
    let caller = tokio::spawn(async move {
        coordinator
            .prepare_child(command, SessionId::new("live-child").unwrap())
            .await
    });
    world.factory.entered.notified().await;
    let child = world.factory.last_child.lock().unwrap().clone().unwrap();
    let before = world.store.read().await.unwrap();
    let refusal = world
        .coordinator
        .reject_unstarted_preparation(&request, &child)
        .await;
    release.notify_one();
    caller.await.unwrap().unwrap();
    assert!(matches!(
        refusal,
        Err(OwnershipFailure::Domain(OwnershipError::StaleOutcome))
    ));
    assert!(before.settlements.is_empty());
    assert!(world.store.read().await.unwrap().settlements.is_empty());
    assert_eq!(world.factory.prepares(), 1);
    world.close(&child).await.unwrap();
}

#[tokio::test]
async fn unstarted_recovery_retries_original_absence_after_audit_or_store_failure() {
    for audit in [false, true] {
        for failure in [PortFailure::Rejected, PortFailure::Uncertain] {
            let room = CountedRoom::new(1);
            let (world, _, request, child) = unstarted_preparation(room.clone(), false).await;
            if audit {
                world.audit.fail_next(failure);
            } else {
                world.store.fail_next_write(failure);
            }
            let result = world
                .coordinator
                .reject_unstarted_preparation(&request, &child)
                .await;
            assert_eq!(
                result,
                Err(if audit {
                    OwnershipFailure::Audit(failure)
                } else {
                    OwnershipFailure::Store(failure)
                })
            );
            assert_eq!(
                world.coordinator.lifetime_state(&child),
                Some(LifetimeState::Closing)
            );
            assert_eq!(room.held.load(Ordering::SeqCst), 0);
            assert_eq!(room.releases.load(Ordering::SeqCst), 1);
            let saved = world.store.read().await.unwrap();
            let operation = saved
                .lifetimes
                .iter()
                .find(|row| row.lifetime_id == child)
                .and_then(|row| row.close_operation.clone())
                .or_else(|| {
                    world
                        .audit
                        .records
                        .lock()
                        .unwrap()
                        .iter()
                        .rev()
                        .find(|record| {
                            (record.child_lifetime.as_ref() == Some(&child)
                                || record.parent_lifetime == child)
                                && record.close_operation.is_some()
                        })
                        .and_then(|record| record.close_operation.clone())
                })
                .expect("the original close owner is retained by store or attempted audit");
            world
                .coordinator
                .reject_unstarted_preparation(&request, &child)
                .await
                .unwrap();
            let saved = world.store.read().await.unwrap();
            let row = saved
                .lifetimes
                .iter()
                .find(|row| row.lifetime_id == child)
                .unwrap();
            assert_eq!(row.state, LifetimeState::Closed);
            assert_eq!(row.close_operation.as_ref(), Some(&operation));
            assert_eq!(row.cause, Some(LifetimeCause::TerminalFailure));
            assert_eq!(row.initiator, Some(Initiator::Runtime));
            assert_eq!(room.releases.load(Ordering::SeqCst), 1);
            assert_eq!(world.factory.prepares(), 0);
            assert_eq!(world.factory.submits(), 0);
        }
    }
}

struct LostAbsenceAcknowledgement {
    inner: Arc<MemoryOwnershipStore>,
    lose: AtomicBool,
}
#[async_trait]
impl OwnershipStore for LostAbsenceAcknowledgement {
    async fn read(&self) -> Result<OwnershipSnapshot, PortFailure> {
        self.inner.read().await
    }
    async fn write(&self, snapshot: &OwnershipSnapshot) -> Result<(), PortFailure> {
        self.inner.write(snapshot).await?;
        if !snapshot.settlements.is_empty() && self.lose.swap(false, Ordering::SeqCst) {
            Err(PortFailure::Uncertain)
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn unstarted_absence_with_lost_committed_ack_keeps_first_close_across_restart() {
    let room = CountedRoom::new(1);
    let (original, _, request, child) = unstarted_preparation(room.clone(), false).await;
    let store = original.store.clone();
    // Start a new host over the real reserved prefix, with an adapter that
    // commits the first absence snapshot but loses its acknowledgement.
    drop(original);
    let room = CountedRoom::new(1);
    let factory = ScriptFactory::new();
    let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: Arc::new(LostAbsenceAcknowledgement {
            inner: store.clone(),
            lose: AtomicBool::new(true),
        }),
        audit: ScriptAudit::new(),
        factory: factory.clone(),
        room: room.clone(),
    });
    coordinator.resume().await.unwrap();
    assert_eq!(
        coordinator
            .reject_unstarted_preparation(&request, &child)
            .await,
        Err(OwnershipFailure::Store(PortFailure::Uncertain))
    );
    let saved = store.read().await.unwrap();
    let first = saved
        .lifetimes
        .iter()
        .find(|row| row.lifetime_id == child)
        .unwrap()
        .clone();
    assert_eq!(first.state, LifetimeState::Closing);
    assert_eq!(room.releases.load(Ordering::SeqCst), 1);
    assert_eq!(factory.prepares(), 0);
    drop(coordinator);
    let next_room = CountedRoom::new(1);
    let restored = restore_with(store.clone(), next_room.clone());
    restored.coordinator.resume().await.unwrap();
    restored
        .coordinator
        .reject_unstarted_preparation(&request, &child)
        .await
        .unwrap();
    let saved = store.read().await.unwrap();
    let last = saved
        .lifetimes
        .iter()
        .find(|row| row.lifetime_id == child)
        .unwrap();
    assert_eq!(last.state, LifetimeState::Closed);
    assert_eq!(last.close_operation, first.close_operation);
    assert_eq!(last.cause, first.cause);
    assert_eq!(last.initiator, first.initiator);
    assert_eq!(
        next_room.calls.load(Ordering::SeqCst),
        0,
        "actual absence reserves no room again"
    );
    assert_eq!(
        next_room.releases.load(Ordering::SeqCst),
        0,
        "an unreserved slot cannot be released"
    );
    assert_eq!(restored.factory.prepares(), 0);
}

#[tokio::test]
async fn unstarted_recovery_survives_caller_loss_and_keeps_the_failed_original_waiter() {
    let room = CountedRoom::new(1);
    let (world, _, request, child) = unstarted_preparation(room.clone(), false).await;
    let rejection = Arc::new(RejectedCloseObservation {
        child: child.clone(),
        attempts: Mutex::new(Vec::new()),
        entered: Notify::new(),
        release: Notify::new(),
    });
    *world.audit.reject_close_observation.lock().unwrap() = Some(rejection.clone());
    let caller = tokio::spawn({
        let coordinator = world.coordinator.clone();
        let request = request.clone();
        let child = child.clone();
        async move {
            coordinator
                .reject_unstarted_preparation(&request, &child)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), rejection.entered.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let joined = world.coordinator.end_lifetime(CloseCommand {
        lifetime: child.clone(),
        cause: LifetimeCause::HostClose,
        initiator: Initiator::Host(actor("later-waiter")),
        external_attachment: false,
        timeout: Some(Duration::from_secs(5)),
    });
    tokio::pin!(joined);
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(joined.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    let original = rejection.attempts.lock().unwrap()[0].clone();
    rejection.release.notify_one();
    assert_eq!(
        joined.await,
        Err(OwnershipFailure::Audit(PortFailure::Rejected))
    );
    world
        .coordinator
        .reject_unstarted_preparation(&request, &child)
        .await
        .unwrap();
    let saved = world.store.read().await.unwrap();
    let row = saved
        .lifetimes
        .iter()
        .find(|row| row.lifetime_id == child)
        .unwrap();
    assert_eq!(row.state, LifetimeState::Closed);
    assert_eq!(row.close_operation, original.close_operation);
    assert_eq!(row.cause, original.cause);
    assert_eq!(row.initiator.as_ref(), Some(&original.initiator));
    assert_eq!(room.releases.load(Ordering::SeqCst), 1);
    assert_eq!(world.factory.prepares(), 0);
}

#[tokio::test]
async fn refused_restored_absence_cannot_release_a_previously_retained_reservation() {
    let room = CountedRoom::new(1);
    let (world, _, request, child) = unstarted_preparation(room.clone(), false).await;
    let (proof_world, _, proof_request, proof_child) =
        unstarted_preparation(CountedRoom::new(1), false).await;
    proof_world
        .coordinator
        .reject_unstarted_preparation(&proof_request, &proof_child)
        .await
        .unwrap();
    let mut contradictory = proof_world.store.read().await.unwrap();
    // A real completed absence is not authority after its retained ancestry
    // becomes contradictory. The already-held quota must remain untouched.
    contradictory.spawns[0].binding.parent_lifetime = child.clone();
    contradictory.spawns[0].binding.parent_session = contradictory.spawns[0].child_session.clone();
    world.store.write(&contradictory).await.unwrap();
    world.coordinator.resume().await.unwrap();
    let refusal = world
        .coordinator
        .reject_unstarted_preparation(&request, &child)
        .await;
    assert!(matches!(
        refusal,
        Err(OwnershipFailure::Domain(OwnershipError::StaleOutcome))
    ));
    assert_eq!(world.store.read().await.unwrap(), contradictory);
    assert_eq!(room.held.load(Ordering::SeqCst), 1);
    assert_eq!(room.releases.load(Ordering::SeqCst), 0);
    assert_eq!(world.factory.prepares(), 0);
}

struct HoldUnconfirmedPreparation {
    inner: Arc<ScriptAudit>,
    rejected: AtomicBool,
    entered: Notify,
    release: Notify,
}
#[async_trait]
impl OwnershipAudit for HoldUnconfirmedPreparation {
    async fn record(&self, evidence: &OwnershipEvidence) -> Result<(), PortFailure> {
        if evidence.after == OwnershipMeaning::Reserved
            && !self.rejected.swap(true, Ordering::SeqCst)
        {
            return Err(PortFailure::Uncertain);
        }
        if evidence.after == OwnershipMeaning::Unconfirmed {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.inner.record(evidence).await
    }
}

#[tokio::test]
async fn unstarted_recovery_refuses_a_live_unconfirmed_preparation_transaction() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let room = CountedRoom::new(1);
    let factory = ScriptFactory::new();
    let audit = Arc::new(HoldUnconfirmedPreparation {
        inner: ScriptAudit::new(),
        rejected: AtomicBool::new(false),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: store.clone(),
        room: room.clone(),
        factory: factory.clone(),
        audit: audit.clone(),
    });
    let root = coordinator
        .open_root(SessionId::new("root-session").unwrap(), Initiator::Runtime)
        .await
        .unwrap();
    let request = SpawnRequestId::new("live-unconfirmed").unwrap();
    let command = SpawnCommand {
        parent: root.clone(),
        request_id: request.clone(),
        task: "live pending admission".into(),
        policy: PolicyRead::Committed(ApprovalPolicy::new("read-only", "ask", "rev-1").unwrap()),
        child_supports_policy: true,
        model: None,
        origin: SpawnOrigin::Host(actor("live-unconfirmed")),
    };
    let caller = tokio::spawn({
        let coordinator = coordinator.clone();
        async move {
            coordinator
                .prepare_child(command, SessionId::new("live-unconfirmed-child").unwrap())
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), audit.entered.notified())
        .await
        .unwrap();
    let child = coordinator.children(&root, None, 1).unwrap().children[0]
        .lifetime
        .clone();
    assert!(matches!(
        coordinator.spawn_progress(&request),
        Some(SpawnProgress::Unconfirmed {
            known: KnownMilestone::Reserved
        })
    ));
    let before = store.read().await.unwrap();
    let refusal = tokio::time::timeout(
        Duration::from_secs(5),
        coordinator.reject_unstarted_preparation(&request, &child),
    )
    .await;
    let after = store.read().await.unwrap();
    let held = room.held.load(Ordering::SeqCst);
    let released = room.releases.load(Ordering::SeqCst);
    audit.release.notify_one();
    assert!(caller.await.unwrap().is_err());
    assert!(matches!(
        refusal,
        Ok(Err(OwnershipFailure::Domain(OwnershipError::StaleOutcome)))
    ));
    assert_eq!(after, before);
    assert_eq!(held, 1);
    assert_eq!(released, 0);
    assert_eq!(factory.prepares(), 0);
}

#[tokio::test]
async fn unstarted_recovery_does_not_join_or_replace_an_ancestors_held_close() {
    let room = CountedRoom::new(1);
    let (world, root, request, child) = unstarted_preparation(room.clone(), false).await;
    let release = Arc::new(Notify::new());
    let owner = Arc::new(ScriptResources {
        closes: AtomicUsize::new(0),
        hold: Some(release.clone()),
        report: ResourceReport {
            physical: PhysicalFact::Released,
            evidence: EvidenceFact::Acknowledged,
        },
    });
    world
        .coordinator
        .bind_resources(root.clone(), owner.clone())
        .unwrap();
    let closing = tokio::spawn({
        let coordinator = world.coordinator.clone();
        let root = root.clone();
        async move {
            coordinator
                .end_lifetime(CloseCommand {
                    lifetime: root,
                    cause: LifetimeCause::HostClose,
                    initiator: Initiator::Host(actor("original-parent-close")),
                    external_attachment: false,
                    timeout: Some(Duration::from_secs(5)),
                })
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while owner.closes.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let first = world
        .store
        .read()
        .await
        .unwrap()
        .lifetimes
        .into_iter()
        .find(|row| row.lifetime_id == root)
        .unwrap();
    let recovered = tokio::time::timeout(
        Duration::from_secs(2),
        world
            .coordinator
            .reject_unstarted_preparation(&request, &child),
    )
    .await;
    let ancestor_held = !closing.is_finished();
    let held = room.held.load(Ordering::SeqCst);
    release.notify_one();
    let closed = closing.await.unwrap();
    // Release and join the real cleanup owner even if the recovery assertion
    // intentionally fails under a rule-removal probe.
    recovered.unwrap().unwrap();
    closed.unwrap();
    assert!(
        ancestor_held,
        "the caller did not join or replace the held ancestor"
    );
    assert_eq!(held, 0);
    assert_eq!(owner.closes.load(Ordering::SeqCst), 1);
    assert_eq!(room.releases.load(Ordering::SeqCst), 1);
    let saved = world.store.read().await.unwrap();
    let child_row = saved
        .lifetimes
        .iter()
        .find(|row| row.lifetime_id == child)
        .unwrap();
    assert_eq!(child_row.state, LifetimeState::Closed);
    assert_eq!(child_row.close_operation, first.close_operation);
    assert_eq!(child_row.cause, first.cause);
    assert_eq!(child_row.initiator, first.initiator);
    assert_eq!(world.factory.prepares(), 0);
}
