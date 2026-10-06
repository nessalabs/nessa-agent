//! Supervises spawn, tree close, and recovery around [`OwnershipGraph`](crate::domain::agent_execution::subagents::OwnershipGraph).
#![deny(missing_docs)]

use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
    time::Duration,
};

use async_trait::async_trait;
use data_encoding::HEXLOWER;
use sha2::{Digest, Sha256};
use tokio::sync::{watch, Notify};
use uuid::Uuid;

use super::{
    failure::OwnershipFailure,
    ports::{
        ChildFactory, ChildResources, LiveRoom, OwnershipAudit, OwnershipStore, PortFailure,
        PrepareRequest,
    },
};
use crate::application::agent_execution::{
    agents::{AgentError, OwnedLifetime},
    permissions::ActionContext,
};
use crate::domain::agent_execution::{
    sessions::SessionId,
    subagents::{
        select_inherited_policy, AgentLifetimeId, ApprovalPolicy, CloseOperationId, DeliveryState,
        EvidenceFact, HostActor, Initiator, LifetimeCause, LifetimeState, OwnershipError,
        OwnershipEvidence, OwnershipGraph, PhysicalFact, PolicyRead, ReportId, SpawnAdmission,
        SpawnBinding, SpawnOrigin, SpawnProgress, SpawnRequestId, TaskDigest, MAX_READ_PAGE,
    },
};

/// Injected effects for one coordinator. The graph stays inside the coordinator.
pub struct OwnershipDependencies {
    /// Acknowledged ownership rows.
    pub store: Arc<dyn OwnershipStore>,
    /// Mandatory audit of ownership evidence.
    pub audit: Arc<dyn OwnershipAudit>,
    /// Prepare-only child factory. It must not dispatch work.
    pub factory: Arc<dyn ChildFactory>,
    /// Live child capacity. The domain enforces tree bounds separately.
    pub room: Arc<dyn LiveRoom>,
}

/// One spawn attempt. The request id is the retry key.
#[derive(Clone)]
pub struct SpawnCommand {
    /// Parent lifetime that must be open.
    pub parent: AgentLifetimeId,
    /// Stable request id. An identical binding returns the original child.
    pub request_id: SpawnRequestId,
    /// Delegated task. The store keeps its digest, not this text.
    pub task: String,
    /// Policy read taken by the host before this call.
    pub policy: PolicyRead,
    /// Whether the child binding can honor the selected policy.
    pub child_supports_policy: bool,
    /// Optional catalog model. A retry that changes it conflicts.
    pub model: Option<crate::domain::agent_execution::subagents::ModelChoice>,
    /// Verified origin. A retry that changes it conflicts.
    pub origin: SpawnOrigin,
}

/// Child identity and how far its spawn chart has moved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnReceipt {
    /// Child lifetime.
    pub child: AgentLifetimeId,
    /// Child session.
    pub session: SessionId,
    /// Chart position after this call.
    pub progress: SpawnProgress,
    /// Policy copied at admission.
    pub policy: ApprovalPolicy,
}

/// Close one lifetime and the descendants sealed with it.
pub struct CloseCommand {
    /// Lifetime to seal. A repeat joins the first cause.
    pub lifetime: AgentLifetimeId,
    /// Cause recorded when this call is the first seal.
    pub cause: LifetimeCause,
    /// Initiator recorded when this call is the first seal.
    pub initiator: Initiator,
    /// When set, this lifetime's attachment is reported through [`OwnedLifetime::note_attachment`].
    pub external_attachment: bool,
    /// When set, return [`OwnershipFailure::Incomplete`] if the drain is still running.
    /// The drain keeps running.
    pub timeout: Option<Duration>,
}

/// Coordinates parent and child lifetimes for one ownership store.
#[derive(Clone)]
pub struct OwnershipCoordinator {
    inner: Arc<Shared>,
}

struct Shared {
    scope: Arc<Mutex<()>>,
    graph: Mutex<OwnershipGraph>,
    store: Arc<dyn OwnershipStore>,
    audit: Arc<dyn OwnershipAudit>,
    factory: Arc<dyn ChildFactory>,
    room: Arc<dyn LiveRoom>,
    resources: Mutex<HashMap<AgentLifetimeId, Arc<dyn ChildResources>>>,
    seals: Mutex<HashMap<AgentLifetimeId, Arc<AtomicBool>>>,
    gates: Mutex<HashMap<AgentLifetimeId, Arc<LifetimeGate>>>,
    inflight: Mutex<HashSet<AgentLifetimeId>>,
    held: Mutex<HashSet<AgentLifetimeId>>,
    claimed: Mutex<HashSet<AgentLifetimeId>>,
    flights: Mutex<HashMap<SpawnRequestId, SpawnFlight>>,
    drains: Mutex<HashMap<AgentLifetimeId, DrainSlot>>,
    notify: Notify,
}

type SpawnFlight = watch::Sender<Option<Result<SpawnReceipt, OwnershipFailure>>>;

struct DrainSlot {
    external: Arc<AtomicBool>,
    receiver: watch::Receiver<Option<Result<(), OwnershipFailure>>>,
}

struct LifetimeGate {
    inner: Weak<Shared>,
    lifetime: AgentLifetimeId,
    scope: Arc<Mutex<()>>,
    sealed: Arc<AtomicBool>,
}

struct InflightGuard {
    shared: Arc<Shared>,
    id: AgentLifetimeId,
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.shared
            .inflight
            .lock()
            .expect("inflight spawns")
            .remove(&self.id);
        self.shared.notify.notify_waiters();
    }
}

