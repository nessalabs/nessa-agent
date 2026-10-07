//! Supervises spawn, tree close, and recovery around [`OwnershipGraph`](crate::domain::agent_execution::subagents::OwnershipGraph).
//!
//! Snapshot publication projects audit-eligible graph state under the admission lock, then writes
//! that copy. A copy that lost the race to a newer acknowledged copy is not
//! written. The store still replaces one body; this fence is the coordinator's.
#![deny(missing_docs)]

use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
    time::Duration,
};

use async_trait::async_trait;
use data_encoding::HEXLOWER;
use sha2::{Digest, Sha256};
use tokio::sync::{watch, Mutex as AsyncMutex, Notify};
use uuid::Uuid;

use super::{
    failure::OwnershipFailure,
    ports::{
        BindResourcesFailure, BindResourcesRefusal, ChildFactory, ChildResources, LiveRoom,
        OwnershipAudit, OwnershipStore, PortFailure, PrepareRequest,
    },
    publication::{OwnershipPublication, PublicationTarget, PublicationToken, PublishedState},
};
use crate::application::agent_execution::{
    agents::{AgentError, OwnedLifetime},
    permissions::ActionContext,
};
use crate::domain::agent_execution::{
    sessions::SessionId,
    subagents::{
        select_inherited_policy, AgentLifetimeId, ApprovalPolicy, CloseOperationId, DeliveryState,
        Dispatch, EvidenceFact, HostActor, Initiator, KnownMilestone, LifetimeCause, LifetimeState,
        OwnershipError, OwnershipEvidence, OwnershipGraph, PhysicalFact, PolicyRead, ReportId,
        SpawnAdmission, SpawnBinding, SpawnOrigin, SpawnProgress, SpawnRequestId, TaskDigest,
        MAX_READ_PAGE,
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

pub(super) struct Shared {
    pub(super) publication: Mutex<OwnershipPublication>,
    /// IDs with a physical owner or a handed-out gate permitting possible transfer.
    pub(super) bound: Mutex<HashSet<AgentLifetimeId>>,
    pub(super) absence_claimed: Mutex<HashSet<AgentLifetimeId>>,
    scope: Arc<Mutex<()>>,
    graph: Mutex<OwnershipGraph>,
    store: Arc<dyn OwnershipStore>,
    pub(super) audit: Arc<dyn OwnershipAudit>,
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
    /// Assigned when a snapshot is copied. A later assignment is a later graph.
    publication_revision: AtomicU64,
    /// Highest revision the store has acknowledged.
    published_revision: AtomicU64,
    /// One store write at a time, so a revision check and its write stay paired.
    write_order: AsyncMutex<()>,
    #[cfg(test)]
    publish_pause: Mutex<Option<Arc<PublishPause>>>,
    #[cfg(test)]
    drain_exclusion: AtomicBool,
    #[cfg(test)]
    absence_pause: Mutex<Option<Arc<PublishPause>>>,
}

type SpawnFlight = watch::Sender<Option<Result<SpawnReceipt, OwnershipFailure>>>;

struct DrainSlot {
    external: Arc<AtomicBool>,
    receiver: watch::Receiver<Option<Result<(), OwnershipFailure>>>,
}

#[cfg(test)]
struct PublishPause {
    entered: Notify,
    release: Notify,
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
                publication: Mutex::new(OwnershipPublication::default()),
                bound: Mutex::new(HashSet::new()),
                absence_claimed: Mutex::new(HashSet::new()),
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
                publication_revision: AtomicU64::new(0),
                published_revision: AtomicU64::new(0),
                write_order: AsyncMutex::new(()),
                #[cfg(test)]
                publish_pause: Mutex::new(None),
                #[cfg(test)]
                drain_exclusion: AtomicBool::new(false),
                #[cfg(test)]
                absence_pause: Mutex::new(None),
            }),
        }
    }

    #[cfg(test)]
    fn pause_next_snapshot_write(&self, pause: Arc<PublishPause>) {
        *self.inner.publish_pause.lock().expect("publish pause") = Some(pause);
    }

    #[cfg(test)]
    fn require_drain_exclusion(&self) {
        self.inner.drain_exclusion.store(true, Ordering::SeqCst);
    }

    /// Open a root lifetime. A session that already has an open or closing root is refused.
    /// Closing the previous root and calling this again mints a new lifetime.
    /// The owned admission task survives caller cancellation. Ownership transfers
    /// when this future returns `Ready(Ok(id))`; an unclaimed queued success seals
    /// and reconciles its root. Audit uncertainty or failed eligible publication
    /// retains the identity in a nonrunnable shape; definite preeligible audit
    /// rejection removes only that private root.
    ///
    /// # Errors
    /// Returns a domain refusal, audit rejection/uncertainty, or store failure.
    /// Use [`Self::active_root_for_session`] to find a retained active identity.
    pub async fn open_root(
        &self,
        session: SessionId,
        initiator: Initiator,
    ) -> Result<AgentLifetimeId, OwnershipFailure> {
        super::root::open(Arc::clone(&self.inner), session, initiator).await
    }

    /// Read an audit-eligible Open or Closing root for a session.
    /// Private admissions and Closed history are excluded. This supports explicit
    /// reconciliation after a failed opening; calling `open_root` still refuses
    /// an existing active root.
    pub fn active_root_for_session(&self, session: &SessionId) -> Option<AgentLifetimeId> {
        self.inner.with_graph(|graph| {
            let id = graph.root_lifetime_for_session(session)?;
            self.inner
                .publication
                .lock()
                .expect("ownership publication")
                .root_eligible(id)
                .then(|| id.clone())
        })
    }

    /// Reserve, prepare, and admit one child. Identical retries do not call the factory again.
    /// A caller that arrives while the attempt is running joins it. Dropping one waiter
    /// leaves the attempt running.
    ///
    /// The coordinator owns the admitted attempt, its retained cleanup resources,
    /// and its publication evidence. The installed Agent's shared lifetime gate
    /// owns attachment and submission admission.
    ///
    /// Stage-specific failure, capacity, and sealing semantics and their named
    /// enforcers are defined by the [ADR329 ordering table](https://github.com/nessalabs/nessa-agent/blob/main/docs/adr/todo/329-subagents.md#sdk-audit-eligibility-and-owned-root-delivery-628).
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
    /// whose physical release was not confirmed. Overlapping calls share that
    /// one drain; they do not close the same target twice.
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

    /// Transfer a cleanup owner for an admitted lifetime, such as the root.
    /// `resources` remains the caller's responsibility until this returns success.
    /// A successful transfer is retained for close even if later audit/storage fails.
    /// Binding and never-bound absence claims serialize under the admission scope.
    /// An existing owner is not replaced; Closing recovery may accept a vacant
    /// owner slot only before physical release or an absence claim.
    ///
    /// # Errors
    /// Returns [`BindResourcesFailure`] with the exact rejected owner for an
    /// unknown/private/Closed identity, refused history, active factory flight,
    /// occupied slot, or confirmed absence/release.
    pub fn bind_resources(
        &self,
        lifetime: AgentLifetimeId,
        resources: Arc<dyn ChildResources>,
    ) -> Result<(), BindResourcesFailure> {
        let result = self.inner.with_graph(|graph| {
            let mut owned = self.inner.resources.lock().expect("child resources");
            let refusal = self.inner.transfer_refusal(graph, &lifetime).or_else(|| {
                owned
                    .contains_key(&lifetime)
                    .then_some(BindResourcesRefusal::AlreadyBound)
            });
            if let Some(reason) = refusal {
                return Err(BindResourcesFailure { reason, resources });
            }
            self.inner
                .bound
                .lock()
                .expect("bound lifetimes")
                .insert(lifetime.clone());
            owned.insert(lifetime, resources);
            Ok(())
        });
        if result.is_ok() {
            self.inner.notify.notify_waiters();
        }
        result
    }

    /// Hand out participation for an admitted lifetime, for installation on its Agent.
    /// Gate handoff records possible external resource transfer; it does not prove
    /// physical existence, but it prevents subsequent never-bound absence claims.
    /// Private admissions, Closed history, and confirmed absence/release return
    /// `None`, as do active child flights whose typed factory request owns the gate.
    /// Refused restored history retains its already-sealed inspection
    /// gate, which cannot authorize attachment. Already-held gates keep their seal for
    /// correlated attachment cleanup reporting after close.
    pub fn participation(&self, lifetime: &AgentLifetimeId) -> Option<Arc<dyn OwnedLifetime>> {
        self.inner.with_graph(|graph| {
            if graph.refusal().is_some() {
                let gate = self
                    .inner
                    .gates
                    .lock()
                    .expect("lifetime gates")
                    .get(lifetime)
                    .cloned()?;
                return gate
                    .sealed
                    .load(Ordering::Acquire)
                    .then_some(gate as Arc<dyn OwnedLifetime>);
            }
            if self.inner.transfer_refusal(graph, lifetime).is_some() {
                return None;
            }
            let gate = self
                .inner
                .gates
                .lock()
                .expect("lifetime gates")
                .get(lifetime)
                .cloned()?;
            self.inner
                .bound
                .lock()
                .expect("possible transfers")
                .insert(lifetime.clone());
            Some(gate as Arc<dyn OwnedLifetime>)
        })
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
        let (evidence, token) = self.inner.with_graph(|graph| {
            let evidence = graph
                .admit_report(report.clone(), child, parent)
                .map_err(OwnershipFailure::Domain)?;
            let state = graph.report_state(&report).expect("admitted report");
            let token = self
                .inner
                .publication
                .lock()
                .expect("ownership publication")
                .begin(
                    PublicationTarget::Report(report.clone()),
                    PublishedState::Report(state),
                );
            Ok::<_, OwnershipFailure>((evidence, token))
        })?;
        let state = self
            .inner
            .with_graph(|graph| graph.report_state(&report))
            .ok_or(OwnershipFailure::Domain(OwnershipError::UnknownChild))?;
        self.inner.publish_evidence(&evidence, &token, None).await?;
        Ok(state)
    }
}

