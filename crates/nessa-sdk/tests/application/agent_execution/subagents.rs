//! Ownership coordinator regressions for ADR 329 rows S1–S7, S11, S13, C1, C4–C8, C11, C12, C14, and R1–R5.
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use nessa_sdk::application::agent_execution::agents::{lifetime_disposition, LifetimeDisposition};
use nessa_sdk::application::agent_execution::providers::SessionCloseRequest;
use nessa_sdk::application::agent_execution::subagents::{
    ChildFactory, ChildResources, CloseCommand, InitialSubmit, LiveCapacity, MemoryOwnershipStore,
    OwnershipAudit, OwnershipCoordinator, OwnershipDependencies, OwnershipFailure, OwnershipStore,
    PortFailure, PrepareFailure, PrepareRequest, PreparedChild, ResourceReport, SpawnCommand,
};
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::domain::agent_execution::subagents::{
    AgentLifetimeId, ApprovalPolicy, DeliveryState, EvidenceFact, HostActor, Initiator,
    LifetimeCause, LifetimeRow, LifetimeState, OwnershipError, OwnershipEvidence, OwnershipGraph,
    OwnershipSnapshot, PhysicalFact, PolicyRead, ReportId, SpawnAdmission, SpawnBinding,
    SpawnOrigin, SpawnProgress, SpawnRequestId, SpawnRow, TaskDigest, TaskReceiptId,
};
use tokio::sync::Notify;

struct ScriptAudit {
    fail: Mutex<Option<PortFailure>>,
    records: Mutex<Vec<OwnershipEvidence>>,
}

impl ScriptAudit {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            fail: Mutex::new(None),
            records: Mutex::new(Vec::new()),
        })
    }

    fn fail_next(&self, failure: PortFailure) {
        *self.fail.lock().expect("audit") = Some(failure);
    }
}

#[async_trait]
impl OwnershipAudit for ScriptAudit {
    async fn record(&self, evidence: &OwnershipEvidence) -> Result<(), PortFailure> {
        if let Some(failure) = self.fail.lock().expect("audit").take() {
            return Err(failure);
        }
        self.records.lock().expect("audit").push(evidence.clone());
        Ok(())
    }
}

struct ScriptResources {
    closes: AtomicUsize,
    report: ResourceReport,
    hold: Option<Arc<Notify>>,
}

#[async_trait]
impl ChildResources for ScriptResources {
    async fn close(&self, _cause: &LifetimeCause, _initiator: &Initiator) -> ResourceReport {
        self.closes.fetch_add(1, Ordering::SeqCst);
        if let Some(hold) = &self.hold {
            hold.notified().await;
        }
        self.report
    }
}

struct ScriptSubmit {
    submits: Arc<AtomicUsize>,
    result: Result<TaskReceiptId, PortFailure>,
}

#[async_trait]
impl InitialSubmit for ScriptSubmit {
    async fn submit(
        &self,
        _task: &str,
        _request: &SpawnRequestId,
    ) -> Result<TaskReceiptId, PortFailure> {
        self.submits.fetch_add(1, Ordering::SeqCst);
        self.result.clone()
    }
}

struct ScriptFactory {
    prepares: AtomicUsize,
    submits: Arc<AtomicUsize>,
    entered: Notify,
    release: Mutex<Option<Arc<Notify>>>,
    reports: Mutex<VecDeque<ResourceReport>>,
    fail: Mutex<Option<PrepareFailure>>,
    children: Mutex<Vec<Arc<ScriptResources>>>,
    hold_next: Mutex<Option<Arc<Notify>>>,
    submit_result: Mutex<Result<TaskReceiptId, PortFailure>>,
}