impl OwnershipCoordinator {
    /// Build a coordinator. It does not read the store until [`Self::resume`].
    pub fn new(dependencies: OwnershipDependencies) -> Self {
        Self {
            inner: Arc::new(Shared {
                scope: Arc::new(Mutex::new(())),
                graph: Mutex::new(OwnershipGraph::new()),
                store: dependencies.store,
                audit: dependencies.audit,
                factory: dependencies.factory,
                room: dependencies.room,
                resources: Mutex::new(HashMap::new()),
                seals: Mutex::new(HashMap::new()),
                gates: Mutex::new(HashMap::new()),
                inflight: Mutex::new(HashSet::new()),
                held: Mutex::new(HashSet::new()),
                claimed: Mutex::new(HashSet::new()),
                flights: Mutex::new(HashMap::new()),
                drains: Mutex::new(HashMap::new()),
                notify: Notify::new(),
            }),
        }
    }

    /// Open a root lifetime. A session that already has an open or closing root is refused.
    /// Closing the previous root and calling this again mints a new lifetime.
    pub async fn open_root(
        &self,
        session: SessionId,
        initiator: Initiator,
    ) -> Result<AgentLifetimeId, OwnershipFailure> {
        let lifetime = mint_lifetime();
        let weak = Arc::downgrade(&self.inner);
        let evidence = self.inner.with_graph(|graph| {
            let evidence = graph
                .open_root(session, lifetime.clone(), initiator)
                .map_err(OwnershipFailure::Domain)?;
            self.inner.remember(weak, lifetime.clone(), false);
            Ok(evidence)
        })?;
        if let Err(error) = self.inner.publish_evidence(&evidence, None).await {
            // The caller does not receive the id on this path, so a failed
            // publication must not leave an open root that the next open cannot
            // replace. Nothing else can name this id until publication succeeds.
            // Other sessions' roots stay in the graph.
            self.inner.drop_unpublished_root(&lifetime);
            return Err(error);
        }
        Ok(lifetime)
    }

    /// Reserve, prepare, and admit one child. Identical retries do not call the factory again.
    /// A caller that arrives while the attempt is running joins it. Dropping one waiter
    /// leaves the attempt running.
    pub async fn spawn(&self, command: SpawnCommand) -> Result<SpawnReceipt, OwnershipFailure> {
        if command.task.trim().is_empty() {
            return Err(OwnershipFailure::EmptyTask);
        }
        let (is_leader, mut watch) = self.inner.begin_flight(&command.request_id);
        if !is_leader {
            let watched = await_watch(&mut watch).await;
            if let Some(found) = self.inner.lookup(&command) {
                return found;
            }
            return watched;
        }
        if let Some(found) = self.inner.lookup(&command) {
            self.inner
                .publish_flight(&command.request_id, found.clone());
            return found;
        }
        let shared = Arc::clone(&self.inner);
        let request = command.request_id.clone();
        let command = command.clone();
        tokio::spawn(async move {
            let result = shared.drive_spawn(command).await;
            shared.publish_flight(&request, result);
        });
        await_watch(&mut watch).await
    }

    /// Seal `command.lifetime` and join descendant cleanup.
    /// A second call joins the drain that is still running, or retries targets
    /// whose physical release was not confirmed.
    pub async fn end_lifetime(&self, command: CloseCommand) -> Result<(), OwnershipFailure> {
        let evidence = self.inner.seal_now(
            &command.lifetime,
            mint_close(),
            command.cause,
            command.initiator,
        )?;
        let committed = self.inner.persist_evidence(&evidence).await;
        Arc::clone(&self.inner).start_drain(command.lifetime.clone(), command.external_attachment);
        let waited = match command.timeout {
            Some(duration) => {
                match tokio::time::timeout(duration, self.inner.wait_drain(&command.lifetime)).await
                {
                    Ok(result) => result,
                    Err(_) => return Err(OwnershipFailure::Incomplete),
                }
            }
            None => self.inner.wait_drain(&command.lifetime).await,
        };
        match (committed, waited) {
            (Err(error), Ok(())) => Err(error),
            (_, other) => other,
        }
    }

    /// Attach a cleanup owner for a lifetime the factory did not prepare, such as the root.
    pub fn bind_resources(&self, lifetime: AgentLifetimeId, resources: Arc<dyn ChildResources>) {
        self.inner
            .resources
            .lock()
            .expect("child resources")
            .insert(lifetime, resources);
    }

    /// Gate for an admitted lifetime. Install it on that lifetime's Agent.
    pub fn participation(&self, lifetime: &AgentLifetimeId) -> Option<Arc<dyn OwnedLifetime>> {
        self.inner
            .gates
            .lock()
            .expect("lifetime gates")
            .get(lifetime)
            .map(|gate| Arc::clone(gate) as Arc<dyn OwnedLifetime>)
    }

    /// Reload the store. Reserved, prepared, and attached spawns become unconfirmed
    /// and are not sent to the factory. A closing root resumes its drain.
    pub async fn resume(&self) -> Result<(), OwnershipFailure> {
        Arc::clone(&self.inner).resume_from_store().await
    }

    /// Read one page of children. Illegal restored history stays readable.
    pub fn children(
        &self,
        parent: &AgentLifetimeId,
        cursor: Option<&AgentLifetimeId>,
        limit: usize,
    ) -> Result<crate::domain::agent_execution::subagents::ChildPage, OwnershipFailure> {
        self.inner.with_graph(|graph| {
            graph
                .children_page(parent, cursor, limit)
                .map_err(OwnershipFailure::Domain)
        })
    }

    /// Lifetime state, when the id is present.
    pub fn lifetime_state(&self, id: &AgentLifetimeId) -> Option<LifetimeState> {
        self.inner.with_graph(|graph| graph.lifetime_state(id))
    }

    /// First close cause, when sealing has started.
    pub fn close_cause(&self, id: &AgentLifetimeId) -> Option<LifetimeCause> {
        self.inner
            .with_graph(|graph| graph.close_cause(id).cloned())
    }

    /// First close initiator, when sealing has started.
    pub fn close_initiator(&self, id: &AgentLifetimeId) -> Option<Initiator> {
        self.inner
            .with_graph(|graph| graph.close_initiator(id).cloned())
    }

    /// Ancestor that cascaded this close.
    pub fn cascaded_from(&self, id: &AgentLifetimeId) -> Option<AgentLifetimeId> {
        self.inner
            .with_graph(|graph| graph.cascaded_from(id).cloned())
    }

