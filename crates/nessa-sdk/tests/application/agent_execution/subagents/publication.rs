//! Deterministic #628 ordering matrix: audit gates hold only the selected operation.
use std::{
    future::Future,
    task::{Context, Poll, Waker},
    thread,
};

use super::*;
use nessa_sdk::application::agent_execution::subagents::{BindResourcesRefusal, LiveRoom};
use nessa_sdk::domain::agent_execution::subagents::{KnownMilestone, OwnershipMeaning};
use tokio::{runtime::Handle, sync::oneshot};

struct AuditGate {
    entered: Notify,
    evidence: Mutex<Option<OwnershipEvidence>>,
    result: Mutex<Option<oneshot::Receiver<Result<(), PortFailure>>>>,
}
struct IndependentAudit {
    next: Mutex<Option<(OwnershipMeaning, Arc<AuditGate>)>>,
    records: Mutex<Vec<OwnershipEvidence>>,
    failures: Mutex<VecDeque<(OwnershipMeaning, PortFailure)>>,
}
impl IndependentAudit {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            next: Mutex::new(None),
            records: Mutex::new(Vec::new()),
            failures: Mutex::new(VecDeque::new()),
        })
    }
    fn gate(
        &self,
        meaning: OwnershipMeaning,
    ) -> (Arc<AuditGate>, oneshot::Sender<Result<(), PortFailure>>) {
        let (sender, receiver) = oneshot::channel();
        let gate = Arc::new(AuditGate {
            entered: Notify::new(),
            evidence: Mutex::new(None),
            result: Mutex::new(Some(receiver)),
        });
        *self.next.lock().unwrap() = Some((meaning, gate.clone()));
        (gate, sender)
    }
}
#[async_trait]
impl OwnershipAudit for IndependentAudit {
    async fn record(&self, evidence: &OwnershipEvidence) -> Result<(), PortFailure> {
        self.records.lock().unwrap().push(evidence.clone());
        let gate = {
            let mut next = self.next.lock().unwrap();
            if next
                .as_ref()
                .is_some_and(|(meaning, _)| meaning == &evidence.after)
            {
                next.take().map(|(_, gate)| gate)
            } else {
                None
            }
        };
        if let Some(gate) = gate {
            *gate.evidence.lock().unwrap() = Some(evidence.clone());
            let receiver = gate.result.lock().unwrap().take().unwrap();
            gate.entered.notify_one();
            return receiver.await.unwrap();
        }
        let mut failures = self.failures.lock().unwrap();
        if failures
            .front()
            .is_some_and(|(meaning, _)| meaning == &evidence.after)
        {
            return Err(failures.pop_front().unwrap().1);
        }
        Ok(())
    }
}
fn coordinator(
    store: Arc<dyn OwnershipStore>,
    audit: Arc<IndependentAudit>,
    factory: Arc<ScriptFactory>,
    room: Arc<LiveCapacity>,
) -> OwnershipCoordinator {
    OwnershipCoordinator::new(OwnershipDependencies {
        store,
        audit,
        factory,
        room,
    })
}
async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(3), future)
        .await
        .expect("deterministic operation completed")
}
async fn open(
    coordinator: &OwnershipCoordinator,
    session: &str,
) -> Result<AgentLifetimeId, OwnershipFailure> {
    coordinator
        .open_root(
            SessionId::new(session).unwrap(),
            Initiator::Host(actor(session)),
        )
        .await
}
fn command(parent: &AgentLifetimeId) -> SpawnCommand {
    World::new(8).command(parent, "pending-spawn", "task")
}
async fn close(
    coordinator: &OwnershipCoordinator,
    root: &AgentLifetimeId,
) -> Result<(), OwnershipFailure> {
    coordinator
        .end_lifetime(CloseCommand {
            lifetime: root.clone(),
            cause: LifetimeCause::HostClose,
            initiator: Initiator::Runtime,
            external_attachment: false,
            timeout: None,
        })
        .await
}