impl ScriptFactory {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            prepares: AtomicUsize::new(0),
            submits: Arc::new(AtomicUsize::new(0)),
            entered: Notify::new(),
            release: Mutex::new(None),
            reports: Mutex::new(VecDeque::new()),
            fail: Mutex::new(None),
            children: Mutex::new(Vec::new()),
            hold_next: Mutex::new(None),
            submit_result: Mutex::new(Ok(TaskReceiptId::new("receipt-1").unwrap())),
        })
    }

    fn prepares(&self) -> usize {
        self.prepares.load(Ordering::SeqCst)
    }

    fn submits(&self) -> usize {
        self.submits.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ChildFactory for ScriptFactory {
    async fn prepare(&self, _request: PrepareRequest) -> Result<PreparedChild, PrepareFailure> {
        self.prepares.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        let release = self.release.lock().expect("factory").clone();
        if let Some(release) = release {
            release.notified().await;
        }
        if let Some(failure) = self.fail.lock().expect("factory").take() {
            return Err(failure);
        }
        let report = self
            .reports
            .lock()
            .expect("factory")
            .pop_front()
            .unwrap_or(ResourceReport {
                physical: PhysicalFact::Released,
                evidence: EvidenceFact::Acknowledged,
            });
        let resources = Arc::new(ScriptResources {
            closes: AtomicUsize::new(0),
            report,
            hold: self.hold_next.lock().expect("factory").take(),
        });
        self.children
            .lock()
            .expect("factory")
            .push(Arc::clone(&resources));
        Ok(PreparedChild {
            resources,
            submit: Arc::new(ScriptSubmit {
                submits: Arc::clone(&self.submits),
                result: self.submit_result.lock().expect("submit").clone(),
            }),
        })
    }
}

struct World {
    coordinator: OwnershipCoordinator,
    factory: Arc<ScriptFactory>,
    audit: Arc<ScriptAudit>,
}

impl World {
    fn new(slots: usize) -> Self {
        let factory = ScriptFactory::new();
        let audit = ScriptAudit::new();
        let store = Arc::new(MemoryOwnershipStore::new());
        let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
            store: Arc::clone(&store)
                as Arc<dyn nessa_sdk::application::agent_execution::subagents::OwnershipStore>,
            audit: Arc::clone(&audit) as Arc<dyn OwnershipAudit>,
            factory: Arc::clone(&factory) as Arc<dyn ChildFactory>,
            room: Arc::new(LiveCapacity::new(slots)),
        });
        Self {
            coordinator,
            factory,
            audit,
        }
    }

    async fn root(&self) -> AgentLifetimeId {
        self.coordinator
            .open_root(
                SessionId::new("root-session").unwrap(),
                Initiator::Host(actor("open")),
            )
            .await
            .unwrap()
    }

    fn bind_root(&self, lifetime: &AgentLifetimeId) {
        self.coordinator.bind_resources(
            lifetime.clone(),
            Arc::new(ScriptResources {
                closes: AtomicUsize::new(0),
                report: ResourceReport {
                    physical: PhysicalFact::Released,
                    evidence: EvidenceFact::Acknowledged,
                },
                hold: None,
            }),
        );
    }

    fn command(&self, parent: &AgentLifetimeId, request: &str, task: &str) -> SpawnCommand {
        SpawnCommand {
            parent: parent.clone(),
            request_id: SpawnRequestId::new(request).unwrap(),
            task: task.to_owned(),
            policy: PolicyRead::Committed(
                ApprovalPolicy::new("read-only", "ask", "rev-1").unwrap(),
            ),
            child_supports_policy: true,
            model: None,
            origin: SpawnOrigin::Host(actor(request)),
        }
    }

    async fn close(&self, lifetime: &AgentLifetimeId) -> Result<(), OwnershipFailure> {
        self.coordinator
            .end_lifetime(CloseCommand {
                lifetime: lifetime.clone(),
                cause: LifetimeCause::HostClose,
                initiator: Initiator::Host(actor("close")),
                external_attachment: false,
                timeout: None,
            })
            .await
    }
}

fn actor(request: &str) -> HostActor {
    HostActor::new("person", "desktop", request).unwrap()
}

fn released() -> ResourceReport {
    ResourceReport {
        physical: PhysicalFact::Released,
        evidence: EvidenceFact::Acknowledged,
    }
}

#[test]
fn attachment_stop_classification_keeps_recoverable_closes_attachment_only() {
    assert_eq!(
        lifetime_disposition(&SessionCloseRequest::SessionHandlesDropped),
        LifetimeDisposition::EndOwnedLifetime
    );
    for request in [
        SessionCloseRequest::Explicit(
            nessa_sdk::application::agent_execution::permissions::ActionContext::new(
                "person", "desktop", "close",
            )
            .unwrap(),
        ),
        SessionCloseRequest::ExecutionFailed,
        SessionCloseRequest::SessionFailed,
        SessionCloseRequest::DeadlineExceeded,
        SessionCloseRequest::EventConsumerDropped,
    ] {
        assert_eq!(
            lifetime_disposition(&request),
            LifetimeDisposition::AttachmentOnly
        );
    }
}