    /// Spawn progress for a request id.
    pub fn spawn_progress(&self, id: &SpawnRequestId) -> Option<SpawnProgress> {
        self.inner
            .with_graph(|graph| graph.spawn_progress(id).cloned())
    }

    /// Admit or suppress one child result for the parent lifetime.
    pub async fn deliver_report(
        &self,
        report: ReportId,
        child: &AgentLifetimeId,
        parent: &AgentLifetimeId,
    ) -> Result<DeliveryState, OwnershipFailure> {
        let evidence = self.inner.with_graph(|graph| {
            graph
                .admit_report(report.clone(), child, parent)
                .map_err(OwnershipFailure::Domain)
        })?;
        let state = self
            .inner
            .with_graph(|graph| graph.report_state(&report))
            .ok_or(OwnershipFailure::Domain(OwnershipError::UnknownChild))?;
        self.inner.publish_evidence(&evidence, None).await?;
        Ok(state)
    }
}

impl Shared {
    fn with_graph<T>(&self, body: impl FnOnce(&mut OwnershipGraph) -> T) -> T {
        let _scope = self.scope.lock().expect("tree admission");
        let mut graph = self.graph.lock().expect("ownership graph");
        body(&mut graph)
    }

    fn remember(&self, inner: Weak<Shared>, lifetime: AgentLifetimeId, sealed: bool) {
        let flag = Arc::new(AtomicBool::new(sealed));
        let gate = Arc::new(LifetimeGate {
            inner,
            lifetime: lifetime.clone(),
            scope: Arc::clone(&self.scope),
            sealed: Arc::clone(&flag),
        });
        self.seals
            .lock()
            .expect("lifetime seals")
            .insert(lifetime.clone(), flag);
        self.gates
            .lock()
            .expect("lifetime gates")
            .insert(lifetime, gate);
    }

    fn drop_unpublished_root(&self, lifetime: &AgentLifetimeId) {
        // Same lock order as `remember`: tree scope, graph, then seal maps.
        self.with_graph(|graph| {
            self.seals.lock().expect("lifetime seals").remove(lifetime);
            self.gates.lock().expect("lifetime gates").remove(lifetime);
            let mut snapshot = graph.snapshot();
            snapshot
                .lifetimes
                .retain(|row| &row.lifetime_id != lifetime);
            snapshot.spawns.retain(|row| {
                &row.child_lifetime != lifetime && &row.binding.parent_lifetime != lifetime
            });
            snapshot
                .settlements
                .retain(|row| &row.close_lifetime != lifetime && &row.target != lifetime);
            snapshot
                .reports
                .retain(|row| &row.child_lifetime != lifetime && &row.parent_lifetime != lifetime);
            *graph = OwnershipGraph::restore(snapshot);
        });
    }

    fn lookup(&self, command: &SpawnCommand) -> Option<Result<SpawnReceipt, OwnershipFailure>> {
        self.with_graph(|graph| {
            let progress = graph.spawn_progress(&command.request_id).cloned()?;
            let Some(child) = graph.child_lifetime(&command.request_id).cloned() else {
                return Some(Err(OwnershipFailure::Domain(OwnershipError::UnknownSpawn)));
            };
            let Some(stored_policy) = graph.spawn_policy(&command.request_id).cloned() else {
                return Some(Err(OwnershipFailure::Domain(OwnershipError::UnknownSpawn)));
            };
            let Some(session) = graph.session_id(&child).cloned() else {
                return Some(Err(OwnershipFailure::Domain(OwnershipError::UnknownSpawn)));
            };
            let digest_ok = task_digest(&command.task)
                .map(|digest| digest_matches(graph, &command.request_id, &digest))
                .unwrap_or(false);
            if stored_parent(graph, &command.request_id).as_ref() != Some(&command.parent)
                || !digest_ok
                || !origin_matches(graph, &command.request_id, &command.origin)
                || !model_matches(graph, &command.request_id, &command.model)
            {
                return Some(Err(OwnershipFailure::Domain(
                    OwnershipError::RequestConflict,
                )));
            }
            Some(Ok(SpawnReceipt {
                child,
                session,
                progress,
                policy: stored_policy,
            }))
        })
    }

    fn begin_flight(
        &self,
        request: &SpawnRequestId,
    ) -> (
        bool,
        watch::Receiver<Option<Result<SpawnReceipt, OwnershipFailure>>>,
    ) {
        let mut flights = self.flights.lock().expect("spawn flights");
        if let Some(sender) = flights.get(request) {
            return (false, sender.subscribe());
        }
        let (sender, receiver) = watch::channel(None);
        flights.insert(request.clone(), sender);
        (true, receiver)
    }

    fn publish_flight(
        &self,
        request: &SpawnRequestId,
        result: Result<SpawnReceipt, OwnershipFailure>,
    ) {
        let sender = self.flights.lock().expect("spawn flights").remove(request);
        if let Some(sender) = sender {
            sender.send_replace(Some(result));
        }
    }

    async fn drive_spawn(
        self: &Arc<Self>,
        command: SpawnCommand,
    ) -> Result<SpawnReceipt, OwnershipFailure> {
        if let Some(found) = self.lookup(&command) {
            return found;
        }
        let weak = Arc::downgrade(self);
        let admitted = self.with_graph(|graph| self.admit_new(graph, &command, weak))?;
        let _guard = InflightGuard {
            shared: Arc::clone(self),
            id: admitted.child.clone(),
        };
        self.publish_evidence(&admitted.evidence, Some(&command.request_id))
            .await?;
        self.preparing_factory(&command, admitted).await
    }