#[tokio::test]
async fn rows_1_2_private_root_excluded_while_neighbor_completes_and_rejection_resumes_cleanly() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let c = coordinator(
        store.clone(),
        audit.clone(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    let (gate, release) = audit.gate(OwnershipMeaning::Open);
    let a = tokio::spawn({
        let c = c.clone();
        async move { open(&c, "a").await }
    });
    bounded(gate.entered.notified()).await;
    assert!(c
        .active_root_for_session(&SessionId::new("a").unwrap())
        .is_none());
    let b = bounded(open(&c, "b")).await.unwrap();
    let snapshot = store.read().await.unwrap();
    assert_eq!(snapshot.lifetimes.len(), 1);
    assert_eq!(snapshot.lifetimes[0].lifetime_id, b);
    release.send(Err(PortFailure::Rejected)).unwrap();
    assert_eq!(
        bounded(a).await.unwrap(),
        Err(OwnershipFailure::Audit(PortFailure::Rejected))
    );
    let resumed = coordinator(
        store.clone(),
        audit,
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    resumed.resume().await.unwrap();
    bounded(open(&resumed, "a")).await.unwrap();
    assert_eq!(resumed.lifetime_state(&b), Some(LifetimeState::Open));
}

#[tokio::test]
async fn row_3_private_reservation_excludes_child_and_factory_until_audit() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let factory = ScriptFactory::new();
    let c = coordinator(
        store.clone(),
        audit.clone(),
        factory.clone(),
        Arc::new(LiveCapacity::new(8)),
    );
    let root = open(&c, "a").await.unwrap();
    let (gate, release) = audit.gate(OwnershipMeaning::Reserved);
    let a = tokio::spawn({
        let c = c.clone();
        async move { c.spawn(command(&root)).await }
    });
    bounded(gate.entered.notified()).await;
    bounded(open(&c, "b")).await.unwrap();
    let snapshot = store.read().await.unwrap();
    assert!(snapshot.spawns.is_empty());
    assert_eq!(snapshot.lifetimes.len(), 2);
    assert_eq!(factory.prepares(), 0);
    release.send(Err(PortFailure::Rejected)).unwrap();
    assert!(bounded(a).await.unwrap().is_err());
    bounded(open(&c, "another")).await.unwrap();
    assert!(store.read().await.unwrap().spawns.is_empty());
}

async fn pending_advance(held: OwnershipMeaning, expected: SpawnProgress) {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let factory = ScriptFactory::new();
    let c = coordinator(
        store.clone(),
        audit.clone(),
        factory.clone(),
        Arc::new(LiveCapacity::new(8)),
    );
    let root = open(&c, "a").await.unwrap();
    let (gate, release) = audit.gate(held);
    let a = tokio::spawn({
        let c = c.clone();
        async move { c.spawn(command(&root)).await }
    });
    bounded(gate.entered.notified()).await;
    bounded(open(&c, "b")).await.unwrap();
    let snapshot = store.read().await.unwrap();
    assert_eq!(snapshot.spawns[0].progress, expected);
    assert_eq!(
        snapshot.spawns[0].child_lifetime,
        factory.last_child.lock().unwrap().clone().unwrap()
    );
    let resumed = coordinator(
        store,
        IndependentAudit::new(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    resumed.resume().await.unwrap();
    assert_eq!(
        resumed.spawn_progress(&SpawnRequestId::new("pending-spawn").unwrap()),
        Some(SpawnProgress::Unconfirmed {
            known: expected.known()
        })
    );
    release.send(Ok(())).unwrap();
    bounded(a).await.unwrap().unwrap();
}
#[tokio::test]
async fn row_4_prepared_stays_durable_while_attached_audit_pending() {
    pending_advance(OwnershipMeaning::Attached, SpawnProgress::Prepared).await;
}
#[tokio::test]
async fn row_5_attached_stays_durable_while_task_audit_pending() {
    pending_advance(OwnershipMeaning::TaskAdmitted, SpawnProgress::Attached).await;
}

#[tokio::test]
async fn rows_6_7_pending_ack_cannot_reopen_rejected_close_intent() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let factory = ScriptFactory::new();
    let c = coordinator(
        store.clone(),
        audit.clone(),
        factory.clone(),
        Arc::new(LiveCapacity::new(8)),
    );
    let root = open(&c, "a").await.unwrap();
    let (gate, release) = audit.gate(OwnershipMeaning::Prepared);
    let a = tokio::spawn({
        let c = c.clone();
        let root = root.clone();
        async move { c.spawn(command(&root)).await }
    });
    bounded(gate.entered.notified()).await;
    audit
        .failures
        .lock()
        .unwrap()
        .push_back((OwnershipMeaning::Closing, PortFailure::Rejected));
    let closing = tokio::spawn({
        let c = c.clone();
        let root = root.clone();
        async move { close(&c, &root).await }
    });
    bounded(async {
        loop {
            if store
                .read()
                .await
                .unwrap()
                .lifetimes
                .iter()
                .any(|r| r.lifetime_id == root && r.state != LifetimeState::Open)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    release.send(Ok(())).unwrap();
    let _ = bounded(a).await.unwrap();
    assert!(bounded(closing).await.unwrap().is_err());
    assert_ne!(c.lifetime_state(&root), Some(LifetimeState::Open));
    assert_eq!(c.close_cause(&root), Some(LifetimeCause::HostClose));
    assert!(store
        .read()
        .await
        .unwrap()
        .lifetimes
        .iter()
        .all(|r| r.state != LifetimeState::Open));
}

#[tokio::test]
async fn row_8_released_capacity_and_fact_precede_rejected_cleanup_audit() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let room = Arc::new(LiveCapacity::new(1));
    let c = coordinator(
        store.clone(),
        audit.clone(),
        ScriptFactory::new(),
        room.clone(),
    );
    let root = open(&c, "a").await.unwrap();
    let child = c.spawn(command(&root)).await.unwrap().child;
    // Closing intent consumes the first Closing audit; hold the cleanup's Closed audit.
    let (gate, release) = audit.gate(OwnershipMeaning::Closed);
    let closing = tokio::spawn({
        let c = c.clone();
        async move { close(&c, &child).await }
    });
    bounded(gate.entered.notified()).await;
    assert!(room.try_reserve());
    room.release();
    let evidence = gate.evidence.lock().unwrap().clone().unwrap();
    // An unrelated commit carries the physical fact while audit is still held.
    bounded(open(&c, "b")).await.unwrap();
    assert!(store
        .read()
        .await
        .unwrap()
        .settlements
        .iter()
        .any(
            |r| Some(&r.close_lifetime) == evidence.child_lifetime.as_ref()
                && r.physical == PhysicalFact::Released
        ));
    release.send(Err(PortFailure::Rejected)).unwrap();
    let _ = bounded(closing).await.unwrap();
}

#[tokio::test]
async fn rows_18_19_never_bound_settlement_rejection_stays_closing_and_explicit_retry_uses_actual_audit(
) {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let c = coordinator(
        store.clone(),
        audit.clone(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    let root = open(&c, "a").await.unwrap();
    // Close intent is first, then the absence audit is also Closing.
    let (gate, release) = audit.gate(OwnershipMeaning::Closing);
    let closing = tokio::spawn({
        let c = c.clone();
        let root = root.clone();
        async move { close(&c, &root).await }
    });
    bounded(gate.entered.notified()).await;
    audit
        .failures
        .lock()
        .unwrap()
        .push_back((OwnershipMeaning::Closing, PortFailure::Rejected));
    release.send(Ok(())).unwrap();
    assert_eq!(
        bounded(closing).await.unwrap(),
        Err(OwnershipFailure::Audit(PortFailure::Rejected))
    );
    assert_eq!(
        c.active_root_for_session(&SessionId::new("a").unwrap()),
        Some(root.clone())
    );
    let snapshot = store.read().await.unwrap();
    assert_eq!(snapshot.lifetimes[0].state, LifetimeState::Closing);
    assert_eq!(snapshot.settlements[0].physical, PhysicalFact::Released);
    assert_eq!(snapshot.settlements[0].evidence, EvidenceFact::Failed);
    assert_eq!(
        open(&c, "a").await,
        Err(OwnershipFailure::Domain(OwnershipError::LifetimeStillOpen))
    );
    bounded(close(&c, &root)).await.unwrap();
    assert_eq!(c.lifetime_state(&root), Some(LifetimeState::Closed));
    assert!(c
        .active_root_for_session(&SessionId::new("a").unwrap())
        .is_none());
}

#[tokio::test]
async fn row_11_uncertain_root_identity_is_retained_nonrunnable() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    audit.failures.lock().unwrap().extend([
        (OwnershipMeaning::Open, PortFailure::Uncertain),
        (OwnershipMeaning::Closing, PortFailure::Rejected),
        (OwnershipMeaning::Closing, PortFailure::Rejected),
    ]);
    let c = coordinator(
        store.clone(),
        audit,
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    assert_eq!(
        open(&c, "a").await,
        Err(OwnershipFailure::Audit(PortFailure::Uncertain))
    );
    bounded(async {
        loop {
            if store.read().await.unwrap().settlements.len() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    let root = c
        .active_root_for_session(&SessionId::new("a").unwrap())
        .unwrap();
    assert_eq!(c.lifetime_state(&root), Some(LifetimeState::Closing));
    assert_eq!(store.read().await.unwrap().lifetimes[0].lifetime_id, root);
}

async fn dropped_root(accepted: bool) {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let c = coordinator(
        store.clone(),
        audit.clone(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    let (gate, release) = audit.gate(OwnershipMeaning::Open);
    let opening = tokio::spawn({
        let c = c.clone();
        async move { open(&c, "a").await }
    });
    bounded(gate.entered.notified()).await;
    let root = gate
        .evidence
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .parent_lifetime
        .clone();
    opening.abort();
    let _ = opening.await;
    release
        .send(if accepted {
            Ok(())
        } else {
            Err(PortFailure::Rejected)
        })
        .unwrap();
    bounded(async {
        loop {
            if if accepted {
                c.lifetime_state(&root) == Some(LifetimeState::Closed)
            } else {
                c.lifetime_state(&root).is_none()
            } {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    bounded(open(&c, "b")).await.unwrap();
    let snapshot = store.read().await.unwrap();
    assert!(!snapshot
        .lifetimes
        .iter()
        .any(|r| r.lifetime_id == root && r.state == LifetimeState::Open));
}
#[tokio::test]
async fn row_12_dropped_root_waiter_does_not_abandon_definite_rejection() {
    dropped_root(false).await;
}
#[tokio::test]
async fn row_13_dropped_root_waiter_reconciles_accepted_identity() {
    dropped_root(true).await;
}

#[tokio::test]
async fn observed_task_receipt_survives_rejected_publication_in_nonrunnable_projection() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let c = coordinator(
        store.clone(),
        audit.clone(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    let root = open(&c, "a").await.unwrap();
    audit
        .failures
        .lock()
        .unwrap()
        .push_back((OwnershipMeaning::TaskAdmitted, PortFailure::Rejected));
    assert_eq!(
        c.spawn(command(&root)).await,
        Err(OwnershipFailure::Audit(PortFailure::Rejected))
    );
    let receipt = TaskReceiptId::new("receipt-1").unwrap();
    assert_eq!(
        store.read().await.unwrap().spawns[0].progress,
        SpawnProgress::Unconfirmed {
            known: KnownMilestone::TaskAdmitted { receipt }
        }
    );
}

struct ControlledStore {
    memory: MemoryOwnershipStore,
    outcome: Mutex<Option<(PortFailure, bool)>>,
    gate: Mutex<Option<Arc<Notify>>>,
    entered: Notify,
}
impl ControlledStore {
    fn new(outcome: Option<(PortFailure, bool)>) -> Arc<Self> {
        Arc::new(Self {
            memory: MemoryOwnershipStore::new(),
            outcome: Mutex::new(outcome),
            gate: Mutex::new(None),
            entered: Notify::new(),
        })
    }
}
#[async_trait]
impl OwnershipStore for ControlledStore {
    async fn read(&self) -> Result<OwnershipSnapshot, PortFailure> {
        self.memory.read().await
    }
    async fn write(&self, snapshot: &OwnershipSnapshot) -> Result<(), PortFailure> {
        let gate = self.gate.lock().unwrap().take();
        if let Some(gate) = gate {
            self.entered.notify_one();
            gate.notified().await;
        }
        let outcome = self.outcome.lock().unwrap().take();
        if let Some((failure, landed)) = outcome {
            if landed {
                self.memory.write(snapshot).await?;
            }
            return Err(failure);
        }
        self.memory.write(snapshot).await
    }
}
async fn failed_root_store(failure: PortFailure, landed: bool) {
    let store = ControlledStore::new(Some((failure, landed)));
    let audit = IndependentAudit::new();
    // Preserve a Closing root for explicit reconciliation, rather than erasing
    // an ID whose acceptance or write may already have been observed.
    audit.failures.lock().unwrap().extend([
        (OwnershipMeaning::Closing, PortFailure::Rejected),
        (OwnershipMeaning::Closing, PortFailure::Rejected),
    ]);
    let c = coordinator(
        store.clone(),
        audit.clone(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    assert_eq!(
        bounded(open(&c, "a")).await,
        Err(OwnershipFailure::Store(failure))
    );
    bounded(async {
        loop {
            if store.read().await.unwrap().settlements.len() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    let root = c
        .active_root_for_session(&SessionId::new("a").unwrap())
        .unwrap();
    let audited_id = audit
        .records
        .lock()
        .unwrap()
        .iter()
        .find(|e| e.after == OwnershipMeaning::Open)
        .unwrap()
        .parent_lifetime
        .clone();
    assert_eq!(root, audited_id);
    let resumed = coordinator(
        store.clone(),
        IndependentAudit::new(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    resumed.resume().await.unwrap();
    assert_eq!(
        resumed.active_root_for_session(&SessionId::new("a").unwrap()),
        Some(root.clone())
    );
    assert_eq!(resumed.lifetime_state(&root), Some(LifetimeState::Closing));
}
#[tokio::test]
async fn row_9_audit_eligible_root_survives_definite_store_rejection() {
    failed_root_store(PortFailure::Rejected, false).await;
}
#[tokio::test]
async fn row_10_write_then_uncertain_preserves_same_root_for_resume() {
    failed_root_store(PortFailure::Uncertain, true).await;
}
#[tokio::test]
async fn row_15_dropped_waiter_during_store_keeps_transaction_and_reconciles_id() {
    let store = ControlledStore::new(None);
    let release = Arc::new(Notify::new());
    *store.gate.lock().unwrap() = Some(release.clone());
    let audit = IndependentAudit::new();
    let c = coordinator(
        store.clone(),
        audit.clone(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    let opening = tokio::spawn({
        let c = c.clone();
        async move { open(&c, "a").await }
    });
    bounded(store.entered.notified()).await;
    let root = audit.records.lock().unwrap()[0].parent_lifetime.clone();
    opening.abort();
    let _ = opening.await;
    release.notify_one();
    bounded(async {
        loop {
            if c.lifetime_state(&root) == Some(LifetimeState::Closed) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(store.read().await.unwrap().lifetimes[0].lifetime_id, root);
}

#[tokio::test]
async fn private_report_does_not_leak_through_unrelated_commit() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let c = coordinator(
        store.clone(),
        audit.clone(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    let root = open(&c, "a").await.unwrap();
    let child = c.spawn(command(&root)).await.unwrap().child;
    let (gate, release) = audit.gate(OwnershipMeaning::Submitted);
    let report = tokio::spawn({
        let c = c.clone();
        async move {
            c.deliver_report(ReportId::new("report-a").unwrap(), &child, &root)
                .await
        }
    });
    bounded(gate.entered.notified()).await;
    bounded(open(&c, "b")).await.unwrap();
    assert!(store.read().await.unwrap().reports.is_empty());
    release.send(Err(PortFailure::Rejected)).unwrap();
    assert!(bounded(report).await.unwrap().is_err());
    bounded(open(&c, "another")).await.unwrap();
    assert!(store.read().await.unwrap().reports.is_empty());
}

#[tokio::test]
async fn row_14_queued_success_is_unclaimed_until_open_root_returns_ready() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let c = coordinator(
        store.clone(),
        audit,
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    let mut opening = Box::pin(open(&c, "a"));
    assert!(matches!(
        opening
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    // The Memory store write and success send complete in the owner's same poll;
    // the caller has never been polled again to claim the queued ticket.
    let root = bounded(async {
        loop {
            let snapshot = store.read().await.unwrap();
            if let Some(root) = snapshot.lifetimes.first() {
                break root.lifetime_id.clone();
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(c.lifetime_state(&root), Some(LifetimeState::Open));
    drop(opening);
    bounded(async {
        loop {
            if c.lifetime_state(&root) == Some(LifetimeState::Closed) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(
        store.read().await.unwrap().lifetimes[0].state,
        LifetimeState::Closed
    );
}

#[tokio::test]
async fn row_14_queued_root_drop_without_tokio_context_uses_original_live_runtime() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let c = coordinator(
        store.clone(),
        IndependentAudit::new(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    let mut opening = Box::pin(open(&c, "off-runtime"));
    assert!(matches!(
        opening
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    // This current-thread runtime runs the owner's memory-store write and
    // success send in the same poll. Observing its snapshot after yielding means
    // the success ticket is queued; opening is never repolled or claimed.
    let root = bounded(async {
        loop {
            let snapshot = store.read().await.unwrap();
            if let Some(root) = snapshot.lifetimes.first() {
                break root.lifetime_id.clone();
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(c.lifetime_state(&root), Some(LifetimeState::Open));
    let dropped = thread::scope(|scope| {
        scope
            .spawn(move || {
                assert!(Handle::try_current().is_err());
                drop(opening);
            })
            .join()
    });
    assert!(
        dropped.is_ok(),
        "unclaimed delivery must not depend on drop-thread context"
    );
    let snapshot = bounded(async {
        loop {
            let snapshot = store.read().await.unwrap();
            if snapshot
                .lifetimes
                .iter()
                .any(|row| row.lifetime_id == root && row.state == LifetimeState::Closed)
                && snapshot.settlements.iter().any(|row| {
                    row.physical == PhysicalFact::Released
                        && row.evidence == EvidenceFact::Acknowledged
                })
            {
                break snapshot;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(c.lifetime_state(&root), Some(LifetimeState::Closed));
    assert_eq!(snapshot.lifetimes[0].lifetime_id, root);
    assert_eq!(snapshot.lifetimes[0].state, LifetimeState::Closed);
    assert_eq!(snapshot.settlements[0].physical, PhysicalFact::Released);
    assert_eq!(snapshot.settlements[0].evidence, EvidenceFact::Acknowledged);
    assert!(c.participation(&root).is_none());
    assert_eq!(c.close_cause(&root), Some(LifetimeCause::OwnerDisposed));
    let replacement = open(&c, "off-runtime").await.unwrap();
    assert_ne!(replacement, root);
    bounded(close(&c, &replacement)).await.unwrap();
    // The original runtime stays alive through all reconciliation/settlement;
    // shutting down that executor is outside the delivery guarantee tested here.
}

#[tokio::test]
async fn previously_bound_root_cannot_use_never_bound_settlement() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let c = coordinator(
        store.clone(),
        audit,
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    let root = open(&c, "a").await.unwrap();
    let resources = Arc::new(ScriptResources {
        closes: AtomicUsize::new(0),
        report: ResourceReport {
            physical: PhysicalFact::Failed,
            evidence: EvidenceFact::Failed,
        },
        hold: None,
    });
    c.bind_resources(root.clone(), resources.clone()).unwrap();
    assert_eq!(
        bounded(close(&c, &root)).await,
        Err(OwnershipFailure::Incomplete)
    );
    assert_eq!(c.lifetime_state(&root), Some(LifetimeState::Closing));
    assert_eq!(resources.closes.load(Ordering::SeqCst), 1);
    let snapshot = store.read().await.unwrap();
    assert_eq!(snapshot.settlements[0].physical, PhysicalFact::Failed);
    assert_eq!(
        bounded(close(&c, &root)).await,
        Err(OwnershipFailure::Incomplete)
    );
    assert_eq!(resources.closes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn rejected_close_intent_is_persisted_before_cleanup_can_complete() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let c = coordinator(
        store.clone(),
        audit.clone(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    let root = open(&c, "a").await.unwrap();
    let release = Arc::new(Notify::new());
    let resources = Arc::new(ScriptResources {
        closes: AtomicUsize::new(0),
        report: released(),
        hold: Some(release.clone()),
    });
    c.bind_resources(root.clone(), resources.clone()).unwrap();
    audit
        .failures
        .lock()
        .unwrap()
        .push_back((OwnershipMeaning::Closing, PortFailure::Rejected));
    let closing = tokio::spawn({
        let c = c.clone();
        let root = root.clone();
        async move { close(&c, &root).await }
    });
    bounded(async {
        loop {
            if resources.closes.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    let snapshot = store.read().await.unwrap();
    assert_eq!(snapshot.lifetimes[0].state, LifetimeState::Closing);
    assert_eq!(snapshot.lifetimes[0].cause, Some(LifetimeCause::HostClose));
    release.notify_one();
    assert_eq!(
        bounded(closing).await.unwrap(),
        Err(OwnershipFailure::Audit(PortFailure::Rejected))
    );
}

struct DropResources {
    drops: Arc<AtomicUsize>,
    closes: Arc<AtomicUsize>,
}
impl Drop for DropResources {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
#[async_trait]
impl ChildResources for DropResources {
    async fn close(&self, _: &LifetimeCause, _: &Initiator) -> ResourceReport {
        self.closes.fetch_add(1, Ordering::SeqCst);
        released()
    }
}
fn tracked_owner(drops: &Arc<AtomicUsize>, closes: &Arc<AtomicUsize>) -> Arc<dyn ChildResources> {
    Arc::new(DropResources {
        drops: drops.clone(),
        closes: closes.clone(),
    })
}

#[tokio::test]
async fn row_21_absence_claim_refuses_late_binding_and_returns_sole_owner_even_after_audit_rejection(
) {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let c = coordinator(
        store,
        audit.clone(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    let root = open(&c, "a").await.unwrap();
    let (intent, intent_release) = audit.gate(OwnershipMeaning::Closing);
    let closing = tokio::spawn({
        let c = c.clone();
        let root = root.clone();
        async move { close(&c, &root).await }
    });
    bounded(intent.entered.notified()).await;
    let (absence, absence_release) = audit.gate(OwnershipMeaning::Closing);
    intent_release.send(Ok(())).unwrap();
    bounded(absence.entered.notified()).await;
    let drops = Arc::new(AtomicUsize::new(0));
    let closes = Arc::new(AtomicUsize::new(0));
    let refused = c
        .bind_resources(root.clone(), tracked_owner(&drops, &closes))
        .unwrap_err();
    assert_eq!(refused.reason, BindResourcesRefusal::Released);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert!(!format!("{refused:?}").contains("DropResources"));
    absence_release.send(Err(PortFailure::Rejected)).unwrap();
    assert_eq!(
        bounded(closing).await.unwrap(),
        Err(OwnershipFailure::Audit(PortFailure::Rejected))
    );
    assert_eq!(c.lifetime_state(&root), Some(LifetimeState::Closing));
    let refused_again = c
        .bind_resources(root.clone(), refused.resources)
        .unwrap_err();
    assert_eq!(refused_again.reason, BindResourcesRefusal::Released);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert_eq!(
        refused_again
            .resources
            .close(&LifetimeCause::HostClose, &Initiator::Runtime)
            .await,
        released()
    );
    drop(refused_again);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    bounded(close(&c, &root)).await.unwrap();
    assert_eq!(c.lifetime_state(&root), Some(LifetimeState::Closed));
}

#[tokio::test]
async fn rows_22_23_binding_wins_and_duplicate_unknown_closed_refusals_preserve_owners() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let c = coordinator(
        store,
        audit,
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    let root = open(&c, "a").await.unwrap();
    let drops = Arc::new(AtomicUsize::new(0));
    let closes = Arc::new(AtomicUsize::new(0));
    let first = tracked_owner(&drops, &closes);
    let rejected = tracked_owner(&drops, &closes);
    c.bind_resources(root.clone(), first.clone()).unwrap();
    let failure = c
        .bind_resources(root.clone(), rejected.clone())
        .unwrap_err();
    assert_eq!(failure.reason, BindResourcesRefusal::AlreadyBound);
    assert!(Arc::ptr_eq(&failure.resources, &rejected));
    let unknown = c
        .bind_resources(AgentLifetimeId::new("unknown").unwrap(), rejected.clone())
        .unwrap_err();
    assert_eq!(unknown.reason, BindResourcesRefusal::UnknownLifetime);
    assert!(Arc::ptr_eq(&unknown.resources, &rejected));
    bounded(close(&c, &root)).await.unwrap();
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    let closed = c.bind_resources(root, rejected.clone()).unwrap_err();
    assert_eq!(closed.reason, BindResourcesRefusal::Closed);
    assert!(Arc::ptr_eq(&closed.resources, &rejected));
    assert_eq!(drops.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn row_23_restored_closing_accepts_owner_before_release_and_refuses_owner_after_release() {
    for released_before in [false, true] {
        let store = Arc::new(MemoryOwnershipStore::new());
        let mut graph = OwnershipGraph::new();
        let root = AgentLifetimeId::new("root").unwrap();
        let opened = graph
            .open_root(
                SessionId::new("a").unwrap(),
                root.clone(),
                Initiator::Runtime,
            )
            .unwrap();
        assert_eq!(opened.after, OwnershipMeaning::Open);
        let operation = CloseOperationId::new("close").unwrap();
        graph
            .begin_close(
                &root,
                operation.clone(),
                LifetimeCause::HostClose,
                Initiator::Runtime,
            )
            .unwrap();
        if released_before {
            let recorded = graph
                .apply_report(
                    &root,
                    &operation,
                    &root,
                    PhysicalFact::Released,
                    EvidenceFact::Failed,
                )
                .unwrap();
            assert_eq!(recorded.after, OwnershipMeaning::Closing);
        }
        store.write(&graph.snapshot()).await.unwrap();
        let c = coordinator(
            store,
            IndependentAudit::new(),
            ScriptFactory::new(),
            Arc::new(LiveCapacity::new(8)),
        );
        c.resume().await.unwrap();
        let owner: Arc<dyn ChildResources> = Arc::new(ScriptResources {
            closes: AtomicUsize::new(0),
            report: released(),
            hold: None,
        });
        let result = c.bind_resources(root.clone(), owner.clone());
        if released_before {
            let failure = result.unwrap_err();
            assert_eq!(failure.reason, BindResourcesRefusal::Released);
            assert!(Arc::ptr_eq(&failure.resources, &owner));
        } else {
            result.unwrap();
            bounded(close(&c, &root)).await.unwrap();
            assert_eq!(c.lifetime_state(&root), Some(LifetimeState::Closed));
        }
    }
}

#[tokio::test]
async fn row_24_rejected_root_audit_preserves_preexisting_close_safety() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let c = coordinator(
        store.clone(),
        audit.clone(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    let (gate, release) = audit.gate(OwnershipMeaning::Open);
    let opening = tokio::spawn({
        let c = c.clone();
        async move { open(&c, "a").await }
    });
    bounded(gate.entered.notified()).await;
    let root = gate
        .evidence
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .parent_lifetime
        .clone();
    assert_eq!(
        bounded(close(&c, &root)).await,
        Err(OwnershipFailure::Incomplete)
    );
    release.send(Err(PortFailure::Rejected)).unwrap();
    assert_eq!(
        bounded(opening).await.unwrap(),
        Err(OwnershipFailure::Audit(PortFailure::Rejected))
    );
    bounded(async {
        loop {
            if c.lifetime_state(&root) == Some(LifetimeState::Closed) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    let snapshot = store.read().await.unwrap();
    assert_eq!(snapshot.lifetimes[0].lifetime_id, root);
    assert_eq!(snapshot.lifetimes[0].state, LifetimeState::Closed);
    assert_eq!(snapshot.lifetimes[0].cause, Some(LifetimeCause::HostClose));
}

#[tokio::test]
async fn row_24_failed_eligible_store_keeps_and_closes_actual_transferred_owner() {
    let store = ControlledStore::new(Some((PortFailure::Rejected, false)));
    let release = Arc::new(Notify::new());
    *store.gate.lock().unwrap() = Some(release.clone());
    let audit = IndependentAudit::new();
    let c = coordinator(
        store.clone(),
        audit,
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    let opening = tokio::spawn({
        let c = c.clone();
        async move { open(&c, "a").await }
    });
    bounded(store.entered.notified()).await;
    let root = c
        .active_root_for_session(&SessionId::new("a").unwrap())
        .unwrap();
    let resources = Arc::new(ScriptResources {
        closes: AtomicUsize::new(0),
        report: released(),
        hold: None,
    });
    c.bind_resources(root.clone(), resources.clone()).unwrap();
    release.notify_one();
    assert_eq!(
        bounded(opening).await.unwrap(),
        Err(OwnershipFailure::Store(PortFailure::Rejected))
    );
    bounded(async {
        loop {
            if c.lifetime_state(&root) == Some(LifetimeState::Closed) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(resources.closes.load(Ordering::SeqCst), 1);
    assert_eq!(store.read().await.unwrap().lifetimes[0].lifetime_id, root);
}

async fn sealed_pending_receipt(audit_result: Result<(), PortFailure>) {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let factory = ScriptFactory::new();
    let c = coordinator(
        store.clone(),
        audit.clone(),
        factory.clone(),
        Arc::new(LiveCapacity::new(8)),
    );
    let root = open(&c, "a").await.unwrap();
    let (gate, release) = audit.gate(OwnershipMeaning::TaskAdmitted);
    let spawning = tokio::spawn({
        let c = c.clone();
        let root = root.clone();
        async move { c.spawn(command(&root)).await }
    });
    bounded(gate.entered.notified()).await;
    let child = factory.last_child.lock().unwrap().clone().unwrap();
    // The initial submit already returned its actual provider receipt.
    assert_eq!(factory.submits(), 1);
    let closing = tokio::spawn({
        let c = c.clone();
        let root = root.clone();
        async move { close(&c, &root).await }
    });
    bounded(async {
        loop {
            if c.lifetime_state(&child) != Some(LifetimeState::Open) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    bounded(open(&c, "b")).await.unwrap();
    let expected = SpawnProgress::Unconfirmed {
        known: KnownMilestone::TaskAdmitted {
            receipt: TaskReceiptId::new("receipt-1").unwrap(),
        },
    };
    let snapshot = store.read().await.unwrap();
    assert_eq!(snapshot.spawns[0].progress, expected);
    assert!(
        snapshot
            .lifetimes
            .iter()
            .find(|r| r.lifetime_id == child)
            .unwrap()
            .state
            != LifetimeState::Open
    );
    let resumed_factory = ScriptFactory::new();
    let resumed = coordinator(
        store.clone(),
        IndependentAudit::new(),
        resumed_factory.clone(),
        Arc::new(LiveCapacity::new(8)),
    );
    resumed.resume().await.unwrap();
    assert_eq!(
        resumed.spawn_progress(&SpawnRequestId::new("pending-spawn").unwrap()),
        Some(expected)
    );
    assert_eq!(resumed_factory.prepares(), 0);
    assert_eq!(resumed_factory.submits(), 0);
    release.send(audit_result).unwrap();
    let _ = bounded(spawning).await.unwrap();
    bounded(closing).await.unwrap().unwrap();
    assert_eq!(c.close_cause(&child), Some(LifetimeCause::HostClose));
    assert_eq!(factory.submits(), 1);
    assert_eq!(c.lifetime_state(&child), Some(LifetimeState::Closed));
    assert_eq!(
        store.read().await.unwrap().spawns[0].progress.known(),
        KnownMilestone::TaskAdmitted {
            receipt: TaskReceiptId::new("receipt-1").unwrap()
        }
    );
}
#[tokio::test]
async fn row_25_close_over_pending_task_ack_retains_actual_receipt_through_resume() {
    sealed_pending_receipt(Ok(())).await;
}
#[tokio::test]
async fn row_25_close_over_pending_task_rejection_retains_actual_receipt_through_resume() {
    sealed_pending_receipt(Err(PortFailure::Rejected)).await;
}

#[tokio::test]
async fn row_26_handed_out_gate_blocks_absence_through_rejected_close_and_remains_sealed() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let c = coordinator(
        store.clone(),
        audit.clone(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    let root = open(&c, "a").await.unwrap();
    let gate = c.participation(&root).unwrap();
    audit
        .failures
        .lock()
        .unwrap()
        .push_back((OwnershipMeaning::Closing, PortFailure::Rejected));
    assert_eq!(
        bounded(close(&c, &root)).await,
        Err(OwnershipFailure::Incomplete)
    );
    assert!(gate.is_sealed());
    let snapshot = store.read().await.unwrap();
    assert_eq!(snapshot.lifetimes[0].state, LifetimeState::Closing);
    assert!(snapshot.settlements.is_empty());
    gate.note_attachment(true, true).await;
    assert!(gate.is_sealed());
    assert_eq!(c.lifetime_state(&root), Some(LifetimeState::Closed));
    assert!(c.participation(&root).is_none());
}

#[tokio::test]
async fn row_26_gate_handoff_before_caller_drop_retains_possible_external_owner() {
    let store = ControlledStore::new(None);
    let release = Arc::new(Notify::new());
    *store.gate.lock().unwrap() = Some(release.clone());
    let c = coordinator(
        store.clone(),
        IndependentAudit::new(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    let opening = tokio::spawn({
        let c = c.clone();
        async move { open(&c, "a").await }
    });
    bounded(store.entered.notified()).await;
    let root = c
        .active_root_for_session(&SessionId::new("a").unwrap())
        .unwrap();
    let gate = c.participation(&root).unwrap();
    opening.abort();
    let _ = opening.await;
    release.notify_one();
    bounded(async {
        loop {
            if gate.is_sealed() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    bounded(async {
        loop {
            if store.read().await.unwrap().lifetimes[0].state == LifetimeState::Closing {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(c.lifetime_state(&root), Some(LifetimeState::Closing));
    assert!(store.read().await.unwrap().settlements.is_empty());
    gate.note_attachment(true, true).await;
    assert_eq!(c.lifetime_state(&root), Some(LifetimeState::Closed));
}

#[tokio::test]
async fn row_27_absence_claim_refuses_new_gate_during_rejected_audit_and_after_close() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let c = coordinator(
        store,
        audit.clone(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    let root = open(&c, "a").await.unwrap();
    let (intent, release_intent) = audit.gate(OwnershipMeaning::Closing);
    let closing = tokio::spawn({
        let c = c.clone();
        let root = root.clone();
        async move { close(&c, &root).await }
    });
    bounded(intent.entered.notified()).await;
    let (absence, release_absence) = audit.gate(OwnershipMeaning::Closing);
    release_intent.send(Ok(())).unwrap();
    bounded(absence.entered.notified()).await;
    assert!(c.participation(&root).is_none());
    release_absence.send(Err(PortFailure::Rejected)).unwrap();
    assert_eq!(
        bounded(closing).await.unwrap(),
        Err(OwnershipFailure::Audit(PortFailure::Rejected))
    );
    assert!(c.participation(&root).is_none());
    bounded(close(&c, &root)).await.unwrap();
    assert!(c.participation(&root).is_none());
}

async fn private_child_transfer(accepted: bool) {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let factory = ScriptFactory::new();
    let room = Arc::new(LiveCapacity::new(1));
    let c = coordinator(store, audit.clone(), factory.clone(), room.clone());
    let root = open(&c, "a").await.unwrap();
    let (gate, release) = audit.gate(OwnershipMeaning::Reserved);
    let spawning = tokio::spawn({
        let c = c.clone();
        let root = root.clone();
        async move { c.spawn(command(&root)).await }
    });
    bounded(gate.entered.notified()).await;
    let child = c.children(&root, None, 1).unwrap().children[0]
        .lifetime
        .clone();
    assert!(c.participation(&child).is_none());
    let drops = Arc::new(AtomicUsize::new(0));
    let closes = Arc::new(AtomicUsize::new(0));
    let refusal = c
        .bind_resources(child.clone(), tracked_owner(&drops, &closes))
        .unwrap_err();
    assert_eq!(refusal.reason, BindResourcesRefusal::UnpublishedLifetime);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert_eq!(factory.prepares(), 0);
    release
        .send(if accepted {
            Ok(())
        } else {
            Err(PortFailure::Rejected)
        })
        .unwrap();
    let result = bounded(spawning).await.unwrap();
    if accepted {
        result.unwrap();
        assert!(c.participation(&child).is_some());
        let occupied = c.bind_resources(child, refusal.resources).unwrap_err();
        assert_eq!(occupied.reason, BindResourcesRefusal::AlreadyBound);
        drop(occupied);
    } else {
        assert_eq!(result, Err(OwnershipFailure::Audit(PortFailure::Rejected)));
        assert!(c.participation(&child).is_none());
        assert!(room.try_reserve());
        room.release();
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        refusal
            .resources
            .close(&LifetimeCause::HostClose, &Initiator::Runtime)
            .await;
        drop(refusal);
        assert_eq!(closes.load(Ordering::SeqCst), 1);
    }
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn row_28_private_child_read_cannot_transfer_and_rejection_returns_owner() {
    private_child_transfer(false).await;
}
#[tokio::test]
async fn row_28_acknowledged_child_subsequently_allows_gate_and_retains_factory_owner() {
    private_child_transfer(true).await;
}

async fn private_parent_spawn(accepted: bool) {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let factory = ScriptFactory::new();
    let room = Arc::new(LiveCapacity::new(1));
    let c = coordinator(store, audit.clone(), factory.clone(), room.clone());
    let (gate, release) = audit.gate(OwnershipMeaning::Open);
    let opening = tokio::spawn({
        let c = c.clone();
        async move { open(&c, "a").await }
    });
    bounded(gate.entered.notified()).await;
    let root = gate
        .evidence
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .parent_lifetime
        .clone();
    assert_eq!(
        c.spawn(command(&root)).await,
        Err(OwnershipFailure::UnpublishedParent)
    );
    assert_eq!(factory.prepares(), 0);
    assert!(c.children(&root, None, 1).unwrap().children.is_empty());
    assert!(room.try_reserve());
    room.release();
    assert!(c.participation(&root).is_none());
    release
        .send(if accepted {
            Ok(())
        } else {
            Err(PortFailure::Rejected)
        })
        .unwrap();
    let result = bounded(opening).await.unwrap();
    if accepted {
        result.unwrap();
        c.spawn(command(&root)).await.unwrap();
        assert_eq!(factory.prepares(), 1);
    } else {
        assert_eq!(result, Err(OwnershipFailure::Audit(PortFailure::Rejected)));
        assert_eq!(
            c.spawn(command(&root)).await,
            Err(OwnershipFailure::UnpublishedParent)
        );
        assert_eq!(factory.prepares(), 0);
    }
}
#[tokio::test]
async fn row_29_private_parent_spawns_refused_until_audit_accepts() {
    private_parent_spawn(true).await;
}
#[tokio::test]
async fn row_29_private_parent_rejection_keeps_descendant_factory_unreachable() {
    private_parent_spawn(false).await;
}

#[tokio::test]
async fn row_30_already_unconfirmed_safety_persists_despite_rejected_or_uncertain_audit() {
    for failure in [PortFailure::Rejected, PortFailure::Uncertain] {
        let store = Arc::new(MemoryOwnershipStore::new());
        let audit = IndependentAudit::new();
        let factory = ScriptFactory::new();
        let c = coordinator(
            store.clone(),
            audit.clone(),
            factory.clone(),
            Arc::new(LiveCapacity::new(8)),
        );
        let root = open(&c, "a").await.unwrap();
        *factory.fail.lock().unwrap() = Some(PrepareFailure {
            failure: PortFailure::Uncertain,
            cleanup: None,
        });
        audit
            .failures
            .lock()
            .unwrap()
            .push_back((OwnershipMeaning::Unconfirmed, failure));
        assert_eq!(
            c.spawn(command(&root)).await,
            Err(OwnershipFailure::Startup(PortFailure::Uncertain))
        );
        assert_eq!(
            store.read().await.unwrap().spawns[0].progress,
            SpawnProgress::Unconfirmed {
                known: KnownMilestone::Reserved
            }
        );
    }
}
#[tokio::test]
async fn row_30_ended_startup_safety_persists_when_both_safety_audits_reject() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let factory = ScriptFactory::new();
    let c = coordinator(
        store.clone(),
        audit.clone(),
        factory.clone(),
        Arc::new(LiveCapacity::new(8)),
    );
    let root = open(&c, "a").await.unwrap();
    *factory.fail.lock().unwrap() = Some(PrepareFailure {
        failure: PortFailure::Rejected,
        cleanup: None,
    });
    audit.failures.lock().unwrap().extend([
        (OwnershipMeaning::StartupFailed, PortFailure::Rejected),
        (OwnershipMeaning::Ended, PortFailure::Rejected),
    ]);
    assert_eq!(
        c.spawn(command(&root)).await,
        Err(OwnershipFailure::Startup(PortFailure::Rejected))
    );
    assert_eq!(
        store.read().await.unwrap().spawns[0].progress,
        SpawnProgress::Ended {
            known: KnownMilestone::Reserved
        }
    );
}
#[tokio::test]
async fn row_31_suppressed_report_survives_rejected_safety_audit() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let c = coordinator(
        store.clone(),
        audit.clone(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(8)),
    );
    let root = open(&c, "a").await.unwrap();
    let child = c.spawn(command(&root)).await.unwrap().child;
    bounded(close(&c, &root)).await.unwrap();
    audit
        .failures
        .lock()
        .unwrap()
        .push_back((OwnershipMeaning::Suppressed, PortFailure::Rejected));
    assert_eq!(
        c.deliver_report(ReportId::new("suppressed").unwrap(), &child, &root)
            .await,
        Err(OwnershipFailure::Audit(PortFailure::Rejected))
    );
    assert_eq!(
        store.read().await.unwrap().reports[0].state,
        DeliveryState::Suppressed
    );
}
#[tokio::test]
async fn row_32_uncertain_initial_reservation_retains_sealed_child_and_capacity() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let room = Arc::new(LiveCapacity::new(1));
    let c = coordinator(
        store.clone(),
        audit.clone(),
        ScriptFactory::new(),
        room.clone(),
    );
    let root = open(&c, "a").await.unwrap();
    audit
        .failures
        .lock()
        .unwrap()
        .push_back((OwnershipMeaning::Reserved, PortFailure::Uncertain));
    assert_eq!(
        c.spawn(command(&root)).await,
        Err(OwnershipFailure::Audit(PortFailure::Uncertain))
    );
    let child = c.children(&root, None, 1).unwrap().children[0]
        .lifetime
        .clone();
    assert_eq!(c.lifetime_state(&child), Some(LifetimeState::Closing));
    assert!(c.participation(&child).unwrap().is_sealed());
    assert!(!room.try_reserve());
    let snapshot = store.read().await.unwrap();
    assert_eq!(
        snapshot.spawns[0].progress,
        SpawnProgress::Unconfirmed {
            known: KnownMilestone::Reserved
        }
    );
    assert_eq!(
        snapshot
            .lifetimes
            .iter()
            .find(|r| r.lifetime_id == child)
            .unwrap()
            .state,
        LifetimeState::Closing
    );
}

async fn held_factory_transfer(failure: Option<PortFailure>, drop_caller: bool) {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let factory = ScriptFactory::new();
    let release = Arc::new(Notify::new());
    *factory.release.lock().unwrap() = Some(release.clone());
    let internal_drops = Arc::new(AtomicUsize::new(0));
    let internal_closes = Arc::new(AtomicUsize::new(0));
    if let Some(failure) = failure {
        *factory.fail.lock().unwrap() = Some(PrepareFailure {
            failure,
            cleanup: Some(tracked_owner(&internal_drops, &internal_closes)),
        });
    }
    let c = coordinator(
        store.clone(),
        audit,
        factory.clone(),
        Arc::new(LiveCapacity::new(1)),
    );
    let root = open(&c, "a").await.unwrap();
    let spawning = tokio::spawn({
        let c = c.clone();
        let root = root.clone();
        async move { c.spawn(command(&root)).await }
    });
    bounded(factory.entered.notified()).await;
    let child = factory.last_child.lock().unwrap().clone().unwrap();
    let supplied_gate = factory.last_gate.lock().unwrap().clone().unwrap();
    assert!(!supplied_gate.is_sealed());
    assert!(c.participation(&child).is_none());
    let drops = Arc::new(AtomicUsize::new(0));
    let closes = Arc::new(AtomicUsize::new(0));
    let refusal = c
        .bind_resources(child.clone(), tracked_owner(&drops, &closes))
        .unwrap_err();
    assert_eq!(refusal.reason, BindResourcesRefusal::FactoryInFlight);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    if !drop_caller {
        release.notify_one();
        let result = bounded(spawning).await.unwrap();
        if let Some(failure) = failure {
            assert_eq!(result, Err(OwnershipFailure::Startup(failure)));
        } else {
            result.unwrap();
        }
    } else {
        spawning.abort();
        let _ = spawning.await;
        assert!(c.participation(&child).is_none());
        release.notify_one();
        bounded(async {
            loop {
                if c.participation(&child).is_some() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
    }
    let public_gate = c.participation(&child).unwrap();
    assert!(Arc::ptr_eq(&supplied_gate.seal(), &public_gate.seal()));
    assert!(Arc::ptr_eq(
        &supplied_gate.admission_scope(),
        &public_gate.admission_scope()
    ));
    if failure.is_some() {
        assert!(supplied_gate.is_sealed());
    }
    assert_eq!(internal_drops.load(Ordering::SeqCst), 0);
    close(&c, &root).await.unwrap();
    assert!(supplied_gate.is_sealed());
    if failure.is_some() {
        assert_eq!(internal_closes.load(Ordering::SeqCst), 1);
    } else {
        assert_eq!(
            factory.children.lock().unwrap()[0]
                .closes
                .load(Ordering::SeqCst),
            1
        );
    }
    assert_eq!(closes.load(Ordering::SeqCst), 0);
    drop(refusal);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn rows_33_34_held_factory_retains_internal_success_owner_and_shared_gate() {
    held_factory_transfer(None, false).await;
}
#[tokio::test]
async fn rows_33_35_held_factory_retains_failure_cleanup_and_revokes_gate() {
    held_factory_transfer(Some(PortFailure::Uncertain), false).await;
}
#[tokio::test]
async fn row_33_dropped_factory_waiter_does_not_release_transfer_slot() {
    held_factory_transfer(None, true).await;
}
#[tokio::test]
async fn row_35_rejected_none_revokes_gate_before_rejected_audit_and_releases_capacity_honestly() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let factory = ScriptFactory::new();
    *factory.fail.lock().unwrap() = Some(PrepareFailure {
        failure: PortFailure::Rejected,
        cleanup: None,
    });
    let (gate, release) = audit.gate(OwnershipMeaning::StartupFailed);
    let room = Arc::new(LiveCapacity::new(1));
    let c = coordinator(store.clone(), audit, factory.clone(), room.clone());
    let root = open(&c, "a").await.unwrap();
    let spawning = tokio::spawn({
        let c = c.clone();
        async move { c.spawn(command(&root)).await }
    });
    bounded(gate.entered.notified()).await;
    let supplied = factory.last_gate.lock().unwrap().clone().unwrap();
    assert!(supplied.is_sealed());
    assert!(room.try_reserve(), "actual Ready absence returns capacity before fallible startup audit");
    room.release();
    release.send(Err(PortFailure::Rejected)).unwrap();
    assert_eq!(
        bounded(spawning).await.unwrap(),
        Err(OwnershipFailure::Startup(PortFailure::Rejected))
    );
    assert!(room.try_reserve());
    room.release();
    let snapshot = store.read().await.unwrap();
    assert!(matches!(
        snapshot.spawns[0].progress,
        SpawnProgress::Ended { .. }
    ));
    assert_eq!(
        snapshot
            .lifetimes
            .iter()
            .find(|r| r.lifetime_id == snapshot.spawns[0].child_lifetime)
            .unwrap()
            .state,
        LifetimeState::Closed
    );
    assert!(matches!(snapshot.settlements[0].proof, nessa_sdk::domain::agent_execution::subagents::SettlementProof::Absence(_)));
    let restored = coordinator(
        store,
        IndependentAudit::new(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(1)),
    );
    restored.resume().await.unwrap();
    let child = snapshot.spawns[0].child_lifetime.clone();
    assert!(restored.participation(&child).is_none());
    assert!(c.participation(&child).is_none());
    let drops = Arc::new(AtomicUsize::new(0));
    let closes = Arc::new(AtomicUsize::new(0));
    for coordinator in [&c, &restored] {
        let refusal = coordinator.bind_resources(child.clone(), tracked_owner(&drops, &closes)).unwrap_err();
        assert_eq!(refusal.reason, BindResourcesRefusal::Closed);
        drop(refusal);
        assert!(coordinator.participation(&child).is_none());
    }
    assert_eq!(closes.load(Ordering::SeqCst), 0);
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    assert!(room.try_reserve());
    room.release();
}

#[tokio::test]
async fn row_35_restored_unfinished_cleanup_accepts_vacant_binding_but_keeps_attachment_sealed() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let factory = ScriptFactory::new();
    let drops = Arc::new(AtomicUsize::new(0));
    let closes = Arc::new(AtomicUsize::new(0));
    *factory.fail.lock().unwrap() = Some(PrepareFailure {
        failure: PortFailure::Uncertain,
        cleanup: Some(tracked_owner(&drops, &closes)),
    });
    let c = coordinator(
        store.clone(),
        IndependentAudit::new(),
        factory,
        Arc::new(LiveCapacity::new(1)),
    );
    let root = open(&c, "a").await.unwrap();
    assert_eq!(
        c.spawn(command(&root)).await,
        Err(OwnershipFailure::Startup(PortFailure::Uncertain))
    );
    let child = c.children(&root, None, 1).unwrap().children[0]
        .lifetime
        .clone();
    let restored = coordinator(
        store,
        IndependentAudit::new(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(1)),
    );
    restored.resume().await.unwrap();
    assert!(restored.participation(&child).unwrap().is_sealed());
    let restored_drops = Arc::new(AtomicUsize::new(0));
    let restored_closes = Arc::new(AtomicUsize::new(0));
    restored
        .bind_resources(
            child.clone(),
            tracked_owner(&restored_drops, &restored_closes),
        )
        .unwrap();
    close(&restored, &child).await.unwrap();
    assert_eq!(restored_closes.load(Ordering::SeqCst), 1);
    assert_eq!(restored_drops.load(Ordering::SeqCst), 0);
    close(&c, &root).await.unwrap();
    assert_eq!(closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn row_35_restored_ended_factual_owner_is_not_resource_absence() {
    for prepared in [true, false] {
        let store = Arc::new(MemoryOwnershipStore::new());
        let audit = IndependentAudit::new();
        let factory = ScriptFactory::new();
        if prepared {
            audit
                .failures
                .lock()
                .unwrap()
                .push_back((OwnershipMeaning::Attached, PortFailure::Rejected));
        }
        let c = coordinator(
            store.clone(),
            audit,
            factory.clone(),
            Arc::new(LiveCapacity::new(1)),
        );
        let root = open(&c, "a").await.unwrap();
        let command = command(&root);
        let result = c.spawn(command.clone()).await;
        if prepared {
            assert_eq!(result, Err(OwnershipFailure::Audit(PortFailure::Rejected)));
        } else {
            result.unwrap();
        }
        let mut retained = store.read().await.unwrap();
        // Exercise legacy Open nonrunnable history independently of today's
        // typed-error close. Prepared is factual ownership, not absence.
        for row in &mut retained.lifetimes {
            if row.lifetime_id == retained.spawns[0].child_lifetime {
                row.state = LifetimeState::Open;
                row.close_operation = None;
                row.cause = None;
                row.initiator = None;
                row.cascaded_from = None;
            }
        }
        let mut graph = OwnershipGraph::restore(retained);
        let known = graph.spawn_progress(&command.request_id).unwrap().known();
        if prepared {
            assert_eq!(known, KnownMilestone::Prepared);
        } else {
            assert!(matches!(known, KnownMilestone::TaskAdmitted { .. }));
            let _ = graph
                .advance_spawn(
                    &command.request_id,
                    SpawnProgress::Unconfirmed {
                        known: known.clone(),
                    },
                )
                .unwrap();
        }
        let _ = graph
            .advance_spawn(
                &command.request_id,
                SpawnProgress::Draining {
                    known: known.clone(),
                },
            )
            .unwrap();
        let _ = graph
            .advance_spawn(
                &command.request_id,
                SpawnProgress::Ended {
                    known: known.clone(),
                },
            )
            .unwrap();
        let snapshot = graph.snapshot();
        assert!(snapshot.settlements.is_empty());
        assert_eq!(
            snapshot
                .lifetimes
                .iter()
                .find(|row| row.lifetime_id == snapshot.spawns[0].child_lifetime)
                .unwrap()
                .state,
            LifetimeState::Open
        );
        let owner = factory.children.lock().unwrap()[0].clone();
        assert_eq!(owner.closes.load(Ordering::SeqCst), 0);
        drop(c);
        drop(factory);
        assert_eq!(Arc::strong_count(&owner), 1);
        let weak = Arc::downgrade(&owner);
        let history = Arc::new(MemoryOwnershipStore::new());
        history.write(&snapshot).await.unwrap();
        let restored = coordinator(
            history.clone(),
            IndependentAudit::new(),
            ScriptFactory::new(),
            Arc::new(LiveCapacity::new(1)),
        );
        restored.resume().await.unwrap();
        let child = snapshot.spawns[0].child_lifetime.clone();
        assert!(restored.participation(&child).unwrap().is_sealed());
        restored.bind_resources(child.clone(), owner).unwrap();
        assert_eq!(weak.upgrade().unwrap().closes.load(Ordering::SeqCst), 0);
        assert_eq!(history.read().await.unwrap(), snapshot);
        close(&restored, &child).await.unwrap();
        assert_eq!(weak.upgrade().unwrap().closes.load(Ordering::SeqCst), 1);
        assert_eq!(
            history.read().await.unwrap().spawns[0].progress,
            SpawnProgress::Ended { known }
        );
    }
}

#[tokio::test]
async fn row_36_rooted_refused_history_returns_actual_owner_and_preserves_inspection_evidence() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let factory = ScriptFactory::new();
    let c = coordinator(
        store.clone(),
        IndependentAudit::new(),
        factory.clone(),
        Arc::new(LiveCapacity::new(1)),
    );
    let root = open(&c, "a").await.unwrap();
    let receipt = c.spawn(command(&root)).await.unwrap();
    let mut snapshot = store.read().await.unwrap();
    snapshot.spawns[0].binding.parent_session = SessionId::new("foreign-parent-session").unwrap();
    let owner = factory.children.lock().unwrap()[0].clone();
    drop(c);
    drop(factory);
    assert_eq!(Arc::strong_count(&owner), 1);
    let weak = Arc::downgrade(&owner);
    let history = Arc::new(MemoryOwnershipStore::new());
    history.write(&snapshot).await.unwrap();
    let restored_factory = ScriptFactory::new();
    let restored = coordinator(
        history.clone(),
        IndependentAudit::new(),
        restored_factory.clone(),
        Arc::new(LiveCapacity::new(1)),
    );
    restored.resume().await.unwrap();
    assert!(restored.participation(&receipt.child).unwrap().is_sealed());
    let refusal = restored
        .bind_resources(receipt.child.clone(), owner)
        .unwrap_err();
    assert_eq!(refusal.reason, BindResourcesRefusal::RefusedHistory);
    assert_eq!(weak.upgrade().unwrap().closes.load(Ordering::SeqCst), 0);
    assert_eq!(history.read().await.unwrap(), snapshot);
    let reloaded = coordinator(
        history.clone(),
        IndependentAudit::new(),
        restored_factory.clone(),
        Arc::new(LiveCapacity::new(1)),
    );
    reloaded.resume().await.unwrap();
    assert!(reloaded.participation(&receipt.child).unwrap().is_sealed());
    let refusal = reloaded
        .bind_resources(receipt.child, refusal.resources)
        .unwrap_err();
    assert_eq!(refusal.reason, BindResourcesRefusal::RefusedHistory);
    let mut fresh = command(&root);
    fresh.request_id = SpawnRequestId::new("new-request").unwrap();
    assert_eq!(
        reloaded.spawn(fresh).await,
        Err(OwnershipFailure::Domain(OwnershipError::DispatchRefused))
    );
    assert_eq!(restored_factory.prepares(), 0);
    refusal
        .resources
        .close(&LifetimeCause::HostClose, &Initiator::Runtime)
        .await;
    assert_eq!(weak.upgrade().unwrap().closes.load(Ordering::SeqCst), 1);
    drop(refusal);
    assert!(weak.upgrade().is_none());
    assert_eq!(history.read().await.unwrap(), snapshot);
}

#[tokio::test]
async fn row_38_legacy_open_ended_reserved_refuses_transfer_without_claiming_closing_absence() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let factory = ScriptFactory::new();
    *factory.fail.lock().unwrap() = Some(PrepareFailure {
        failure: PortFailure::Rejected,
        cleanup: None,
    });
    let c = coordinator(
        store.clone(),
        IndependentAudit::new(),
        factory,
        Arc::new(LiveCapacity::new(1)),
    );
    let root = open(&c, "legacy").await.unwrap();
    assert_eq!(
        c.spawn(command(&root)).await,
        Err(OwnershipFailure::Startup(PortFailure::Rejected))
    );
    let mut history = store.read().await.unwrap();
    let child = history.spawns[0].child_lifetime.clone();
    assert_eq!(
        history.spawns[0].progress,
        SpawnProgress::Ended {
            known: KnownMilestone::Reserved
        }
    );
    for row in &mut history.lifetimes {
        if row.lifetime_id == child {
            row.state = LifetimeState::Open;
            row.close_operation = None;
            row.cause = None;
            row.initiator = None;
            row.cascaded_from = None;
        }
    }
    // This fixture represents the old progress-only history, before typed proof existed.
    history.settlements.retain(|row| row.target != child);
    history.close_completions.retain(|row| row.close_lifetime() != &child);
    let retained = Arc::new(MemoryOwnershipStore::new());
    retained.write(&history).await.unwrap();
    let restored = coordinator(
        retained.clone(),
        IndependentAudit::new(),
        ScriptFactory::new(),
        Arc::new(LiveCapacity::new(1)),
    );
    restored.resume().await.unwrap();
    assert!(restored.participation(&child).is_none());
    let drops = Arc::new(AtomicUsize::new(0));
    let closes = Arc::new(AtomicUsize::new(0));
    let owner = tracked_owner(&drops, &closes);
    let weak = Arc::downgrade(&owner);
    let refusal = restored.bind_resources(child, owner).unwrap_err();
    assert_eq!(refusal.reason, BindResourcesRefusal::Released);
    assert!(Arc::ptr_eq(&weak.upgrade().unwrap(), &refusal.resources));
    assert_eq!(closes.load(Ordering::SeqCst), 0);
    assert_eq!(retained.read().await.unwrap(), history);
}

#[tokio::test]
async fn row_38_publication_error_matrix_seals_actual_gate_preserves_owner_receipt_and_restore() {
    for stage in [
        OwnershipMeaning::Reserved,
        OwnershipMeaning::Prepared,
        OwnershipMeaning::Attached,
        OwnershipMeaning::TaskAdmitted,
    ] {
        for failure in [PortFailure::Rejected, PortFailure::Uncertain] {
            for store_failure in [false, true] {
                let store = Arc::new(MemoryOwnershipStore::new());
                let audit = IndependentAudit::new();
                let factory = ScriptFactory::new();
                let room = Arc::new(LiveCapacity::new(8));
                let c = coordinator(store.clone(), audit.clone(), factory.clone(), room.clone());
                let root = open(&c, "matrix").await.unwrap();
                let (gate, release) = audit.gate(stage.clone());
                let spawning = tokio::spawn({
                    let c = c.clone();
                    let root = root.clone();
                    async move { c.spawn(command(&root)).await }
                });
                bounded(gate.entered.notified()).await;
                let child = gate
                    .evidence
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .child_lifetime
                    .clone()
                    .unwrap();
                if store_failure {
                    store.fail_next_write(failure);
                }
                release
                    .send(if store_failure { Ok(()) } else { Err(failure) })
                    .unwrap();
                let expected = if store_failure {
                    OwnershipFailure::Store(failure)
                } else {
                    OwnershipFailure::Audit(failure)
                };
                assert_eq!(
                    bounded(spawning).await.unwrap(),
                    Err(expected),
                    "{stage:?}/{failure:?}/store={store_failure}"
                );
                assert_eq!(c.lifetime_state(&child), Some(LifetimeState::Closing));
                assert_eq!(c.close_cause(&child), Some(LifetimeCause::TerminalFailure));
                let private = stage == OwnershipMeaning::Reserved
                    && !store_failure
                    && failure == PortFailure::Rejected;
                let actual_absence = stage == OwnershipMeaning::Reserved && failure == PortFailure::Rejected;
                if private || actual_absence {
                    assert!(c.participation(&child).is_none());
                } else {
                    assert!(c.participation(&child).unwrap().is_sealed());
                }
                let mut descendant = command(&child);
                descendant.request_id = SpawnRequestId::new("late-descendant").unwrap();
                assert_eq!(
                    c.spawn(descendant).await,
                    Err(if private {
                        OwnershipFailure::UnpublishedParent
                    } else {
                        OwnershipFailure::Domain(OwnershipError::ParentClosing)
                    })
                );
                let initial = stage == OwnershipMeaning::Reserved;
                assert_eq!(factory.prepares(), usize::from(!initial));
                assert_eq!(
                    factory.submits(),
                    usize::from(stage == OwnershipMeaning::TaskAdmitted)
                );
                if !initial {
                    assert!(factory
                        .last_gate
                        .lock()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .is_sealed());
                    assert_eq!(
                        factory.children.lock().unwrap()[0]
                            .closes
                            .load(Ordering::SeqCst),
                        0
                    );
                    let supplied = Arc::new(ScriptResources {
                        closes: AtomicUsize::new(0),
                        report: released(),
                        hold: None,
                    });
                    assert_eq!(
                        c.bind_resources(child.clone(), supplied)
                            .unwrap_err()
                            .reason,
                        BindResourcesRefusal::AlreadyBound
                    );
                }
                let snapshot = store.read().await.unwrap();
                if private {
                    assert!(snapshot.spawns.is_empty());
                } else {
                    if stage == OwnershipMeaning::TaskAdmitted {
                        assert!(matches!(
                            snapshot.spawns[0].progress.known(),
                            KnownMilestone::TaskAdmitted { .. }
                        ));
                    }
                    let restored_factory = ScriptFactory::new();
                    let restored = coordinator(
                        store,
                        IndependentAudit::new(),
                        restored_factory.clone(),
                        Arc::new(LiveCapacity::new(8)),
                    );
                    restored.resume().await.unwrap();
                    assert_eq!(
                        restored.lifetime_state(&child),
                        Some(LifetimeState::Closing)
                    );
                    if actual_absence { assert!(restored.participation(&child).is_none()); }
                    else { assert!(restored.participation(&child).unwrap().is_sealed()); }
                    assert_eq!(restored_factory.prepares(), 0);
                }
                let mut available = 0;
                while room.try_reserve() {
                    available += 1;
                }
                let returned = initial && failure == PortFailure::Rejected;
                assert_eq!(available, if returned { 8 } else { 7 });
                for _ in 0..available {
                    room.release();
                }
            }
        }
    }
}

#[tokio::test]
async fn row_38_factory_and_submission_failures_revoke_descendants_without_erasing_actual_cleanup()
{
    for failure in [PortFailure::Rejected, PortFailure::Uncertain] {
        for factory_failure in [false, true] {
            for cleanup in [false, true] {
                if !factory_failure && cleanup {
                    continue;
                }
                let store = Arc::new(MemoryOwnershipStore::new());
                let factory = ScriptFactory::new();
                let owner = Arc::new(ScriptResources {
                    closes: AtomicUsize::new(0),
                    report: ResourceReport {
                        physical: PhysicalFact::Failed,
                        evidence: EvidenceFact::Failed,
                    },
                    hold: None,
                });
                if factory_failure {
                    *factory.fail.lock().unwrap() = Some(PrepareFailure {
                        failure,
                        cleanup: cleanup.then(|| owner.clone() as Arc<dyn ChildResources>),
                    });
                } else {
                    *factory.submit_result.lock().unwrap() = Err(failure);
                }
                let room = Arc::new(LiveCapacity::new(8));
                let c = coordinator(
                    store.clone(),
                    IndependentAudit::new(),
                    factory.clone(),
                    room.clone(),
                );
                let root = open(&c, "effect").await.unwrap();
                assert_eq!(
                    c.spawn(command(&root)).await,
                    Err(if factory_failure {
                        OwnershipFailure::Startup(failure)
                    } else {
                        OwnershipFailure::Submission(failure)
                    })
                );
                let child = factory.last_child.lock().unwrap().clone().unwrap();
                let definite_absence = factory_failure && !cleanup && failure == PortFailure::Rejected;
                assert_eq!(c.lifetime_state(&child), Some(if definite_absence { LifetimeState::Closed } else { LifetimeState::Closing }));
                assert!(factory
                    .last_gate
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .is_sealed());
                if definite_absence { assert!(c.participation(&child).is_none()); }
                else { assert!(c.participation(&child).unwrap().is_sealed()); }
                let mut descendant = command(&child);
                descendant.request_id = SpawnRequestId::new("effect-descendant").unwrap();
                assert_eq!(
                    c.spawn(descendant).await,
                    Err(OwnershipFailure::Domain(if definite_absence { OwnershipError::ParentClosed } else { OwnershipError::ParentClosing }))
                );
                assert_eq!(factory.prepares(), 1);
                assert_eq!(factory.submits(), usize::from(!factory_failure));
                if cleanup || !factory_failure {
                    assert_eq!(
                        c.bind_resources(
                            child.clone(),
                            Arc::new(ScriptResources {
                                closes: AtomicUsize::new(0),
                                report: released(),
                                hold: None
                            })
                        )
                        .unwrap_err()
                        .reason,
                        BindResourcesRefusal::AlreadyBound
                    );
                } else if definite_absence {
                    let refusal = c.bind_resources(child.clone(), Arc::new(ScriptResources {
                        closes: AtomicUsize::new(0), report: released(), hold: None,
                    })).unwrap_err();
                    assert_eq!(refusal.reason, BindResourcesRefusal::Closed);
                } else {
                    c.bind_resources(
                        child,
                        Arc::new(ScriptResources {
                            closes: AtomicUsize::new(0),
                            report: released(),
                            hold: None,
                        }),
                    )
                    .unwrap();
                }
                assert_eq!(
                    owner.closes.load(Ordering::SeqCst),
                    usize::from(cleanup && failure == PortFailure::Rejected)
                );
                let mut available = 0;
                while room.try_reserve() {
                    available += 1;
                }
                assert_eq!(
                    available,
                    if factory_failure && !cleanup && failure == PortFailure::Rejected {
                        8
                    } else {
                        7
                    }
                );
                for _ in 0..available {
                    room.release();
                }
            }
        }
    }
}

#[tokio::test]
async fn row_38_known_failure_revokes_before_failed_fallback_audit_and_store_awaits() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let audit = IndependentAudit::new();
    let factory = ScriptFactory::new();
    let c = coordinator(
        store.clone(),
        audit.clone(),
        factory.clone(),
        Arc::new(LiveCapacity::new(8)),
    );
    let root = open(&c, "fallback").await.unwrap();
    let (main, main_release) = audit.gate(OwnershipMeaning::Prepared);
    let spawning = tokio::spawn({
        let c = c.clone();
        let root = root.clone();
        async move { c.spawn(command(&root)).await }
    });
    bounded(main.entered.notified()).await;
    let child = factory.last_child.lock().unwrap().clone().unwrap();
    let (fallback, fallback_release) = audit.gate(OwnershipMeaning::Unconfirmed);
    main_release.send(Err(PortFailure::Rejected)).unwrap();
    bounded(fallback.entered.notified()).await;
    assert_eq!(c.lifetime_state(&child), Some(LifetimeState::Closing));
    assert!(factory
        .last_gate
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .is_sealed());
    let mut descendant = command(&child);
    descendant.request_id = SpawnRequestId::new("fallback-descendant").unwrap();
    assert_eq!(
        c.spawn(descendant).await,
        Err(OwnershipFailure::Domain(OwnershipError::ParentClosing))
    );
    open(&c, "neighbor").await.unwrap();
    assert_eq!(
        store.read().await.unwrap().spawns[0].progress.known(),
        KnownMilestone::Prepared
    );
    store.fail_next_write(PortFailure::Rejected);
    fallback_release.send(Err(PortFailure::Uncertain)).unwrap();
    assert_eq!(
        bounded(spawning).await.unwrap(),
        Err(OwnershipFailure::Audit(PortFailure::Rejected))
    );
    assert_eq!(factory.prepares(), 1);
    assert_eq!(factory.submits(), 0);
    assert_eq!(
        factory.children.lock().unwrap()[0]
            .closes
            .load(Ordering::SeqCst),
        0
    );
    let records = audit.records.lock().unwrap();
    let closes: Vec<_> = records
        .iter()
        .filter(|r| r.parent_lifetime == child && r.after == OwnershipMeaning::Closing)
        .collect();
    // The inner failure owns the first close; its outer fallback only retries
    // the shared writer rather than inventing another first-close audit.
    assert_eq!(closes.len(), 1);
    assert_eq!(closes[0].cause, Some(LifetimeCause::TerminalFailure));
}

#[tokio::test]
async fn row_38_conflict_and_pre_admission_failures_do_not_revoke_another_admitted_child() {
    let factory = ScriptFactory::new();
    let release = Arc::new(Notify::new());
    *factory.release.lock().unwrap() = Some(release.clone());
    let c = coordinator(
        Arc::new(MemoryOwnershipStore::new()),
        IndependentAudit::new(),
        factory.clone(),
        Arc::new(LiveCapacity::new(8)),
    );
    let root = open(&c, "owner").await.unwrap();
    let original = command(&root);
    let spawning = tokio::spawn({
        let c = c.clone();
        let original = original.clone();
        async move { c.spawn(original).await }
    });
    bounded(factory.entered.notified()).await;
    let child = factory.last_child.lock().unwrap().clone().unwrap();
    let actual = factory.last_gate.lock().unwrap().clone().unwrap();
    let mut conflict = original.clone();
    conflict.task = "different".to_owned();
    let conflicting = tokio::spawn({
        let c = c.clone();
        async move { c.spawn(conflict).await }
    });
    let mut refused = original.clone();
    refused.request_id = SpawnRequestId::new("policy-refused").unwrap();
    refused.policy = PolicyRead::ParentUnavailable;
    assert_eq!(
        c.spawn(refused).await,
        Err(OwnershipFailure::Domain(OwnershipError::ParentClosing))
    );
    assert_eq!(c.lifetime_state(&child), Some(LifetimeState::Open));
    assert!(!actual.is_sealed());
    release.notify_one();
    let result = bounded(spawning).await.unwrap().unwrap();
    assert_eq!(
        bounded(conflicting).await.unwrap(),
        Err(OwnershipFailure::Domain(OwnershipError::RequestConflict))
    );
    assert_eq!(c.spawn(original).await.unwrap(), result);
    assert_eq!(factory.prepares(), 1);
    assert_eq!(factory.submits(), 1);
    assert_eq!(c.lifetime_state(&child), Some(LifetimeState::Open));
    assert!(!actual.is_sealed());
}

#[tokio::test]
async fn row_38_uncertain_submission_revokes_before_held_fallback_and_retains_owner() {
    let audit = IndependentAudit::new();
    let factory = ScriptFactory::new();
    *factory.submit_result.lock().unwrap() = Err(PortFailure::Uncertain);
    let (fallback, release) = audit.gate(OwnershipMeaning::Unconfirmed);
    let c = coordinator(
        Arc::new(MemoryOwnershipStore::new()),
        audit,
        factory.clone(),
        Arc::new(LiveCapacity::new(8)),
    );
    let root = open(&c, "uncertain-submit").await.unwrap();
    let spawning = tokio::spawn({
        let c = c.clone();
        let root = root.clone();
        async move { c.spawn(command(&root)).await }
    });
    bounded(fallback.entered.notified()).await;
    let child = factory.last_child.lock().unwrap().clone().unwrap();
    assert_eq!(c.lifetime_state(&child), Some(LifetimeState::Closing));
    assert!(factory
        .last_gate
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .is_sealed());
    let mut descendant = command(&child);
    descendant.request_id = SpawnRequestId::new("submit-descendant").unwrap();
    assert_eq!(
        c.spawn(descendant).await,
        Err(OwnershipFailure::Domain(OwnershipError::ParentClosing))
    );
    assert_eq!(
        factory.children.lock().unwrap()[0]
            .closes
            .load(Ordering::SeqCst),
        0
    );
    release.send(Err(PortFailure::Rejected)).unwrap();
    assert_eq!(
        bounded(spawning).await.unwrap(),
        Err(OwnershipFailure::Submission(PortFailure::Uncertain))
    );
    assert_eq!(factory.prepares(), 1);
    assert_eq!(factory.submits(), 1);
}