#[tokio::test]
async fn rejected_root_publication_can_be_opened_again() {
    let world = World::new(8);
    let neighbor = world
        .coordinator
        .open_root(
            SessionId::new("neighbor-session").unwrap(),
            Initiator::Host(actor("neighbor")),
        )
        .await
        .unwrap();
    world.audit.fail_next(PortFailure::Rejected);
    let rejected = world
        .coordinator
        .open_root(
            SessionId::new("root-session").unwrap(),
            Initiator::Host(actor("open")),
        )
        .await;
    assert!(matches!(
        rejected,
        Err(OwnershipFailure::Audit(PortFailure::Rejected))
    ));
    assert_eq!(
        world.coordinator.lifetime_state(&neighbor),
        Some(LifetimeState::Open)
    );
    let root = world.root().await;
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Open)
    );
    assert_ne!(root, neighbor);
}

#[tokio::test]
async fn s1_spawn_binds_once_and_admits_the_initial_task() {
    let world = World::new(8);
    let root = world.root().await;
    let receipt = world
        .coordinator
        .spawn(world.command(&root, "req-1", "draft the note"))
        .await
        .unwrap();
    assert!(matches!(
        receipt.progress,
        SpawnProgress::TaskAdmitted { .. }
    ));
    assert_eq!(receipt.policy.mode(), "read-only");
    assert_eq!(world.factory.prepares(), 1);
    assert_eq!(world.factory.submits(), 1);
    let again = world
        .coordinator
        .spawn(world.command(&root, "req-1", "draft the note"))
        .await
        .unwrap();
    assert_eq!(again.child, receipt.child);
    assert_eq!(world.factory.prepares(), 1);
    assert_eq!(world.factory.submits(), 1);
}

#[tokio::test]
async fn s3_changed_task_conflicts_and_keeps_the_original_child() {
    let world = World::new(8);
    let root = world.root().await;
    let original = world
        .coordinator
        .spawn(world.command(&root, "req-1", "draft the note"))
        .await
        .unwrap();
    let conflict = world
        .coordinator
        .spawn(world.command(&root, "req-1", "a different task"))
        .await;
    assert!(matches!(
        conflict,
        Err(OwnershipFailure::Domain(OwnershipError::RequestConflict))
    ));
    assert_eq!(
        world
            .coordinator
            .spawn_progress(&SpawnRequestId::new("req-1").unwrap()),
        Some(original.progress)
    );
    assert_eq!(world.factory.prepares(), 1);
}

#[tokio::test]
async fn s4_close_before_spawn_does_not_prepare_a_child() {
    let world = World::new(8);
    let root = world.root().await;
    world.bind_root(&root);
    world.close(&root).await.unwrap();
    let refused = world
        .coordinator
        .spawn(world.command(&root, "req-1", "draft the note"))
        .await;
    assert!(matches!(
        refused,
        Err(OwnershipFailure::Domain(OwnershipError::ParentClosed))
    ));
    assert_eq!(world.factory.prepares(), 0);
}

#[tokio::test]
async fn s5_close_during_factory_does_not_submit() {
    let world = World::new(8);
    let root = world.root().await;
    world.bind_root(&root);
    let release = Arc::new(Notify::new());
    *world.factory.release.lock().expect("factory") = Some(Arc::clone(&release));
    let coordinator = world.coordinator.clone();
    let command = world.command(&root, "req-1", "draft the note");
    let spawning = tokio::spawn(async move { coordinator.spawn(command).await });
    world.factory.entered.notified().await;
    let coordinator = world.coordinator.clone();
    let closing = tokio::spawn(async move {
        coordinator
            .end_lifetime(CloseCommand {
                lifetime: root,
                cause: LifetimeCause::HostClose,
                initiator: Initiator::Host(actor("close")),
                external_attachment: false,
                timeout: None,
            })
            .await
    });
    release.notify_one();
    let spawned = spawning.await.unwrap();
    assert!(matches!(
        spawned,
        Err(OwnershipFailure::Domain(OwnershipError::ParentClosing))
    ));
    closing.await.unwrap().unwrap();
    assert_eq!(world.factory.prepares(), 1);
    assert_eq!(world.factory.submits(), 0);
}