    fn admit_new(
        &self,
        graph: &mut OwnershipGraph,
        command: &SpawnCommand,
        _weak: Weak<Shared>,
    ) -> Result<Admitted, OwnershipFailure> {
        if graph.spawn_progress(&command.request_id).is_some() {
            return Err(OwnershipFailure::Domain(OwnershipError::RequestConflict));
        }
        let policy = select_inherited_policy(command.policy.clone(), command.child_supports_policy)
            .map_err(OwnershipFailure::Domain)?;
        let parent_session = graph
            .session_id(&command.parent)
            .cloned()
            .ok_or(OwnershipFailure::Domain(OwnershipError::ParentMissing))?;
        if !self.room.try_reserve() {
            return Err(OwnershipFailure::Domain(OwnershipError::NoRoom));
        }
        let child = mint_lifetime();
        let session = mint_session();
        let digest = task_digest(&command.task).map_err(OwnershipFailure::Domain)?;
        let binding = SpawnBinding {
            parent_lifetime: command.parent.clone(),
            parent_session,
            request_id: command.request_id.clone(),
            task_digest: digest,
            policy: policy.clone(),
            model: command.model.clone(),
            origin: command.origin.clone(),
        };
        match graph.admit_spawn(SpawnAdmission {
            child_lifetime: child.clone(),
            child_session: session.clone(),
            binding,
            live_room: true,
        }) {
            Ok(evidence) => {
                self.held.lock().expect("live slots").insert(child.clone());
                self.inflight
                    .lock()
                    .expect("inflight spawns")
                    .insert(child.clone());
                self.remember(_weak, child.clone(), false);
                Ok(Admitted {
                    child,
                    session,
                    evidence,
                    policy,
                })
            }
            Err(error) => {
                self.room.release();
                Err(OwnershipFailure::Domain(error))
            }
        }
    }