impl Shared {
    pub(super) fn with_graph<T>(&self, body: impl FnOnce(&mut OwnershipGraph) -> T) -> T {
        let _scope = self.scope.lock().expect("tree admission");
        let mut graph = self.graph.lock().expect("ownership graph");
        body(&mut graph)
    }

    fn transfer_refusal(
        &self,
        graph: &OwnershipGraph,
        lifetime: &AgentLifetimeId,
    ) -> Option<BindResourcesRefusal> {
        match graph.lifetime_state(lifetime) {
            None => Some(BindResourcesRefusal::UnknownLifetime),
            Some(LifetimeState::Closed) => Some(BindResourcesRefusal::Closed),
            _ if graph.refusal().is_some() => Some(BindResourcesRefusal::RefusedHistory),
            _ if !self
                .publication
                .lock()
                .expect("ownership publication")
                .lifetime_eligible(graph, lifetime) =>
            {
                Some(BindResourcesRefusal::UnpublishedLifetime)
            }
            Some(LifetimeState::Open)
                if graph.snapshot().spawns.iter().any(|row| {
                    &row.child_lifetime == lifetime
                        && matches!(
                            row.progress,
                            SpawnProgress::Ended {
                                known: KnownMilestone::Reserved
                            }
                        )
                }) =>
            {
                Some(BindResourcesRefusal::Released)
            }
            _ if self
                .inflight
                .lock()
                .expect("inflight spawns")
                .contains(lifetime) =>
            {
                Some(BindResourcesRefusal::FactoryInFlight)
            }
            _ if self
                .absence_claimed
                .lock()
                .expect("absence claims")
                .contains(lifetime)
                || graph.snapshot().settlements.iter().any(|row| {
                    row.target == *lifetime && row.physical == PhysicalFact::Released
                }) =>
            {
                Some(BindResourcesRefusal::Released)
            }
            _ => None,
        }
    }