#[tokio::test]
async fn s6_startup_failure_keeps_the_cleanup_owner() {
    let world = World::new(8);
    let root = world.root().await;
    let cleanup = Arc::new(ScriptResources {
        closes: AtomicUsize::new(0),
        report: released(),
        hold: None,
    });
    *world.factory.fail.lock().expect("factory") = Some(PrepareFailure {
        failure: PortFailure::Rejected,
        cleanup: Some(cleanup.clone()),
    });
    let error = world
        .coordinator
        .spawn(world.command(&root, "req-1", "draft the note"))
        .await;
    assert!(matches!(
        error,
        Err(OwnershipFailure::Startup(PortFailure::Rejected))
    ));
    assert_eq!(cleanup.closes.load(Ordering::SeqCst), 1);
    assert!(matches!(
        world
            .coordinator
            .spawn_progress(&SpawnRequestId::new("req-1").unwrap()),
        Some(SpawnProgress::Ended { .. })
    ));
    assert_eq!(world.factory.submits(), 0);
}

#[tokio::test]
async fn s7_dropped_waiter_keeps_the_attempt() {
    let world = World::new(8);
    let root = world.root().await;
    let release = Arc::new(Notify::new());
    *world.factory.release.lock().expect("factory") = Some(Arc::clone(&release));
    let coordinator = world.coordinator.clone();
    let command = world.command(&root, "req-1", "draft the note");
    let waiting = tokio::spawn(async move { coordinator.spawn(command).await });
    world.factory.entered.notified().await;
    waiting.abort();
    release.notify_one();
    let found = world
        .coordinator
        .spawn(world.command(&root, "req-1", "draft the note"))
        .await
        .unwrap();
    assert!(matches!(found.progress, SpawnProgress::TaskAdmitted { .. }));
    assert_eq!(world.factory.prepares(), 1);
    assert_eq!(world.factory.submits(), 1);
}

#[tokio::test]
async fn rejected_submission_stays_attached_and_is_not_submitted_again() {
    let world = World::new(8);
    let root = world.root().await;
    *world.factory.submit_result.lock().expect("submit") = Err(PortFailure::Rejected);
    let rejected = world
        .coordinator
        .spawn(world.command(&root, "req-1", "draft the note"))
        .await;
    assert!(matches!(
        rejected,
        Err(OwnershipFailure::Submission(PortFailure::Rejected))
    ));
    assert!(matches!(
        world
            .coordinator
            .spawn_progress(&SpawnRequestId::new("req-1").unwrap()),
        Some(SpawnProgress::Attached)
    ));
    let again = world
        .coordinator
        .spawn(world.command(&root, "req-1", "draft the note"))
        .await
        .unwrap();
    assert!(matches!(again.progress, SpawnProgress::Attached));
    assert_eq!(world.factory.prepares(), 1);
    assert_eq!(world.factory.submits(), 1);
}