    async fn preparing_factory(
        &self,
        command: &SpawnCommand,
        admitted: Admitted,
    ) -> Result<SpawnReceipt, OwnershipFailure> {
        let prepared = self
            .factory
            .prepare(PrepareRequest {
                parent: command.parent.clone(),
                child: admitted.child.clone(),
                session: admitted.session.clone(),
                policy: admitted.policy.clone(),
                model: command.model.clone(),
                task: command.task.clone(),
                request: command.request_id.clone(),
                origin: command.origin.clone(),
            })
            .await;
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(failure) => {
                if let Some(cleanup) = failure.cleanup {
                    self.resources
                        .lock()
                        .expect("child resources")
                        .insert(admitted.child.clone(), cleanup);
                }
                let known = SpawnProgress::Reserved.known();
                let next = match failure.failure {
                    PortFailure::Uncertain => SpawnProgress::Unconfirmed { known },
                    PortFailure::Rejected => SpawnProgress::StartupFailed { known },
                };
                let _ = self.advance(&command.request_id, next).await;
                if matches!(failure.failure, PortFailure::Rejected) {
                    self.finish_startup(&command.request_id, &admitted.child)
                        .await;
                }
                return Err(OwnershipFailure::Startup(failure.failure));
            }
        };
        self.resources
            .lock()
            .expect("child resources")
            .insert(admitted.child.clone(), prepared.resources);
        let dispatch = self.with_graph(|graph| {
            graph
                .dispatch_after_prepare(&command.request_id)
                .map_err(OwnershipFailure::Domain)
        })?;
        self.notify.notify_waiters();
        if matches!(
            dispatch,
            crate::domain::agent_execution::subagents::Dispatch::Drain
        ) {
            self.close_claimed(&command.parent, &admitted.child).await;
            let _ = self
                .advance(
                    &command.request_id,
                    SpawnProgress::Ended {
                        known: SpawnProgress::Reserved.known(),
                    },
                )
                .await;
            return Err(OwnershipFailure::Domain(OwnershipError::ParentClosing));
        }
        self.advance(&command.request_id, SpawnProgress::Prepared)
            .await?;
        self.advance(&command.request_id, SpawnProgress::Attached)
            .await?;
        match prepared
            .submit
            .submit(&command.task, &command.request_id)
            .await
        {
            Ok(receipt) => {
                self.advance(&command.request_id, SpawnProgress::TaskAdmitted { receipt })
                    .await?;
                self.receipt(&command.request_id)
            }
            Err(PortFailure::Uncertain) => {
                let _ = self
                    .advance(
                        &command.request_id,
                        SpawnProgress::Unconfirmed {
                            known: SpawnProgress::Attached.known(),
                        },
                    )
                    .await;
                Err(OwnershipFailure::Submission(PortFailure::Uncertain))
            }
            Err(PortFailure::Rejected) => Err(OwnershipFailure::Submission(PortFailure::Rejected)),
        }
    }

    async fn finish_startup(&self, request: &SpawnRequestId, child: &AgentLifetimeId) {
        let report = self.close_claimed_local(child).await;
        if report.physical == PhysicalFact::Released {
            self.release_slot(child);
            let _ = self
                .advance(
                    request,
                    SpawnProgress::Ended {
                        known: SpawnProgress::Reserved.known(),
                    },
                )
                .await;
        }
    }

    async fn close_claimed(&self, root: &AgentLifetimeId, target: &AgentLifetimeId) {
        let Some(resources) = self.claim(target) else {
            return;
        };
        let (cause, initiator, operation) = self.close_facts(root);
        let report = match (cause, initiator, operation) {
            (Some(cause), Some(initiator), Some(operation)) => {
                let report = resources.close(&cause, &initiator).await;
                self.apply_close(root, &operation, target, report).await;
                report
            }
            _ => {
                resources
                    .close(&LifetimeCause::HostClose, &Initiator::Runtime)
                    .await
            }
        };
        if report.physical == PhysicalFact::Released {
            self.release_slot(target);
        }
        self.notify.notify_waiters();
    }

    async fn close_claimed_local(&self, target: &AgentLifetimeId) -> super::ports::ResourceReport {
        let Some(resources) = self.claim(target) else {
            return super::ports::ResourceReport {
                physical: PhysicalFact::Pending,
                evidence: EvidenceFact::Pending,
            };
        };
        let report = resources
            .close(&LifetimeCause::TerminalFailure, &Initiator::Runtime)
            .await;
        self.notify.notify_waiters();
        report
    }

    fn claim(&self, target: &AgentLifetimeId) -> Option<Arc<dyn ChildResources>> {
        let resources = self
            .resources
            .lock()
            .expect("child resources")
            .get(target)
            .cloned()?;
        let mut claimed = self.claimed.lock().expect("close claims");
        if !claimed.insert(target.clone()) {
            return None;
        }
        Some(resources)
    }

    fn close_facts(
        &self,
        root: &AgentLifetimeId,
    ) -> (
        Option<LifetimeCause>,
        Option<Initiator>,
        Option<CloseOperationId>,
    ) {
        self.with_graph(|graph| {
            (
                graph.close_cause(root).cloned(),
                graph.close_initiator(root).cloned(),
                graph.close_operation(root).cloned(),
            )
        })
    }

    async fn apply_close(
        &self,
        root: &AgentLifetimeId,
        operation: &CloseOperationId,
        target: &AgentLifetimeId,
        report: super::ports::ResourceReport,
    ) {
        let evidence = self.with_graph(|graph| {
            graph
                .apply_report(root, operation, target, report.physical, report.evidence)
                .ok()
        });
        if let Some(evidence) = evidence {
            let _ = self.audit.record(&evidence).await;
            let snapshot = self.with_graph(|graph| graph.snapshot());
            let _ = self.store.write(&snapshot).await;
        }
    }

    async fn advance(
        &self,
        request: &SpawnRequestId,
        next: SpawnProgress,
    ) -> Result<(), OwnershipFailure> {
        let evidence = self.with_graph(|graph| {
            graph
                .advance_spawn(request, next)
                .map_err(OwnershipFailure::Domain)
        })?;
        self.publish_evidence(&evidence, None).await
    }

    fn receipt(&self, request: &SpawnRequestId) -> Result<SpawnReceipt, OwnershipFailure> {
        self.with_graph(|graph| {
            let progress = graph
                .spawn_progress(request)
                .cloned()
                .ok_or(OwnershipFailure::Domain(OwnershipError::UnknownSpawn))?;
            let child = graph
                .child_lifetime(request)
                .cloned()
                .ok_or(OwnershipFailure::Domain(OwnershipError::UnknownSpawn))?;
            let session = graph
                .session_id(&child)
                .cloned()
                .ok_or(OwnershipFailure::Domain(OwnershipError::UnknownSpawn))?;
            let policy = graph
                .spawn_policy(request)
                .cloned()
                .ok_or(OwnershipFailure::Domain(OwnershipError::UnknownSpawn))?;
            Ok(SpawnReceipt {
                child,
                session,
                progress,
                policy,
            })
        })
    }

    /// Audit this evidence, then write the snapshot only when that audit is accepted.
    /// `unconfirmed_request` marks that spawn unconfirmed when the audit or the store is uncertain.
    async fn publish_evidence(
        &self,
        evidence: &OwnershipEvidence,
        unconfirmed_request: Option<&SpawnRequestId>,
    ) -> Result<(), OwnershipFailure> {
        match self.audit.record(evidence).await {
            Ok(()) => {}
            Err(PortFailure::Rejected) => {
                return Err(OwnershipFailure::Audit(PortFailure::Rejected));
            }
            Err(PortFailure::Uncertain) => {
                if let Some(request) = unconfirmed_request {
                    self.mark_unconfirmed(request).await;
                }
                return Err(OwnershipFailure::Audit(PortFailure::Uncertain));
            }
        }
        let snapshot = self.with_graph(|graph| graph.snapshot());
        match self.store.write(&snapshot).await {
            Ok(()) => Ok(()),
            Err(PortFailure::Rejected) => Err(OwnershipFailure::Store(PortFailure::Rejected)),
            Err(PortFailure::Uncertain) => {
                if let Some(request) = unconfirmed_request {
                    self.mark_unconfirmed(request).await;
                }
                Err(OwnershipFailure::Store(PortFailure::Uncertain))
            }
        }
    }

    async fn mark_unconfirmed(&self, request: &SpawnRequestId) {
        let evidence = self.with_graph(|graph| {
            let progress = graph.spawn_progress(request)?.clone();
            graph
                .advance_spawn(
                    request,
                    SpawnProgress::Unconfirmed {
                        known: progress.known(),
                    },
                )
                .ok()
        });
        if let Some(evidence) = evidence {
            let _ = self.audit.record(&evidence).await;
            let snapshot = self.with_graph(|graph| graph.snapshot());
            let _ = self.store.write(&snapshot).await;
        }
    }

    /// Write the snapshot even when the audit port rejects the record.
    /// Close intent and an interrupted cascade stay durable across that rejection.
    async fn persist_evidence(&self, evidence: &OwnershipEvidence) -> Result<(), OwnershipFailure> {
        let audit = self.audit.record(evidence).await;
        let snapshot = self.with_graph(|graph| graph.snapshot());
        let stored = self.store.write(&snapshot).await;
        match (audit, stored) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(failure), _) => Err(OwnershipFailure::Audit(failure)),
            (Ok(()), Err(failure)) => Err(OwnershipFailure::Store(failure)),
        }
    }

    fn seal_now(
        &self,
        lifetime: &AgentLifetimeId,
        operation: CloseOperationId,
        cause: LifetimeCause,
        initiator: Initiator,
    ) -> Result<OwnershipEvidence, OwnershipFailure> {
        self.with_graph(|graph| {
            let admission = graph
                .begin_close(lifetime, operation, cause, initiator)
                .map_err(OwnershipFailure::Domain)?;
            let seals = self.seals.lock().expect("lifetime seals");
            if let Some(flag) = seals.get(lifetime) {
                flag.store(true, Ordering::Release);
            }
            for target in &admission.targets {
                if let Some(flag) = seals.get(target) {
                    flag.store(true, Ordering::Release);
                }
            }
            Ok(admission.evidence)
        })
    }

    fn start_drain(self: &Arc<Self>, root: AgentLifetimeId, external: bool) {
        {
            let drains = self.drains.lock().expect("close drains");
            if let Some(slot) = drains.get(&root) {
                if slot.receiver.borrow().is_none() {
                    if external {
                        slot.external.store(true, Ordering::Release);
                    }
                    return;
                }
            }
        }
        if !self.with_graph(|graph| graph.lifetime_state(&root) == Some(LifetimeState::Closing)) {
            return;
        }
        self.unclaim_unreleased(&root);
        let external_flag = Arc::new(AtomicBool::new(external));
        let (sender, receiver) = watch::channel(None);
        self.drains.lock().expect("close drains").insert(
            root.clone(),
            DrainSlot {
                external: Arc::clone(&external_flag),
                receiver,
            },
        );
        let shared = Arc::clone(self);
        tokio::spawn(async move {
            let result = shared.drive_drain(root, external_flag).await;
            sender.send_replace(Some(result));
        });
    }

    fn unclaim_unreleased(&self, root: &AgentLifetimeId) {
        self.with_graph(|graph| {
            let pending = closing_ids(graph, root);
            let mut claimed = self.claimed.lock().expect("close claims");
            for id in pending {
                if graph.physical(root, &id) != Some(PhysicalFact::Released) {
                    claimed.remove(&id);
                }
            }
        });
    }

    async fn wait_drain(&self, id: &AgentLifetimeId) -> Result<(), OwnershipFailure> {
        let receiver = self
            .drains
            .lock()
            .expect("close drains")
            .get(id)
            .map(|slot| slot.receiver.clone());
        let Some(mut receiver) = receiver else {
            return Ok(());
        };
        loop {
            if let Some(result) = receiver.borrow().clone() {
                return result;
            }
            if receiver.changed().await.is_err() {
                return Err(OwnershipFailure::Incomplete);
            }
        }
    }

    async fn drive_drain(
        self: &Arc<Self>,
        root: AgentLifetimeId,
        external: Arc<AtomicBool>,
    ) -> Result<(), OwnershipFailure> {
        loop {
            // Register before inspecting, so a spawn that finishes during this
            // pass still wakes the next wait. `notify_waiters` does not store
            // a permit for a waiter that has not subscribed yet.
            let notified = self.notify.notified();
            tokio::pin!(notified);
            let targets = self.with_graph(|graph| closing_ids(graph, &root));
            if targets.is_empty() {
                return Ok(());
            }
            let mut closed_any = false;
            for target in &targets {
                if external.load(Ordering::Acquire) && target == &root {
                    continue;
                }
                let Some(resources) = self.claim(target) else {
                    continue;
                };
                closed_any = true;
                let (cause, initiator, operation) = self.close_facts(&root);
                let (cause, initiator) = (
                    cause.unwrap_or(LifetimeCause::HostClose),
                    initiator.unwrap_or(Initiator::Runtime),
                );
                let report = resources.close(&cause, &initiator).await;
                if let Some(operation) = operation {
                    self.apply_close(&root, &operation, target, report).await;
                }
                if report.physical == PhysicalFact::Released {
                    self.release_slot(target);
                }
            }
            if closed_any {
                self.notify.notify_waiters();
                continue;
            }
            let blocked = self.drain_blocked(&root, &external);
            if blocked {
                notified.await;
                continue;
            }
            return Err(OwnershipFailure::Incomplete);
        }
    }

    fn drain_blocked(&self, root: &AgentLifetimeId, external: &AtomicBool) -> bool {
        let targets = self.with_graph(|graph| closing_ids(graph, root));
        let external_root = external.load(Ordering::Acquire)
            && targets.iter().any(|target| target == root)
            && self.with_graph(|graph| graph.physical(root, root) != Some(PhysicalFact::Released));
        if external_root || self.subtree_inflight(root) {
            return true;
        }
        let resources = self.resources.lock().expect("child resources");
        let claimed = self.claimed.lock().expect("close claims");
        targets.iter().any(|target| {
            target != root && resources.contains_key(target) && !claimed.contains(target)
        }) || targets.iter().any(|target| {
            !external.load(Ordering::Acquire)
                && target == root
                && resources.contains_key(target)
                && !claimed.contains(target)
        })
    }

    fn subtree_inflight(&self, root: &AgentLifetimeId) -> bool {
        self.with_graph(|graph| {
            let inflight = self.inflight.lock().expect("inflight spawns");
            closing_ids(graph, root)
                .into_iter()
                .any(|id| inflight.contains(&id))
        })
    }

    fn release_slot(&self, lifetime: &AgentLifetimeId) {
        if self.held.lock().expect("live slots").remove(lifetime) {
            self.room.release();
        }
    }

    async fn resume_from_store(self: &Arc<Self>) -> Result<(), OwnershipFailure> {
        let snapshot = self.store.read().await.map_err(OwnershipFailure::Store)?;
        let mut restored = OwnershipGraph::restore(snapshot);
        let recovery = restored.recovery_records().to_vec();
        let refused = restored.refusal().is_some();
        let requests: Vec<_> = restored
            .snapshot()
            .spawns
            .into_iter()
            .map(|row| row.binding.request_id)
            .collect();
        let mut advances = Vec::new();
        if !refused {
            for request in &requests {
                let Some(progress) = restored.spawn_progress(request).cloned() else {
                    continue;
                };
                let next = match progress {
                    SpawnProgress::Reserved | SpawnProgress::Prepared | SpawnProgress::Attached => {
                        Some(SpawnProgress::Unconfirmed {
                            known: progress.known(),
                        })
                    }
                    _ => None,
                };
                if let Some(next) = next {
                    if let Ok(evidence) = restored.advance_spawn(request, next) {
                        advances.push(evidence);
                    }
                }
            }
        }
        let rows = restored.snapshot().lifetimes;
        let weak = Arc::downgrade(self);
        self.with_graph(|graph| {
            *graph = restored;
            for row in &rows {
                self.remember(
                    Weak::clone(&weak),
                    row.lifetime_id.clone(),
                    refused || row.state != LifetimeState::Open,
                );
            }
        });
        for evidence in &advances {
            let _ = self.persist_evidence(evidence).await;
        }
        for evidence in &recovery {
            let _ = self.persist_evidence(evidence).await;
        }
        if refused {
            return Ok(());
        }
        let closing: Vec<_> = rows
            .into_iter()
            .filter(|row| row.state == LifetimeState::Closing && row.cascaded_from.is_none())
            .map(|row| row.lifetime_id)
            .collect();
        for lifetime in closing {
            self.start_drain(lifetime, false);
        }
        Ok(())
    }
}