    pub(super) fn remember(&self, inner: Weak<Shared>, lifetime: AgentLifetimeId, sealed: bool) {
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

    pub(super) fn drop_unpublished_root(&self, lifetime: &AgentLifetimeId) -> bool {
        self.with_graph(|graph| {
            let mut publication = self.publication.lock().expect("ownership publication");
            if publication.root_eligible(lifetime)
                || self
                    .bound
                    .lock()
                    .expect("bound lifetimes")
                    .contains(lifetime)
            {
                return false;
            }
            if graph.discard_private_root(lifetime).is_ok() {
                publication.discard_root(lifetime);
                self.seals.lock().expect("lifetime seals").remove(lifetime);
                self.gates.lock().expect("lifetime gates").remove(lifetime);
                true
            } else {
                false
            }
        })
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
        if let Err(error) = self
            .publish_evidence(
                &admitted.evidence,
                &admitted.token,
                Some(&command.request_id),
            )
            .await
        {
            // Publication failure has already revoked admission before its
            // fallback evidence awaits; only now may a definite slot return.
            // Rejected means the publication did not happen, so this reservation
            // is not a live child. The graph row stays so an identical retry
            // still finds it and does not prepare a second child.
            if matches!(
                error,
                OwnershipFailure::Audit(PortFailure::Rejected)
                    | OwnershipFailure::Store(PortFailure::Rejected)
            ) {
                self.release_slot(&admitted.child);
            }
            return Err(error);
        }
        let child = admitted.child.clone();
        let result = self.preparing_factory(&command, admitted).await;
        if result.is_err() {
            // Only this invocation's admitted child belongs to this fallback.
            let closing = self.revoke_child(&child);
            self.persist_revocation(closing).await;
        }
        result
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
        if let Some(refusal) = graph.refusal() {
            return Err(OwnershipFailure::Domain(
                if *refusal == OwnershipError::Cycle {
                    OwnershipError::Cycle
                } else {
                    OwnershipError::DispatchRefused
                },
            ));
        }
        if !self
            .publication
            .lock()
            .expect("ownership publication")
            .lifetime_eligible(graph, &command.parent)
        {
            return Err(OwnershipFailure::UnpublishedParent);
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
                let token = self
                    .publication
                    .lock()
                    .expect("ownership publication")
                    .begin(
                        PublicationTarget::Spawn(command.request_id.clone()),
                        PublishedState::Spawn(SpawnProgress::Reserved),
                    );
                Ok(Admitted {
                    token,
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
        let owned_lifetime = self.with_graph(|_| {
            self.bound
                .lock()
                .expect("possible transfers")
                .insert(admitted.child.clone());
            self.gates
                .lock()
                .expect("lifetime gates")
                .get(&admitted.child)
                .cloned()
                .expect("admitted child gate") as Arc<dyn OwnedLifetime>
        });
        let prepared = self
            .factory
            .prepare(PrepareRequest {
                owned_lifetime,
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
                // Stop new attachment admission before any fallible safety publication.
                // This revokes permission; it does not prove physical cleanup.
                let closing = self.revoke_child(&admitted.child);
                if let Some(cleanup) = failure.cleanup {
                    self.bound
                        .lock()
                        .expect("bound lifetimes")
                        .insert(admitted.child.clone());
                    self.resources
                        .lock()
                        .expect("child resources")
                        .insert(admitted.child.clone(), cleanup);
                }
                self.persist_revocation(closing).await;
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
        self.bound
            .lock()
            .expect("bound lifetimes")
            .insert(admitted.child.clone());
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
        if matches!(dispatch, Dispatch::Drain | Dispatch::ChildUnavailable) {
            let root = if dispatch == Dispatch::ChildUnavailable {
                &admitted.child
            } else {
                &command.parent
            };
            self.close_claimed(root, &admitted.child).await;
            let _ = self
                .advance(
                    &command.request_id,
                    SpawnProgress::Ended {
                        known: SpawnProgress::Reserved.known(),
                    },
                )
                .await;
            let error = if dispatch == Dispatch::ChildUnavailable {
                OwnershipError::ChildUnavailable
            } else {
                OwnershipError::ParentClosing
            };
            return Err(OwnershipFailure::Domain(error));
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
                let closing = self.revoke_child(&admitted.child);
                self.persist_revocation(closing).await;
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
            // There is no fallback await here; drive_spawn synchronously
            // revokes its locally admitted child before returning this error.
            Err(PortFailure::Rejected) => Err(OwnershipFailure::Submission(PortFailure::Rejected)),
        }
    }

    async fn finish_startup(&self, request: &SpawnRequestId, child: &AgentLifetimeId) {
        let owned = self
            .resources
            .lock()
            .expect("child resources")
            .contains_key(child);
        let report = self.close_claimed_local(child).await;
        // No cleanup owner means startup held nothing physical. `StartupFailed`
        // would still say that cleanup is owned, so the chart ends and the live
        // slot goes back. A real owner keeps the slot until release is confirmed.
        if report.physical == PhysicalFact::Released || !owned {
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
        if report.physical == PhysicalFact::Released {
            self.release_slot(target);
        }
        let evidence = self.with_graph(|graph| {
            graph
                .apply_report(root, operation, target, report.physical, report.evidence)
                .ok()
        });
        if let Some(evidence) = evidence {
            let _ = self.audit.record(&evidence).await;
            let _ = self.commit_snapshot().await;
        }
    }

    async fn advance(
        &self,
        request: &SpawnRequestId,
        next: SpawnProgress,
    ) -> Result<(), OwnershipFailure> {
        let (evidence, token) = self.with_graph(|graph| {
            let evidence = graph
                .advance_spawn(request, next.clone())
                .map_err(OwnershipFailure::Domain)?;
            let token = self
                .publication
                .lock()
                .expect("ownership publication")
                .begin(
                    PublicationTarget::Spawn(request.clone()),
                    PublishedState::Spawn(next),
                );
            Ok::<_, OwnershipFailure>((evidence, token))
        })?;
        self.publish_evidence(&evidence, &token, Some(request))
            .await
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

    /// Audit captured permission; affirmative milestones become eligible only on acceptance.
    /// Captured nonrunnable safety facts use the shared writer regardless of audit outcome.
    /// `unconfirmed_request` conservatively retains a spawn after publication uncertainty.
    pub(super) async fn publish_evidence(
        &self,
        evidence: &OwnershipEvidence,
        token: &PublicationToken,
        unconfirmed_request: Option<&SpawnRequestId>,
    ) -> Result<(), OwnershipFailure> {
        if token.is_safety() {
            return self.persist_token_evidence(evidence, Some(token)).await;
        }
        match self.audit.record(evidence).await {
            Ok(()) => {
                self.with_graph(|_| {
                    self.publication
                        .lock()
                        .expect("ownership publication")
                        .acknowledge(token)
                });
            }
            Err(PortFailure::Rejected) => {
                if let Some(request) = unconfirmed_request {
                    let eligible = self.with_graph(|_| {
                        self.publication
                            .lock()
                            .expect("ownership publication")
                            .spawn_eligible(request)
                    });
                    if eligible {
                        self.mark_unconfirmed(request).await;
                    } else {
                        let closing = self.revoke_request(request);
                        self.persist_revocation(closing).await;
                    }
                }
                return Err(OwnershipFailure::Audit(PortFailure::Rejected));
            }
            Err(PortFailure::Uncertain) => {
                if let Some(request) = unconfirmed_request {
                    self.mark_unconfirmed(request).await;
                }
                return Err(OwnershipFailure::Audit(PortFailure::Uncertain));
            }
        }
        match self.commit_snapshot().await {
            Ok(()) => Ok(()),
            Err(PortFailure::Rejected) => {
                if let Some(request) = unconfirmed_request {
                    let closing = self.revoke_request(request);
                    self.persist_revocation(closing).await;
                }
                Err(OwnershipFailure::Store(PortFailure::Rejected))
            }
            Err(PortFailure::Uncertain) => {
                if let Some(request) = unconfirmed_request {
                    self.mark_unconfirmed(request).await;
                }
                Err(OwnershipFailure::Store(PortFailure::Uncertain))
            }
        }
    }

    /// Copy the graph and write it unless a newer copy is already acknowledged.
    ///
    /// The revision is taken with the copy, under the admission lock. Callers
    /// that lose the race return success: the newer copy includes this graph
    /// unless a later transition removed rows. The store is not asked to keep
    /// history; it still replaces the one retained body.
    pub(super) async fn commit_snapshot(&self) -> Result<(), PortFailure> {
        let (revision, snapshot) = self.with_graph(|graph| {
            let revision = self.publication_revision.fetch_add(1, Ordering::AcqRel) + 1;
            (
                revision,
                self.publication
                    .lock()
                    .expect("ownership publication")
                    .project(graph),
            )
        });
        #[cfg(test)]
        self.pause_before_publish().await;
        let _order = self.write_order.lock().await;
        if revision < self.published_revision.load(Ordering::Acquire) {
            return Ok(());
        }
        self.store.write(&snapshot).await?;
        self.published_revision
            .fetch_max(revision, Ordering::Release);
        Ok(())
    }

    #[cfg(test)]
    async fn pause_before_publish(&self) {
        let pause = self.publish_pause.lock().expect("publish pause").take();
        if let Some(pause) = pause {
            pause.entered.notify_one();
            pause.release.notified().await;
        }
    }

    fn revoke_child_in_graph(
        &self,
        graph: &mut OwnershipGraph,
        child: &AgentLifetimeId,
    ) -> Option<OwnershipEvidence> {
        let first_close = graph.lifetime_state(child) == Some(LifetimeState::Open);
        let evidence = self
            .seal_in_graph(
                graph,
                child,
                mint_close(),
                LifetimeCause::TerminalFailure,
                Initiator::Runtime,
            )
            .ok()?;
        // Joins still seal every actual gate, but are not another first close.
        first_close.then_some(evidence)
    }

    fn revoke_child(&self, child: &AgentLifetimeId) -> Option<OwnershipEvidence> {
        self.with_graph(|graph| self.revoke_child_in_graph(graph, child))
    }

    fn revoke_request(&self, request: &SpawnRequestId) -> Option<OwnershipEvidence> {
        self.with_graph(|graph| {
            let child = graph.child_lifetime(request)?.clone();
            self.revoke_child_in_graph(graph, &child)
        })
    }

    async fn persist_revocation(&self, evidence: Option<OwnershipEvidence>) {
        if let Some(evidence) = evidence {
            // The original typed operation failure remains the returned error.
            let _ = self.persist_evidence(&evidence).await;
        } else {
            // A joined close may need a conservative writer retry; that does
            // not acknowledge or re-audit its existing evidence debt.
            let _ = self.commit_snapshot().await;
        }
    }

    async fn mark_unconfirmed(&self, request: &SpawnRequestId) {
        let (evidence, close) = self.with_graph(|graph| {
            let close = graph
                .child_lifetime(request)
                .cloned()
                .and_then(|child| self.revoke_child_in_graph(graph, &child));
            self.publication
                .lock()
                .expect("ownership publication")
                .retain_spawn(request);
            let Some(progress) = graph.spawn_progress(request).cloned() else {
                return (None, close);
            };
            let evidence = graph
                .advance_spawn(
                    request,
                    SpawnProgress::Unconfirmed {
                        known: progress.known(),
                    },
                )
                .ok();
            (evidence, close)
        });
        if let Some(evidence) = evidence {
            let _ = self.audit.record(&evidence).await;
        }
        if let Some(evidence) = close {
            let _ = self.audit.record(&evidence).await;
        }
        // Already-Unconfirmed and other terminal safety positions need no new
        // progress transition, but their close facts still use the same writer.
        let _ = self.commit_snapshot().await;
    }

    /// Write the snapshot even when the audit port rejects the record.
    /// Close intent and an interrupted cascade stay durable across that rejection.
    pub(super) async fn persist_evidence(
        &self,
        evidence: &OwnershipEvidence,
    ) -> Result<(), OwnershipFailure> {
        self.persist_token_evidence(evidence, None).await
    }

    async fn persist_token_evidence(
        &self,
        evidence: &OwnershipEvidence,
        token: Option<&PublicationToken>,
    ) -> Result<(), OwnershipFailure> {
        let audit = self.audit.record(evidence).await;
        if audit.is_ok() {
            if let Some(token) = token {
                self.with_graph(|_| {
                    self.publication
                        .lock()
                        .expect("ownership publication")
                        .acknowledge(token)
                });
            }
        }
        let stored = self.commit_snapshot().await;
        match (audit, stored) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(failure), _) => Err(OwnershipFailure::Audit(failure)),
            (Ok(()), Err(failure)) => Err(OwnershipFailure::Store(failure)),
        }
    }

    pub(super) fn seal_now(
        &self,
        lifetime: &AgentLifetimeId,
        operation: CloseOperationId,
        cause: LifetimeCause,
        initiator: Initiator,
    ) -> Result<OwnershipEvidence, OwnershipFailure> {
        self.with_graph(|graph| self.seal_in_graph(graph, lifetime, operation, cause, initiator))
    }

    fn seal_in_graph(
        &self,
        graph: &mut OwnershipGraph,
        lifetime: &AgentLifetimeId,
        operation: CloseOperationId,
        cause: LifetimeCause,
        initiator: Initiator,
    ) -> Result<OwnershipEvidence, OwnershipFailure> {
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
    }

    pub(super) fn retain_and_seal_root(
        &self,
        lifetime: &AgentLifetimeId,
    ) -> Result<OwnershipEvidence, OwnershipFailure> {
        self.with_graph(|graph| {
            let evidence = self.seal_in_graph(
                graph,
                lifetime,
                mint_close(),
                LifetimeCause::OwnerDisposed,
                Initiator::Runtime,
            )?;
            self.publication
                .lock()
                .expect("ownership publication")
                .retain_root(lifetime);
            Ok(evidence)
        })
    }

    pub(super) fn start_drain(self: &Arc<Self>, root: AgentLifetimeId, external: bool) {
        // Closing is decided before the drains lock. Nothing holds the tree
        // scope and then takes `drains`, so holding `drains` across the later
        // graph read cannot cycle. The running-slot check and the insert share
        // one guard: two closes cannot both pass the check and start two drains.
        if !self.with_graph(|graph| graph.lifetime_state(&root) == Some(LifetimeState::Closing)) {
            return;
        }
        let mut drains = self.drains.lock().expect("close drains");
        if let Some(slot) = drains.get(&root) {
            if slot.receiver.borrow().is_none() {
                if external {
                    slot.external.store(true, Ordering::Release);
                }
                return;
            }
        }
        if !self.with_graph(|graph| graph.lifetime_state(&root) == Some(LifetimeState::Closing)) {
            return;
        }
        self.unclaim_unreleased(&root);
        #[cfg(test)]
        if self.drain_exclusion.load(Ordering::SeqCst) {
            assert!(
                self.drains.try_lock().is_err(),
                "start_drain must hold the drains lock from the running check through insert"
            );
        }
        let external_flag = Arc::new(AtomicBool::new(external));
        let (sender, receiver) = watch::channel(None);
        drains.insert(
            root.clone(),
            DrainSlot {
                external: Arc::clone(&external_flag),
                receiver,
            },
        );
        drop(drains);
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
        let mut absence_attempted = false;
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
            let mut processed_any = false;
            for target in &targets {
                if external.load(Ordering::Acquire) && target == &root {
                    continue;
                }
                let Some(resources) = self.claim(target) else {
                    if target == &root
                        && !absence_attempted
                        && self
                            .publication
                            .lock()
                            .expect("ownership publication")
                            .root_eligible(target)
                        && !self.bound.lock().expect("bound lifetimes").contains(target)
                    {
                        #[cfg(test)]
                        {
                            let pause = self.absence_pause.lock().expect("absence pause").take();
                            if let Some(pause) = pause {
                                pause.entered.notify_one();
                                pause.release.notified().await;
                            }
                        }
                        absence_attempted = true;
                        super::root::settle_never_bound(self, target).await?;
                        processed_any = true;
                    }
                    continue;
                };
                processed_any = true;
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
            if processed_any {
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
        let publication = OwnershipPublication::restored(&snapshot);
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
        let restored_snapshot = restored.snapshot();
        let nonrunnable_children: HashSet<_> = restored_snapshot
            .spawns
            .iter()
            .filter(|row| {
                matches!(
                    row.progress,
                    SpawnProgress::Unconfirmed { .. }
                        | SpawnProgress::StartupFailed { .. }
                        | SpawnProgress::Draining { .. }
                        | SpawnProgress::Ended { .. }
                )
            })
            .map(|row| row.child_lifetime.clone())
            .collect();
        let rows = restored_snapshot.lifetimes;
        let weak = Arc::downgrade(self);
        self.with_graph(|graph| {
            *graph = restored;
            *self.publication.lock().expect("ownership publication") = publication;
            // A restored identity cannot prove never-bound absence.
            self.bound
                .lock()
                .expect("bound lifetimes")
                .extend(rows.iter().map(|r| r.lifetime_id.clone()));
            for row in &rows {
                self.remember(
                    Weak::clone(&weak),
                    row.lifetime_id.clone(),
                    refused
                        || row.state != LifetimeState::Open
                        || nonrunnable_children.contains(&row.lifetime_id),
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
    token: PublicationToken,
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

pub(super) fn mint_lifetime() -> AgentLifetimeId {
    AgentLifetimeId::new(format!("life-{}", Uuid::new_v4().simple())).expect("lifetime id")
}

fn mint_session() -> SessionId {
    SessionId::new(format!("sess-{}", Uuid::new_v4().simple())).expect("session id")
}

pub(super) fn mint_close() -> CloseOperationId {
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
        if released {
            shared.release_slot(&self.lifetime);
        }
        if let Some(evidence) = recorded {
            let _ = shared.audit.record(&evidence).await;
            let _ = shared.commit_snapshot().await;
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
        coordinator
            .bind_resources(root.clone(), Arc::new(ReleasedRoot))
            .unwrap();
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

#[cfg(test)]
mod lifetime_races {
    use super::{
        CloseCommand, OwnershipCoordinator, OwnershipDependencies, OwnershipFailure, PublishPause,
        SpawnCommand,
    };
    use crate::application::agent_execution::subagents::{
        ChildFactory, ChildResources, InitialSubmit, LiveCapacity, MemoryOwnershipStore,
        OwnershipAudit, OwnershipStore, PortFailure, PrepareFailure, PrepareRequest, PreparedChild,
        ResourceReport,
    };
    use crate::domain::agent_execution::sessions::SessionId;
    use crate::domain::agent_execution::subagents::{
        AgentLifetimeId, ApprovalPolicy, EvidenceFact, HostActor, Initiator, KnownMilestone,
        LifetimeCause, LifetimeState, OwnershipError, OwnershipEvidence, PhysicalFact, PolicyRead,
        SpawnOrigin, SpawnProgress, SpawnRequestId, TaskReceiptId,
    };
    use async_trait::async_trait;
    use std::{
        future::Future,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        },
        task::{Context, Poll, Waker},
        time::Duration,
    };
    use tokio::sync::Notify;

    struct AcceptAudit;

    #[async_trait]
    impl OwnershipAudit for AcceptAudit {
        async fn record(&self, _evidence: &OwnershipEvidence) -> Result<(), PortFailure> {
            Ok(())
        }
    }

    struct Released;

    #[async_trait]
    impl ChildResources for Released {
        async fn close(&self, _cause: &LifetimeCause, _initiator: &Initiator) -> ResourceReport {
            ResourceReport {
                physical: PhysicalFact::Released,
                evidence: EvidenceFact::Acknowledged,
            }
        }
    }

    struct Holding {
        closes: AtomicUsize,
        hold: Arc<Notify>,
    }

    #[async_trait]
    impl ChildResources for Holding {
        async fn close(&self, _cause: &LifetimeCause, _initiator: &Initiator) -> ResourceReport {
            self.closes.fetch_add(1, Ordering::SeqCst);
            self.hold.notified().await;
            ResourceReport {
                physical: PhysicalFact::Released,
                evidence: EvidenceFact::Acknowledged,
            }
        }
    }

    struct OnceFactory {
        resources: Arc<Holding>,
    }

    #[async_trait]
    impl ChildFactory for OnceFactory {
        async fn prepare(&self, _request: PrepareRequest) -> Result<PreparedChild, PrepareFailure> {
            Ok(PreparedChild {
                resources: Arc::clone(&self.resources) as Arc<dyn ChildResources>,
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
            Ok(TaskReceiptId::new("receipt").expect("receipt"))
        }
    }

    fn command(parent: AgentLifetimeId) -> SpawnCommand {
        SpawnCommand {
            parent,
            request_id: SpawnRequestId::new("child").unwrap(),
            task: "draft".to_owned(),
            policy: PolicyRead::Committed(
                ApprovalPolicy::new("read-only", "ask", "rev-1").unwrap(),
            ),
            child_supports_policy: true,
            model: None,
            origin: SpawnOrigin::Host(HostActor::new("person", "desktop", "spawn").unwrap()),
        }
    }

    struct RecordingReleased {
        closes: AtomicUsize,
        causes: Mutex<Vec<LifetimeCause>>,
    }

    #[async_trait]
    impl ChildResources for RecordingReleased {
        async fn close(&self, cause: &LifetimeCause, initiator: &Initiator) -> ResourceReport {
            assert_eq!(initiator, &Initiator::Runtime);
            self.closes.fetch_add(1, Ordering::SeqCst);
            self.causes.lock().unwrap().push(cause.clone());
            ResourceReport {
                physical: PhysicalFact::Released,
                evidence: EvidenceFact::Acknowledged,
            }
        }
    }

    async fn unclaimed_root_at_absence_boundary() -> (
        OwnershipCoordinator,
        Arc<MemoryOwnershipStore>,
        AgentLifetimeId,
        Arc<PublishPause>,
    ) {
        let store = Arc::new(MemoryOwnershipStore::new());
        let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
            store: store.clone(),
            audit: Arc::new(AcceptAudit),
            factory: Arc::new(OnceFactory {
                resources: Arc::new(Holding {
                    closes: AtomicUsize::new(0),
                    hold: Arc::new(Notify::new()),
                }),
            }),
            room: Arc::new(LiveCapacity::new(4)),
        });
        let pause = Arc::new(PublishPause {
            entered: Notify::new(),
            release: Notify::new(),
        });
        *coordinator.inner.absence_pause.lock().unwrap() = Some(pause.clone());
        let mut opening = Box::pin(coordinator.open_root(
            SessionId::new("binding-between-inspections").unwrap(),
            Initiator::Runtime,
        ));
        assert!(matches!(
            opening
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        let root = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(row) = store.read().await.unwrap().lifetimes.first() {
                    break row.lifetime_id.clone();
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("owned admission queued its unclaimed delivery");
        drop(opening);
        tokio::time::timeout(Duration::from_secs(3), pause.entered.notified())
            .await
            .expect("unclaimed reconciliation reached speculative absence boundary");
        assert_eq!(
            coordinator.lifetime_state(&root),
            Some(LifetimeState::Closing)
        );
        assert_eq!(
            coordinator.close_cause(&root),
            Some(LifetimeCause::OwnerDisposed)
        );
        assert!(!coordinator.inner.bound.lock().unwrap().contains(&root));
        assert!(!coordinator
            .inner
            .absence_claimed
            .lock()
            .unwrap()
            .contains(&root));
        (coordinator, store, root, pause)
    }

    #[tokio::test]
    async fn row_22_binding_between_absence_inspections_is_drained_without_caller_retry() {
        let (coordinator, store, root, pause) = unclaimed_root_at_absence_boundary().await;
        let owner = Arc::new(RecordingReleased {
            closes: AtomicUsize::new(0),
            causes: Mutex::new(Vec::new()),
        });
        coordinator
            .bind_resources(root.clone(), owner.clone())
            .unwrap();
        pause.release.notify_one();
        // Observe the original owned drain; no end_lifetime call starts a retry.
        let result =
            tokio::time::timeout(Duration::from_secs(3), coordinator.inner.wait_drain(&root))
                .await
                .expect("original unclaimed-root drain finished");
        assert_eq!(
            result,
            Ok(()),
            "accepted cleanup owner must stay owned by the original drain"
        );
        assert_eq!(owner.closes.load(Ordering::SeqCst), 1);
        assert_eq!(
            *owner.causes.lock().unwrap(),
            vec![LifetimeCause::OwnerDisposed]
        );
        assert_eq!(
            coordinator.lifetime_state(&root),
            Some(LifetimeState::Closed)
        );
        let snapshot = store.read().await.unwrap();
        assert_eq!(snapshot.lifetimes[0].lifetime_id, root);
        assert_eq!(snapshot.lifetimes[0].state, LifetimeState::Closed);
        assert_eq!(snapshot.settlements[0].physical, PhysicalFact::Released);
        assert_eq!(snapshot.settlements[0].evidence, EvidenceFact::Acknowledged);
        assert!(!coordinator
            .inner
            .absence_claimed
            .lock()
            .unwrap()
            .contains(&root));
    }

    #[tokio::test]
    async fn row_22_gate_between_absence_inspections_retains_closing_incomplete() {
        let (coordinator, store, root, pause) = unclaimed_root_at_absence_boundary().await;
        let gate = coordinator
            .participation(&root)
            .expect("legal Closing gate handoff");
        assert!(gate.is_sealed());
        pause.release.notify_one();
        let result =
            tokio::time::timeout(Duration::from_secs(3), coordinator.inner.wait_drain(&root))
                .await
                .expect("gate-only owned drain finished");
        assert_eq!(result, Err(OwnershipFailure::Incomplete));
        assert_eq!(
            coordinator.lifetime_state(&root),
            Some(LifetimeState::Closing)
        );
        let snapshot = store.read().await.unwrap();
        assert_eq!(snapshot.lifetimes[0].state, LifetimeState::Closing);
        assert!(snapshot.settlements.is_empty());
        assert!(!coordinator
            .inner
            .absence_claimed
            .lock()
            .unwrap()
            .contains(&root));
    }

    #[tokio::test]
    async fn row_37_retained_root_is_sealed_at_the_admission_scope_boundary() {
        let resources = Arc::new(Holding {
            closes: AtomicUsize::new(0),
            hold: Arc::new(Notify::new()),
        });
        let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
            store: Arc::new(MemoryOwnershipStore::new()),
            audit: Arc::new(AcceptAudit),
            factory: Arc::new(OnceFactory {
                resources: Arc::clone(&resources),
            }),
            room: Arc::new(LiveCapacity::new(4)),
        });
        let session = SessionId::new("uncertain-root").unwrap();
        let root = AgentLifetimeId::new("uncertain-root-id").unwrap();
        coordinator.inner.with_graph(|graph| {
            let _ = graph
                .open_root(session.clone(), root.clone(), Initiator::Runtime)
                .unwrap();
            coordinator
                .inner
                .remember(Arc::downgrade(&coordinator.inner), root.clone(), false);
        });
        assert_eq!(coordinator.active_root_for_session(&session), None);

        // This is the first scope boundary after conservative retention. No
        // audit/store await or drain is needed to revoke attachment authority.
        let _ = coordinator.inner.retain_and_seal_root(&root).unwrap();
        coordinator.inner.with_graph(|graph| {
            assert_eq!(graph.lifetime_state(&root), Some(LifetimeState::Closing));
            assert!(coordinator
                .inner
                .publication
                .lock()
                .unwrap()
                .lifetime_eligible(graph, &root));
            assert_eq!(
                graph.close_cause(&root),
                Some(&LifetimeCause::OwnerDisposed)
            );
            assert!(coordinator.inner.seals.lock().unwrap()[&root].load(Ordering::Acquire));
        });
        assert_eq!(
            coordinator.active_root_for_session(&session),
            Some(root.clone())
        );
        let gate = coordinator.participation(&root).unwrap();
        assert!(gate.seal().load(Ordering::Acquire));
        assert_eq!(
            coordinator.spawn(command(root.clone())).await,
            Err(OwnershipFailure::Domain(OwnershipError::ParentClosing))
        );
        assert!(coordinator
            .inner
            .with_graph(|graph| graph.snapshot().spawns.is_empty()));
        assert_eq!(resources.closes.load(Ordering::SeqCst), 0);

        // Closing still accepts an actual cleanup owner; retention never
        // manufactures absence, and repeated sealing preserves the first cause.
        coordinator
            .bind_resources(root.clone(), Arc::new(Released))
            .unwrap();
        let _ = coordinator.inner.retain_and_seal_root(&root).unwrap();
        coordinator.inner.with_graph(|graph| {
            assert_eq!(
                graph.close_cause(&root),
                Some(&LifetimeCause::OwnerDisposed)
            );
            assert!(coordinator
                .inner
                .resources
                .lock()
                .unwrap()
                .contains_key(&root));
        });
    }

    #[tokio::test]
    async fn row_38_error_revocation_joins_preserve_first_cause_and_actual_gates_without_new_evidence(
    ) {
        let hold = Arc::new(Notify::new());
        let resources = Arc::new(Holding {
            closes: AtomicUsize::new(0),
            hold: hold.clone(),
        });
        let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
            store: Arc::new(MemoryOwnershipStore::new()),
            audit: Arc::new(AcceptAudit),
            factory: Arc::new(OnceFactory {
                resources: resources.clone(),
            }),
            room: Arc::new(LiveCapacity::new(4)),
        });
        let root = coordinator
            .open_root(SessionId::new("join-root").unwrap(), Initiator::Runtime)
            .await
            .unwrap();
        let child = coordinator.spawn(command(root)).await.unwrap().child;
        let gate = coordinator.participation(&child).unwrap();
        let first = coordinator
            .inner
            .seal_now(
                &child,
                super::mint_close(),
                LifetimeCause::HostClose,
                Initiator::Runtime,
            )
            .unwrap();
        coordinator.inner.persist_evidence(&first).await.unwrap();
        assert!(coordinator.inner.revoke_child(&child).is_none());
        coordinator.inner.persist_revocation(None).await;
        assert!(gate.is_sealed());
        assert_eq!(
            coordinator.close_cause(&child),
            Some(LifetimeCause::HostClose)
        );
        assert_eq!(
            coordinator.lifetime_state(&child),
            Some(LifetimeState::Closing)
        );
        assert_eq!(resources.closes.load(Ordering::SeqCst), 0);
        hold.notify_one();
        coordinator
            .end_lifetime(CloseCommand {
                lifetime: child.clone(),
                cause: LifetimeCause::TerminalFailure,
                initiator: Initiator::Runtime,
                external_attachment: false,
                timeout: None,
            })
            .await
            .unwrap();
        assert_eq!(resources.closes.load(Ordering::SeqCst), 1);
        assert_eq!(
            coordinator.lifetime_state(&child),
            Some(LifetimeState::Closed)
        );
        assert!(coordinator.inner.revoke_child(&child).is_none());
        assert!(gate.is_sealed());
        assert_eq!(
            coordinator.close_cause(&child),
            Some(LifetimeCause::HostClose)
        );
    }

    #[tokio::test]
    async fn an_older_snapshot_does_not_replace_a_newer_seal() {
        let store = Arc::new(MemoryOwnershipStore::new());
        let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
            store: Arc::clone(&store)
                as Arc<dyn crate::application::agent_execution::subagents::OwnershipStore>,
            audit: Arc::new(AcceptAudit),
            factory: Arc::new(OnceFactory {
                resources: Arc::new(Holding {
                    closes: AtomicUsize::new(0),
                    hold: Arc::new(Notify::new()),
                }),
            }),
            room: Arc::new(LiveCapacity::new(4)),
        });
        let pause = Arc::new(PublishPause {
            entered: Notify::new(),
            release: Notify::new(),
        });
        coordinator.pause_next_snapshot_write(Arc::clone(&pause));
        let opening = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move {
                coordinator
                    .open_root(
                        SessionId::new("root-session").unwrap(),
                        Initiator::Host(HostActor::new("person", "desktop", "open").unwrap()),
                    )
                    .await
            })
        };
        pause.entered.notified().await;
        let root = coordinator
            .inner
            .with_graph(|graph| graph.snapshot().lifetimes[0].lifetime_id.clone());
        coordinator
            .bind_resources(root.clone(), Arc::new(Released))
            .unwrap();
        coordinator
            .end_lifetime(CloseCommand {
                lifetime: root.clone(),
                cause: LifetimeCause::HostClose,
                initiator: Initiator::Host(HostActor::new("person", "desktop", "close").unwrap()),
                external_attachment: false,
                timeout: None,
            })
            .await
            .unwrap();
        pause.release.notify_one();
        opening.await.unwrap().unwrap();
        let stored = store.read().await.unwrap();
        let row = stored
            .lifetimes
            .iter()
            .find(|row| row.lifetime_id == root)
            .expect("root row");
        assert_eq!(row.state, LifetimeState::Closed);
        assert_eq!(row.cause, Some(LifetimeCause::HostClose));
    }

    #[tokio::test]
    async fn row_30_already_safety_reconciliation_still_writes_current_fact() {
        let store = Arc::new(MemoryOwnershipStore::new());
        let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
            store: store.clone(),
            audit: Arc::new(AcceptAudit),
            factory: Arc::new(OnceFactory {
                resources: Arc::new(Holding {
                    closes: AtomicUsize::new(0),
                    hold: Arc::new(Notify::new()),
                }),
            }),
            room: Arc::new(LiveCapacity::new(4)),
        });
        let root = coordinator
            .open_root(SessionId::new("root-session").unwrap(), Initiator::Runtime)
            .await
            .unwrap();
        let request = command(root);
        coordinator.spawn(request.clone()).await.unwrap();
        let safety = SpawnProgress::Unconfirmed {
            known: KnownMilestone::TaskAdmitted {
                receipt: TaskReceiptId::new("receipt").unwrap(),
            },
        };
        coordinator.inner.with_graph(|graph| {
            let _ = graph
                .advance_spawn(&request.request_id, safety.clone())
                .unwrap();
        });
        coordinator
            .inner
            .mark_unconfirmed(&request.request_id)
            .await;
        assert_eq!(store.read().await.unwrap().spawns[0].progress, safety);
    }

    #[tokio::test]
    async fn overlapping_closes_share_one_drain() {
        let hold = Arc::new(Notify::new());
        let resources = Arc::new(Holding {
            closes: AtomicUsize::new(0),
            hold: Arc::clone(&hold),
        });
        let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
            store: Arc::new(MemoryOwnershipStore::new()),
            audit: Arc::new(AcceptAudit),
            factory: Arc::new(OnceFactory {
                resources: Arc::clone(&resources),
            }),
            room: Arc::new(LiveCapacity::new(4)),
        });
        coordinator.require_drain_exclusion();
        let root = coordinator
            .open_root(SessionId::new("root-session").unwrap(), Initiator::Runtime)
            .await
            .unwrap();
        coordinator
            .bind_resources(root.clone(), Arc::new(Released))
            .unwrap();
        coordinator.spawn(command(root.clone())).await.unwrap();
        let first = {
            let coordinator = coordinator.clone();
            let root = root.clone();
            tokio::spawn(async move {
                coordinator
                    .end_lifetime(CloseCommand {
                        lifetime: root,
                        cause: LifetimeCause::HostClose,
                        initiator: Initiator::Host(
                            HostActor::new("person", "desktop", "first").unwrap(),
                        ),
                        external_attachment: false,
                        timeout: None,
                    })
                    .await
            })
        };
        let second = {
            let coordinator = coordinator.clone();
            let root = root.clone();
            tokio::spawn(async move {
                coordinator
                    .end_lifetime(CloseCommand {
                        lifetime: root,
                        cause: LifetimeCause::Deletion,
                        initiator: Initiator::Host(
                            HostActor::new("person", "desktop", "second").unwrap(),
                        ),
                        external_attachment: false,
                        timeout: None,
                    })
                    .await
            })
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while resources.closes.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("child close started");
        assert_eq!(resources.closes.load(Ordering::SeqCst), 1);
        hold.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(5), first)
            .await
            .expect("first close")
            .unwrap()
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), second)
            .await
            .expect("second close")
            .unwrap()
            .unwrap();
        assert_eq!(resources.closes.load(Ordering::SeqCst), 1);
    }
}