#[tokio::test]
async fn host_gate_closes_the_child_and_waits_for_the_root_attachment() {
    let world = World::new(8);
    let root = world.root().await;
    let child = world
        .coordinator
        .spawn(world.command(&root, "child", "task"))
        .await
        .unwrap();
    let gate = world.coordinator.participation(&root).expect("root gate");
    let actor = nessa_sdk::application::agent_execution::permissions::ActionContext::new(
        "person", "desktop", "close",
    )
    .unwrap();
    gate.seal_for_host(&actor).await.unwrap();
    assert!(gate.is_sealed());
    gate.note_attachment(true, true).await;
    gate.join_descendants().await.unwrap();
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        world.coordinator.lifetime_state(&child.child),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        world.factory.children.lock().expect("children")[0]
            .closes
            .load(Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn s11_room_child_and_depth_bounds_refuse_the_extra_spawn() {
    let world = World::new(1);
    let root = world.root().await;
    world
        .coordinator
        .spawn(world.command(&root, "req-1", "one"))
        .await
        .unwrap();
    let no_room = world
        .coordinator
        .spawn(world.command(&root, "req-2", "two"))
        .await;
    assert!(matches!(
        no_room,
        Err(OwnershipFailure::Domain(OwnershipError::NoRoom))
    ));
    assert_eq!(world.factory.prepares(), 1);

    let world = World::new(64);
    let mut parent = world.root().await;
    for index in 0..4 {
        let receipt = world
            .coordinator
            .spawn(world.command(&parent, &format!("depth-{index}"), "deeper"))
            .await
            .unwrap();
        parent = receipt.child;
    }
    let too_deep = world
        .coordinator
        .spawn(world.command(&parent, "depth-4", "too deep"))
        .await;
    assert!(matches!(
        too_deep,
        Err(OwnershipFailure::Domain(OwnershipError::DepthExceeded))
    ));

    let world = World::new(64);
    let root = world.root().await;
    for index in 0..16 {
        world
            .coordinator
            .spawn(world.command(&root, &format!("child-{index}"), "sibling"))
            .await
            .unwrap();
    }
    let extra = world
        .coordinator
        .spawn(world.command(&root, "child-16", "one more"))
        .await;
    assert!(matches!(
        extra,
        Err(OwnershipFailure::Domain(
            OwnershipError::DirectChildrenExceeded
        ))
    ));
}

#[tokio::test]
async fn s13_rejected_and_uncertain_publication_does_not_prepare() {
    let world = World::new(8);
    let root = world.root().await;
    world.audit.fail_next(PortFailure::Rejected);
    let rejected = world
        .coordinator
        .spawn(world.command(&root, "req-reject", "draft"))
        .await;
    assert!(matches!(
        rejected,
        Err(OwnershipFailure::Audit(PortFailure::Rejected))
    ));
    assert_eq!(world.factory.prepares(), 0);
    let retained = world
        .coordinator
        .spawn(world.command(&root, "req-reject", "draft"))
        .await
        .unwrap();
    assert!(matches!(retained.progress, SpawnProgress::Reserved));
    assert_eq!(world.factory.prepares(), 0);

    world.audit.fail_next(PortFailure::Uncertain);
    let uncertain = world
        .coordinator
        .spawn(world.command(&root, "req-uncertain", "draft"))
        .await;
    assert!(matches!(
        uncertain,
        Err(OwnershipFailure::Audit(PortFailure::Uncertain))
    ));
    assert_eq!(world.factory.prepares(), 0);
    assert!(matches!(
        world
            .coordinator
            .spawn_progress(&SpawnRequestId::new("req-uncertain").unwrap()),
        Some(SpawnProgress::Unconfirmed { .. })
    ));
}

#[tokio::test]
async fn c1_parent_close_joins_siblings_and_a_grandchild() {
    let world = World::new(8);
    let root = world.root().await;
    world.bind_root(&root);
    let left = world
        .coordinator
        .spawn(world.command(&root, "left", "left task"))
        .await
        .unwrap();
    let right = world
        .coordinator
        .spawn(world.command(&root, "right", "right task"))
        .await
        .unwrap();
    let grandchild = world
        .coordinator
        .spawn(world.command(&left.child, "grand", "grand task"))
        .await
        .unwrap();
    world.close(&root).await.unwrap();
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        world.coordinator.lifetime_state(&left.child),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        world.coordinator.lifetime_state(&right.child),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        world.coordinator.lifetime_state(&grandchild.child),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        world.coordinator.close_cause(&grandchild.child),
        Some(LifetimeCause::HostClose)
    );
    assert_eq!(
        world.coordinator.cascaded_from(&grandchild.child),
        Some(left.child.clone())
    );
    assert_eq!(world.coordinator.cascaded_from(&root), None);
    for child in world.factory.children.lock().expect("children").iter() {
        assert_eq!(child.closes.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn c4_one_cleanup_failure_still_closes_the_sibling() {
    let world = World::new(8);
    let root = world.root().await;
    world.bind_root(&root);
    world.factory.reports.lock().expect("reports").extend([
        ResourceReport {
            physical: PhysicalFact::Failed,
            evidence: EvidenceFact::Failed,
        },
        released(),
    ]);
    world
        .coordinator
        .spawn(world.command(&root, "left", "left"))
        .await
        .unwrap();
    world
        .coordinator
        .spawn(world.command(&root, "right", "right"))
        .await
        .unwrap();
    let closed = world.close(&root).await;
    assert!(closed.is_err());
    let children = world.factory.children.lock().expect("children");
    assert_eq!(children.len(), 2);
    assert_eq!(children[0].closes.load(Ordering::SeqCst), 1);
    assert_eq!(children[1].closes.load(Ordering::SeqCst), 1);
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Closing)
    );
}

#[tokio::test]
async fn c5_released_process_with_failed_audit_is_not_an_audited_close() {
    let world = World::new(8);
    let root = world.root().await;
    world.bind_root(&root);
    world
        .factory
        .reports
        .lock()
        .expect("reports")
        .push_back(ResourceReport {
            physical: PhysicalFact::Released,
            evidence: EvidenceFact::Failed,
        });
    world
        .coordinator
        .spawn(world.command(&root, "child", "task"))
        .await
        .unwrap();
    assert!(world.close(&root).await.is_err());
    assert_eq!(
        world.factory.children.lock().expect("children")[0]
            .closes
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Closing)
    );
}

#[tokio::test]
async fn c6_timeout_leaves_the_drain_running() {
    let world = World::new(8);
    let root = world.root().await;
    world.bind_root(&root);
    let hold = Arc::new(Notify::new());
    *world.factory.hold_next.lock().expect("hold") = Some(Arc::clone(&hold));
    world
        .coordinator
        .spawn(world.command(&root, "child", "task"))
        .await
        .unwrap();
    let incomplete = world
        .coordinator
        .end_lifetime(CloseCommand {
            lifetime: root.clone(),
            cause: LifetimeCause::HostClose,
            initiator: Initiator::Host(actor("close")),
            external_attachment: false,
            timeout: Some(Duration::from_millis(20)),
        })
        .await;
    assert!(matches!(incomplete, Err(OwnershipFailure::Incomplete)));
    hold.notify_one();
    world.close(&root).await.unwrap();
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Closed)
    );
}