struct Admitted {
    child: AgentLifetimeId,
    session: SessionId,
    evidence: OwnershipEvidence,
    policy: ApprovalPolicy,
}

fn closing_ids(graph: &OwnershipGraph, root: &AgentLifetimeId) -> Vec<AgentLifetimeId> {
    let mut pending = Vec::new();
    let mut seen = HashSet::new();
    walk_closing(graph, root, &mut seen, &mut pending);
    pending
}

fn walk_closing(
    graph: &OwnershipGraph,
    node: &AgentLifetimeId,
    seen: &mut HashSet<AgentLifetimeId>,
    pending: &mut Vec<AgentLifetimeId>,
) {
    if !seen.insert(node.clone()) {
        return;
    }
    if graph.lifetime_state(node) == Some(LifetimeState::Closing) {
        pending.push(node.clone());
    }
    let mut cursor = None;
    while let Ok(page) = graph.children_page(node, cursor.as_ref(), MAX_READ_PAGE) {
        let next = page.next.clone();
        for child in page.children {
            walk_closing(graph, &child.lifetime, seen, pending);
        }
        cursor = next;
        if cursor.is_none() {
            break;
        }
    }
}

fn digest_matches(graph: &OwnershipGraph, request: &SpawnRequestId, digest: &TaskDigest) -> bool {
    graph
        .snapshot()
        .spawns
        .into_iter()
        .any(|row| &row.binding.request_id == request && &row.binding.task_digest == digest)
}