#[tokio::test]
async fn c7_second_close_joins_the_first_cause() {
    let world = World::new(8);
    let root = world.root().await;
    world.bind_root(&root);
    let hold = Arc::new(Notify::new());
    *world.factory.hold_next.lock().expect("hold") = Some(Arc::clone(&hold));
    let child = world
        .coordinator
        .spawn(world.command(&root, "child", "task"))
        .await
        .unwrap();
    let first = world.coordinator.clone();
    let root_for_first = root.clone();
    let closing = tokio::spawn(async move {
        first
            .end_lifetime(CloseCommand {
                lifetime: root_for_first,
                cause: LifetimeCause::HostClose,
                initiator: Initiator::Host(actor("first")),
                external_attachment: false,
                timeout: None,
            })
            .await
    });
    while world.coordinator.close_cause(&root).is_none() {
        tokio::task::yield_now().await;
    }
    let second = world
        .coordinator
        .end_lifetime(CloseCommand {
            lifetime: root.clone(),
            cause: LifetimeCause::Deletion,
            initiator: Initiator::Host(actor("second")),
            external_attachment: false,
            timeout: Some(Duration::from_millis(20)),
        })
        .await;
    assert!(matches!(second, Err(OwnershipFailure::Incomplete)) || second.is_ok());
    assert_eq!(
        world.coordinator.close_cause(&root),
        Some(LifetimeCause::HostClose)
    );
    hold.notify_one();
    closing.await.unwrap().unwrap();
    assert_eq!(
        world.factory.children.lock().expect("children")[0]
            .closes
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        world.coordinator.close_cause(&child.child),
        Some(LifetimeCause::HostClose)
    );
}

#[tokio::test]
async fn c8_direct_child_close_leaves_the_parent_and_sibling_open() {
    let world = World::new(8);
    let root = world.root().await;
    let left = world
        .coordinator
        .spawn(world.command(&root, "left", "left"))
        .await
        .unwrap();
    let right = world
        .coordinator
        .spawn(world.command(&root, "right", "right"))
        .await
        .unwrap();
    world.close(&left.child).await.unwrap();
    assert_eq!(
        world.coordinator.lifetime_state(&left.child),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Open)
    );
    assert_eq!(
        world.coordinator.lifetime_state(&right.child),
        Some(LifetimeState::Open)
    );
}

#[tokio::test]
async fn c11_spawn_after_the_ancestor_fence_does_not_prepare() {
    let world = World::new(8);
    let root = world.root().await;
    world.bind_root(&root);
    let child = world
        .coordinator
        .spawn(world.command(&root, "child", "task"))
        .await
        .unwrap();
    world.close(&root).await.unwrap();
    let refused = world
        .coordinator
        .spawn(world.command(&child.child, "nested", "later"))
        .await;
    assert!(matches!(
        refused,
        Err(OwnershipFailure::Domain(
            OwnershipError::ParentClosed | OwnershipError::ParentClosing
        ))
    ));
    assert_eq!(world.factory.prepares(), 1);
}

#[tokio::test]
async fn c12_a_result_after_close_is_suppressed() {
    let world = World::new(8);
    let root = world.root().await;
    world.bind_root(&root);
    let child = world
        .coordinator
        .spawn(world.command(&root, "child", "task"))
        .await
        .unwrap();
    let submitted = world
        .coordinator
        .deliver_report(ReportId::new("report-1").unwrap(), &child.child, &root)
        .await
        .unwrap();
    assert_eq!(submitted, DeliveryState::Submitted);
    world.close(&root).await.unwrap();
    let suppressed = world
        .coordinator
        .deliver_report(ReportId::new("report-2").unwrap(), &child.child, &root)
        .await
        .unwrap();
    assert_eq!(suppressed, DeliveryState::Suppressed);
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Closed)
    );
}

#[tokio::test]
async fn c14_close_audit_failure_still_cleans_up() {
    let world = World::new(8);
    let root = world.root().await;
    world.bind_root(&root);
    world
        .coordinator
        .spawn(world.command(&root, "child", "task"))
        .await
        .unwrap();
    world.audit.fail_next(PortFailure::Rejected);
    let closed = world.close(&root).await;
    assert!(matches!(
        closed,
        Err(OwnershipFailure::Audit(PortFailure::Rejected))
    ));
    assert_eq!(
        world.factory.children.lock().expect("children")[0]
            .closes
            .load(Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn r1_r2_resume_does_not_prepare_or_submit_again() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let mut graph = OwnershipGraph::new();
    let root = AgentLifetimeId::new("root-life").unwrap();
    let _evidence = graph
        .open_root(
            SessionId::new("root-session").unwrap(),
            root.clone(),
            Initiator::Runtime,
        )
        .unwrap();
    let reserved = SpawnRequestId::new("reserved-req").unwrap();
    let _evidence = graph
        .admit_spawn(SpawnAdmission {
            child_lifetime: AgentLifetimeId::new("reserved-child").unwrap(),
            child_session: SessionId::new("reserved-session").unwrap(),
            binding: binding(&root, &reserved, "reserved-task"),
            live_room: true,
        })
        .unwrap();
    let admitted = SpawnRequestId::new("admitted-req").unwrap();
    let _evidence = graph
        .admit_spawn(SpawnAdmission {
            child_lifetime: AgentLifetimeId::new("admitted-child").unwrap(),
            child_session: SessionId::new("admitted-session").unwrap(),
            binding: binding(&root, &admitted, "admitted-task"),
            live_room: true,
        })
        .unwrap();
    let _evidence = graph
        .advance_spawn(&admitted, SpawnProgress::Prepared)
        .unwrap();
    let _evidence = graph
        .advance_spawn(&admitted, SpawnProgress::Attached)
        .unwrap();
    let _evidence = graph
        .advance_spawn(
            &admitted,
            SpawnProgress::TaskAdmitted {
                receipt: TaskReceiptId::new("receipt-kept").unwrap(),
            },
        )
        .unwrap();
    store.write(&graph.snapshot()).await.unwrap();
    let world = resumed(store);
    world.coordinator.resume().await.unwrap();
    assert!(matches!(
        world.coordinator.spawn_progress(&reserved),
        Some(SpawnProgress::Unconfirmed { .. })
    ));
    assert!(matches!(
        world.coordinator.spawn_progress(&admitted),
        Some(SpawnProgress::TaskAdmitted { .. })
    ));
    assert_eq!(world.factory.prepares(), 0);
    assert_eq!(world.factory.submits(), 0);
}

#[tokio::test]
async fn r3_resume_of_a_closing_tree_does_not_dispatch() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let mut graph = OwnershipGraph::new();
    let root = AgentLifetimeId::new("root-life").unwrap();
    let _evidence = graph
        .open_root(
            SessionId::new("root-session").unwrap(),
            root.clone(),
            Initiator::Runtime,
        )
        .unwrap();
    let _evidence = graph
        .begin_close(
            &root,
            nessa_sdk::domain::agent_execution::subagents::CloseOperationId::new("close-1")
                .unwrap(),
            LifetimeCause::HostClose,
            Initiator::Host(actor("close")),
        )
        .unwrap();
    store.write(&graph.snapshot()).await.unwrap();
    let world = resumed(store);
    world.coordinator.resume().await.unwrap();
    assert_eq!(world.factory.prepares(), 0);
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Closing)
    );
}