fn origin_matches(graph: &OwnershipGraph, request: &SpawnRequestId, origin: &SpawnOrigin) -> bool {
    graph
        .snapshot()
        .spawns
        .into_iter()
        .any(|row| &row.binding.request_id == request && &row.binding.origin == origin)
}

fn model_matches(
    graph: &OwnershipGraph,
    request: &SpawnRequestId,
    model: &Option<crate::domain::agent_execution::subagents::ModelChoice>,
) -> bool {
    graph
        .snapshot()
        .spawns
        .into_iter()
        .any(|row| &row.binding.request_id == request && &row.binding.model == model)
}

fn task_digest(task: &str) -> Result<TaskDigest, OwnershipError> {
    let hash = Sha256::digest(task.as_bytes());
    TaskDigest::new(HEXLOWER.encode(&hash))
}

fn mint_lifetime() -> AgentLifetimeId {
    AgentLifetimeId::new(format!("life-{}", Uuid::new_v4().simple())).expect("lifetime id")
}

fn mint_session() -> SessionId {
    SessionId::new(format!("sess-{}", Uuid::new_v4().simple())).expect("session id")
}

fn mint_close() -> CloseOperationId {
    CloseOperationId::new(format!("close-{}", Uuid::new_v4().simple())).expect("close id")
}

async fn await_watch(
    receiver: &mut watch::Receiver<Option<Result<SpawnReceipt, OwnershipFailure>>>,
) -> Result<SpawnReceipt, OwnershipFailure> {
    loop {
        if let Some(result) = receiver.borrow().clone() {
            return result;
        }
        if receiver.changed().await.is_err() {
            return Err(OwnershipFailure::Incomplete);
        }
    }
}

fn to_agent(error: OwnershipFailure) -> AgentError {
    AgentError::InvalidInput(error.to_string())
}

fn host_actor(actor: &ActionContext) -> Result<HostActor, AgentError> {
    HostActor::new(actor.principal_id(), actor.surface_id(), actor.request_id())
        .map_err(|error| AgentError::InvalidInput(error.to_string()))
}

#[async_trait]
impl OwnedLifetime for LifetimeGate {
    fn admission_scope(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.scope)
    }

    fn seal(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.sealed)
    }

    fn seal_for_disposal(&self) {
        let Some(shared) = self.inner.upgrade() else {
            return;
        };
        let Ok(evidence) = shared.seal_now(
            &self.lifetime,
            mint_close(),
            LifetimeCause::OwnerDisposed,
            Initiator::Runtime,
        ) else {
            return;
        };
        let shared_for_commit = Arc::clone(&shared);
        tokio::spawn(async move {
            let _ = shared_for_commit.persist_evidence(&evidence).await;
        });
        shared.start_drain(self.lifetime.clone(), true);
    }

    async fn seal_for_host(&self, actor: &ActionContext) -> Result<(), AgentError> {
        let shared = self.inner.upgrade().ok_or(AgentError::Closed)?;
        let evidence = shared
            .seal_now(
                &self.lifetime,
                mint_close(),
                LifetimeCause::HostClose,
                Initiator::Host(host_actor(actor)?),
            )
            .map_err(to_agent)?;
        let committed = shared.persist_evidence(&evidence).await;
        shared.start_drain(self.lifetime.clone(), true);
        committed.map_err(to_agent)
    }

    async fn note_attachment(&self, released: bool, evidence_acknowledged: bool) {
        let Some(shared) = self.inner.upgrade() else {
            return;
        };
        let physical = if released {
            PhysicalFact::Released
        } else {
            PhysicalFact::Failed
        };
        let evidence_fact = if evidence_acknowledged {
            EvidenceFact::Acknowledged
        } else {
            EvidenceFact::Failed
        };
        let recorded = shared.with_graph(|graph| {
            let operation = graph.close_operation(&self.lifetime)?.clone();
            graph
                .apply_report(
                    &self.lifetime,
                    &operation,
                    &self.lifetime,
                    physical,
                    evidence_fact,
                )
                .ok()
        });
        if let Some(evidence) = recorded {
            let _ = shared.audit.record(&evidence).await;
            let snapshot = shared.with_graph(|graph| graph.snapshot());
            let _ = shared.store.write(&snapshot).await;
        }
        if released {
            shared.release_slot(&self.lifetime);
        }
        shared.notify.notify_waiters();
    }

    async fn join_descendants(&self) -> Result<(), AgentError> {
        let shared = self.inner.upgrade().ok_or(AgentError::Closed)?;
        shared.wait_drain(&self.lifetime).await.map_err(to_agent)
    }
}