#[tokio::test]
async fn r4_reopen_mints_a_new_lifetime_and_does_not_adopt_children() {
    let world = World::new(8);
    let root = world.root().await;
    world.bind_root(&root);
    let child = world
        .coordinator
        .spawn(world.command(&root, "child", "task"))
        .await
        .unwrap();
    world.close(&root).await.unwrap();
    let reopened = world
        .coordinator
        .open_root(
            SessionId::new("root-session").unwrap(),
            Initiator::Host(actor("reopen")),
        )
        .await
        .unwrap();
    assert_ne!(reopened, root);
    let page = world.coordinator.children(&reopened, None, 10).unwrap();
    assert!(page.children.is_empty());
    let historical = world.coordinator.children(&root, None, 10).unwrap();
    assert_eq!(historical.children[0].lifetime, child.child);
    assert_eq!(historical.children[0].state, LifetimeState::Closed);
}

#[tokio::test]
async fn r5_a_cycle_stays_readable_and_refuses_dispatch() {
    let store = Arc::new(MemoryOwnershipStore::new());
    let left = AgentLifetimeId::new("life-left").unwrap();
    let right = AgentLifetimeId::new("life-right").unwrap();
    let snapshot = OwnershipSnapshot {
        lifetimes: vec![open_row(&left, "sess-left"), open_row(&right, "sess-right")],
        spawns: vec![
            spawn_row(&right, &left, "req-left"),
            spawn_row(&left, &right, "req-right"),
        ],
        settlements: Vec::new(),
        reports: Vec::new(),
    };
    store.write(&snapshot).await.unwrap();
    let world = resumed(store);
    world.coordinator.resume().await.unwrap();
    let refused = world
        .coordinator
        .spawn(world.command(&left, "req-new", "task"))
        .await;
    assert!(matches!(
        refused,
        Err(OwnershipFailure::Domain(
            OwnershipError::Cycle | OwnershipError::DispatchRefused
        ))
    ));
    let page = world.coordinator.children(&left, None, 10).unwrap();
    assert_eq!(page.children.len(), 1);
    assert_eq!(world.factory.prepares(), 0);
}

fn resumed(store: Arc<MemoryOwnershipStore>) -> World {
    let factory = ScriptFactory::new();
    let audit = ScriptAudit::new();
    let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: store.clone(),
        audit: Arc::clone(&audit) as Arc<dyn OwnershipAudit>,
        factory: Arc::clone(&factory) as Arc<dyn ChildFactory>,
        room: Arc::new(LiveCapacity::new(8)),
    });
    World {
        coordinator,
        factory,
        audit,
    }
}

fn binding(parent: &AgentLifetimeId, request: &SpawnRequestId, task: &str) -> SpawnBinding {
    SpawnBinding {
        parent_lifetime: parent.clone(),
        parent_session: SessionId::new("root-session").unwrap(),
        request_id: request.clone(),
        task_digest: TaskDigest::new(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap(),
        policy: ApprovalPolicy::new("read-only", "ask", "rev-1").unwrap(),
        model: None,
        origin: SpawnOrigin::Host(actor(task)),
    }
}

fn open_row(lifetime: &AgentLifetimeId, session: &str) -> LifetimeRow {
    LifetimeRow {
        lifetime_id: lifetime.clone(),
        session_id: SessionId::new(session).unwrap(),
        state: LifetimeState::Open,
        close_operation: None,
        cause: None,
        initiator: None,
        cascaded_from: None,
    }
}

fn spawn_row(child: &AgentLifetimeId, parent: &AgentLifetimeId, request: &str) -> SpawnRow {
    SpawnRow {
        child_lifetime: child.clone(),
        child_session: SessionId::new(request).unwrap(),
        binding: SpawnBinding {
            parent_lifetime: parent.clone(),
            parent_session: SessionId::new(if parent.as_str() == "life-left" {
                "sess-left"
            } else {
                "sess-right"
            })
            .unwrap(),
            request_id: SpawnRequestId::new(request).unwrap(),
            task_digest: TaskDigest::new(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            )
            .unwrap(),
            policy: ApprovalPolicy::new("read-only", "ask", "rev-1").unwrap(),
            model: None,
            origin: SpawnOrigin::Host(actor(request)),
        },
        progress: SpawnProgress::Reserved,
    }
}