fn stored_parent(graph: &OwnershipGraph, request: &SpawnRequestId) -> Option<AgentLifetimeId> {
    graph
        .snapshot()
        .spawns
        .into_iter()
        .find_map(|row| (row.binding.request_id == *request).then_some(row.binding.parent_lifetime))
}

#[cfg(all(test, unix))]
mod process_cleanup {
    use super::{CloseCommand, OwnershipCoordinator, OwnershipDependencies, SpawnCommand};
    use crate::application::agent_execution::subagents::{
        ChildFactory, ChildResources, InitialSubmit, LiveCapacity, MemoryOwnershipStore,
        OwnershipAudit, PortFailure, PrepareFailure, PrepareRequest, PreparedChild, ResourceReport,
    };
    use crate::domain::agent_execution::sessions::SessionId;
    use crate::domain::agent_execution::subagents::{
        ApprovalPolicy, EvidenceFact, HostActor, Initiator, LifetimeCause, OwnershipEvidence,
        PhysicalFact, PolicyRead, SpawnOrigin, SpawnRequestId, TaskReceiptId,
    };
    use async_trait::async_trait;
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        Arc, Mutex,
    };

    struct GroupProcess {
        pid: i32,
        released: AtomicBool,
    }

    #[async_trait]
    impl ChildResources for GroupProcess {
        async fn close(&self, _cause: &LifetimeCause, _initiator: &Initiator) -> ResourceReport {
            unsafe {
                libc::kill(-(self.pid), libc::SIGKILL);
                let mut status = 0;
                libc::waitpid(self.pid, &mut status, 0);
            }
            let alive = unsafe { libc::kill(self.pid, 0) } == 0;
            self.released.store(!alive, AtomicOrdering::SeqCst);
            ResourceReport {
                physical: if alive {
                    PhysicalFact::Failed
                } else {
                    PhysicalFact::Released
                },
                evidence: EvidenceFact::Acknowledged,
            }
        }
    }

    struct ProcessFactory {
        child: Mutex<Option<Arc<GroupProcess>>>,
    }

    #[async_trait]
    impl ChildFactory for ProcessFactory {
        async fn prepare(&self, request: PrepareRequest) -> Result<PreparedChild, PrepareFailure> {
            let mut command = Command::new("sleep");
            command.arg("30");
            unsafe {
                command.pre_exec(|| {
                    if libc::setpgid(0, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let child = command.spawn().expect("sleep");
            let process = Arc::new(GroupProcess {
                pid: child.id() as i32,
                released: AtomicBool::new(false),
            });
            // Leak the Child handle so dropping it does not kill ahead of the close.
            std::mem::forget(child);
            *self.child.lock().expect("process") = Some(Arc::clone(&process));
            let _ = request;
            Ok(PreparedChild {
                resources: process,
                submit: Arc::new(OnceSubmit),
            })
        }
    }

    struct OnceSubmit;

    #[async_trait]
    impl InitialSubmit for OnceSubmit {
        async fn submit(
            &self,
            _task: &str,
            _request: &SpawnRequestId,
        ) -> Result<TaskReceiptId, PortFailure> {
            Ok(TaskReceiptId::new("process-receipt").expect("receipt"))
        }
    }

    struct AcceptAudit;

    #[async_trait]
    impl OwnershipAudit for AcceptAudit {
        async fn record(&self, evidence: &OwnershipEvidence) -> Result<(), PortFailure> {
            let _ = evidence;
            Ok(())
        }
    }

    struct ReleasedRoot;

    #[async_trait]
    impl ChildResources for ReleasedRoot {
        async fn close(&self, _cause: &LifetimeCause, _initiator: &Initiator) -> ResourceReport {
            ResourceReport {
                physical: PhysicalFact::Released,
                evidence: EvidenceFact::Acknowledged,
            }
        }
    }

    #[tokio::test]
    async fn parent_close_kills_the_child_process_group() {
        let factory = Arc::new(ProcessFactory {
            child: Mutex::new(None),
        });
        let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
            store: Arc::new(MemoryOwnershipStore::new()),
            audit: Arc::new(AcceptAudit),
            factory: factory.clone(),
            room: Arc::new(LiveCapacity::new(4)),
        });
        let root = coordinator
            .open_root(SessionId::new("root-session").unwrap(), Initiator::Runtime)
            .await
            .unwrap();
        coordinator.bind_resources(root.clone(), Arc::new(ReleasedRoot));
        coordinator
            .spawn(SpawnCommand {
                parent: root.clone(),
                request_id: SpawnRequestId::new("proc-req").unwrap(),
                task: "sleep".to_owned(),
                policy: PolicyRead::Committed(
                    ApprovalPolicy::new("read-only", "ask", "rev-1").unwrap(),
                ),
                child_supports_policy: true,
                model: None,
                origin: SpawnOrigin::Host(HostActor::new("person", "desktop", "spawn").unwrap()),
            })
            .await
            .unwrap();
        let process = factory.child.lock().expect("process").clone().unwrap();
        assert_eq!(unsafe { libc::kill(process.pid, 0) }, 0);
        coordinator
            .end_lifetime(CloseCommand {
                lifetime: root,
                cause: LifetimeCause::HostClose,
                initiator: Initiator::Runtime,
                external_attachment: false,
                timeout: None,
            })
            .await
            .unwrap();
        assert!(process.released.load(AtomicOrdering::SeqCst));
        assert_ne!(unsafe { libc::kill(process.pid, 0) }, 0);
    }
}
