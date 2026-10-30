//! Where each conversation runs: in this process, on an SSH host, on a
//! paired environment, or on any of them, named when it was created (issues
//! #699, #706, #791).
//!
//! ```text
//! create(environment?) ──▶ Environments::place ──▶ ConversationPlacements::place
//!                            (refused unless the host is configured, (written before
//!                             or the paired environment connected,   the record)
//!                             or, for peer:any, some worker serves
//!                             and is not draining)
//! start_slot ──▶ Environments::for_lease(conversation) ──▶ placement
//!   ──▶ here | that host | that peer
//!   | peer:any ──▶ ConversationBindings::binding ──▶ the worker it is bound to,
//!       while that one serves and is not draining
//!     otherwise ──▶ the serving, not draining worker with fewest agent leases
//!       ──▶ ConversationBindings::bind (written before the lease is asked)
//!   the lease's fence restores the saved provider context only when the
//!   binding says this worker opened it (WorkerContext); the context each
//!   session opens there is written back to the binding
//! delete ──▶ Environments::erase ──▶ ConversationPlacements::erase,
//!   ConversationBindings::erase
//! ```
//!
//! Arrows are calls, in order. A placement is written once, before the
//! conversation's record exists, so a conversation never exists without
//! knowing where it runs; it is never moved. A conversation with no
//! placement runs here: that is every conversation unless a host was named,
//! and a gateway whose configuration names none never reaches a host's code.
//! A placement this build cannot read refuses that conversation's opening
//! alone (ADR 202, record scope). A placement naming a host the configuration
//! no longer names is refused the same way, never run somewhere else. A
//! paired environment is placed on only while it is connected, so a name
//! nobody serves is never kept; once placed, the conversation waits for it
//! to dial in again, and its key's credential is asked before every lease.
//!
//! `peer:any` places a conversation on this gateway's pool: every paired
//! environment that holds a credential to serve it, which is the registry's
//! fact and is kept nowhere here. Which worker holds the conversation now is
//! a separate fact, its binding, which moves where the placement never
//! does: chosen when its agent's lease is asked, and kept while that worker
//! serves and is not draining. A conversation moves only when its agent is
//! opened, which is only while no lease of it is live, so one worker holds
//! it at a time. Provider sessions stay on the worker that opened them, so
//! a moved conversation's agent opens new context there.
use super::{CommandPolicyEvaluator, ConversationError, Environment, EnvironmentFuture};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::leases::{EnvironmentRef, PeerEnvironment};
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
use std::{collections::BTreeMap, sync::Arc, time::Duration};

/// The name that places a conversation on any worker of this gateway's
/// pool: its paired environments. An SSH destination holds no `:`, and no
/// key is `any`, so it names nothing else.
pub const POOL: &str = "peer:any";

/// How long an `artifacts_publish` call waits, from its ask, for its file
/// to be staged, read, checked and kept before it answers that it does not
/// know (`timed_out`). It is longer than one read's own bound (row
/// `artifact.publish_deadline`), but the collector takes a lease's files one
/// at a time, so a file queued behind others may still be read after its
/// call stopped waiting; it is then kept for the call's person all the same.
/// Row `artifact.publish_call_wait` in `docs/limits.md`.
pub const PUBLISH_CALL_WAIT: Duration = Duration::from_secs(16 * 60);

/// How long an `artifacts_fetch` call waits, from its ask, for its file to
/// be written to the environment, checked and placed before it answers that
/// it does not know (`timed_out`). A file is written range by range from the
/// conversation's hold, each range waiting at most its read's busy wait.
/// Stopped before every byte was written, the environment is told so and
/// places nothing. Row `artifact.fetch_call_wait` in `docs/limits.md`.
pub const FETCH_CALL_WAIT: Duration = Duration::from_secs(16 * 60);

/// Why a placement could not be read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(unix),
    allow(dead_code, reason = "raised by the Unix gateway's store")
)]
pub(crate) enum PlacementError {
    /// The placement exists and this build cannot read it.
    Unreadable,
    /// The store could not be used just now.
    Unavailable,
}

impl From<PlacementError> for ConversationError {
    fn from(error: PlacementError) -> Self {
        match error {
            PlacementError::Unreadable => Self::PlacementUnreadable,
            PlacementError::Unavailable => Self::Unavailable,
        }
    }
}

/// The durable record of which conversations run on a host, and which.
/// Implemented in infrastructure.
pub(crate) trait ConversationPlacements: Send + Sync {
    /// Record that `conversation`, not yet created, runs on `host`, or here
    /// when `None` (removing a placement a failed creation of the same
    /// identity left). Durable before it answers.
    fn place<'a>(
        &'a self,
        conversation: &'a ConversationId,
        host: Option<&'a str>,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>>;
    /// The host `conversation` runs on, `None` for here.
    fn placement<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<Option<String>, PlacementError>>;
    /// Forget `conversation`'s placement, once its history is erased.
    fn erase<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>>;
}

/// Which worker of the pool holds a `peer:any` conversation now, and the
/// provider context its agent last opened there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkerBinding {
    pub(crate) worker: PeerEnvironment,
    /// The provider context a session of this conversation last opened on
    /// `worker`; `None` until one has. Only this one is restored there.
    pub(crate) context: Option<ExecutionSessionId>,
}

/// The durable record of which worker holds each `peer:any` conversation.
/// Implemented in infrastructure.
pub(crate) trait ConversationBindings: Send + Sync {
    /// Record `binding` for `conversation`, in place of any it had. Durable
    /// before it answers.
    fn bind<'a>(
        &'a self,
        conversation: &'a ConversationId,
        binding: &'a WorkerBinding,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>>;
    /// The worker `conversation` is bound to, `None` before its first lease.
    fn binding<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<Option<WorkerBinding>, PlacementError>>;
    /// Forget `conversation`'s binding, once its history is erased.
    fn erase<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>>;
}

/// What a paired environment's connection carries now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WorkerLoad {
    /// Leases of an agent held on it.
    pub(crate) leases: usize,
    /// It said it is stopping: it admits no new lease.
    pub(crate) draining: bool,
}

/// The paired environments that dial this gateway. Implemented in
/// infrastructure.
pub(crate) trait PairedEnvironments: Send + Sync {
    /// The environment `peer` names, connected or not.
    fn environment(&self, peer: &PeerEnvironment) -> Arc<dyn Environment>;
    /// The paired environments connected now.
    fn connected(&self) -> Vec<PeerEnvironment>;
    /// The paired environments whose connection serves leases now, each
    /// with what it carries, read off that connection.
    fn serving(&self) -> Vec<(PeerEnvironment, WorkerLoad)>;
}

/// The worker of `serving` a new lease goes to: one that is not draining
/// and not in `avoid`, with the fewest leases of an agent, the lowest key
/// among equals.
fn least_loaded(
    serving: &[(PeerEnvironment, WorkerLoad)],
    avoid: &[PeerEnvironment],
) -> Option<PeerEnvironment> {
    serving
        .iter()
        .filter(|(peer, load)| !load.draining && !avoid.contains(peer))
        .min_by_key(|(peer, load)| (load.leases, *peer))
        .map(|(peer, _)| *peer)
}

/// The pool's workers, and where which of them holds each conversation is
/// kept.
type Pool<'a> = (
    &'a Arc<dyn PairedEnvironments>,
    &'a Arc<dyn ConversationBindings>,
);

/// Where a conversation's agent lease is asked: the environment, and for a
/// `peer:any` conversation, what its worker holds of its provider context.
pub(crate) struct LeasePlace {
    pub(crate) environment: Arc<dyn Environment>,
    /// `None` outside the pool: wherever else it runs, the saved context is
    /// where it was saved.
    pub(crate) context: Option<Arc<WorkerContext>>,
    /// Counts this lease against its worker until it is let go, which is
    /// once the lease was asked there and its connection counts it.
    pub(crate) _reservation: Option<Reservation>,
}

/// Leases of the pool chosen for a worker and not yet asked of it, by
/// worker: so leases asked at once, before any reaches its connection, are
/// spread as if each had.
#[derive(Default)]
struct Reserved(std::sync::Mutex<BTreeMap<PeerEnvironment, usize>>);

impl Reserved {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<PeerEnvironment, usize>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The worker `choose` picks from `serving`, each worker's leases
    /// counting those reserved there, reserved for one more lease in the
    /// same step: two openings choosing at once each count the other's.
    /// `None` when `choose` picks none, and nothing is reserved.
    fn reserve(
        self: &Arc<Self>,
        mut serving: Vec<(PeerEnvironment, WorkerLoad)>,
        choose: impl FnOnce(&[(PeerEnvironment, WorkerLoad)]) -> Option<PeerEnvironment>,
    ) -> Option<(PeerEnvironment, Reservation)> {
        let mut reserved = self.lock();
        for (peer, load) in &mut serving {
            load.leases += reserved.get(peer).copied().unwrap_or(0);
        }
        let worker = choose(&serving)?;
        *reserved.entry(worker).or_insert(0) += 1;
        Some((
            worker,
            Reservation {
                reserved: self.clone(),
                worker,
            },
        ))
    }
}

/// One lease reserved on a worker, let go when dropped.
pub(crate) struct Reservation {
    reserved: Arc<Reserved>,
    worker: PeerEnvironment,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut reserved = self.reserved.lock();
        if let Some(count) = reserved.get_mut(&self.worker) {
            *count -= 1;
            if *count == 0 {
                reserved.remove(&self.worker);
            }
        }
    }
}

/// Which provider context the worker a `peer:any` conversation is bound to
/// opened, as its lease's fence asks before restoring one, and told of each
/// context a session opens there, which it writes to the binding.
pub(crate) struct WorkerContext {
    conversation: ConversationId,
    bindings: Arc<dyn ConversationBindings>,
    binding: std::sync::Mutex<WorkerBinding>,
}

impl WorkerContext {
    /// The worker the conversation is bound to.
    pub(crate) fn worker(&self) -> PeerEnvironment {
        self.lock().worker
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, WorkerBinding> {
        self.binding
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether the worker opened `saved`, so it can restore it.
    pub(crate) fn holds(&self, saved: &ExecutionSessionId) -> bool {
        self.lock().context.as_ref() == Some(saved)
    }

    /// A session opened `context` on the worker: kept in the binding, so the
    /// next agent there restores it. Written before the opening answers, so
    /// it lands before anything can move or erase the conversation; a write
    /// that fails costs only that context, opened anew next time, never a
    /// restore of what the worker does not hold.
    pub(crate) async fn opened(&self, context: &ExecutionSessionId) {
        let binding = {
            let mut binding = self.lock();
            if binding.context.as_ref() == Some(context) {
                return;
            }
            binding.context = Some(context.clone());
            binding.clone()
        };
        if let Err(error) = self.bindings.bind(&self.conversation, &binding).await {
            tracing::warn!(conversation_id = %self.conversation, ?error, "the provider context a worker opened could not be recorded; it opens anew next time");
        }
    }
}

/// Where this gateway's conversations can run: here, each SSH host its
/// configuration names, and the paired environments that dial it.
/// Per-thread environments supplied by a workspace provider. This factory owns no
/// model policy; the channel and existing conversation admission choose placement.
pub(crate) trait ThreadEnvironments: Send + Sync {
    fn environment(
        &self,
        conversation: &ConversationId,
    ) -> Result<Arc<dyn Environment>, ConversationError>;
    /// Stop `conversation`'s VM, keeping its disk, unless something still
    /// uses it; the next opening resumes it. Answers whether it stopped one.
    fn rest<'a>(&'a self, conversation: &'a ConversationId) -> EnvironmentFuture<'a, bool>;
}
pub(crate) const BOAT: &str = "boat:thread";
/// Bound retained workspace environments independently of saved thread history.
pub(crate) const MAX_RETAINED_ENVIRONMENTS: usize = 256;

pub(crate) struct Environments {
    workspaces: Option<Arc<dyn ThreadEnvironments>>,
    here: Arc<dyn Environment>,
    hosts: BTreeMap<String, Arc<dyn Environment>>,
    /// `None` when this gateway pairs no environments.
    peers: Option<Arc<dyn PairedEnvironments>>,
    placements: Option<Arc<dyn ConversationPlacements>>,
    /// Which worker holds each `peer:any` conversation; `None` where no
    /// conversation can be placed so, with no paired environments.
    bindings: Option<Arc<dyn ConversationBindings>>,
    /// Who may run which commands on which hosts, asked of the command
    /// policy; `None` when this gateway grants none, and the environment
    /// tools are absent.
    commands: Option<Arc<dyn CommandPolicyEvaluator>>,
    /// How long an `artifacts_publish` call waits for its file to be kept.
    publish_wait: Duration,
    /// How long an `artifacts_fetch` call waits for its file to be placed.
    fetch_wait: Duration,
    /// Pool leases chosen for a worker and not yet asked there.
    reserved: Arc<Reserved>,
}

impl From<Arc<dyn Environment>> for Environments {
    /// Here alone, with nowhere placements are kept: every conversation runs
    /// here. For tests; composition always keeps placements.
    fn from(here: Arc<dyn Environment>) -> Self {
        Self {
            workspaces: None,
            here,
            hosts: BTreeMap::new(),
            peers: None,
            placements: None,
            bindings: None,
            commands: None,
            publish_wait: PUBLISH_CALL_WAIT,
            fetch_wait: FETCH_CALL_WAIT,
            reserved: Arc::default(),
        }
    }
}

impl Environments {
    #[cfg(unix)]
    pub(crate) fn with_workspaces(mut self, workspaces: Arc<dyn ThreadEnvironments>) -> Self {
        self.workspaces = Some(workspaces);
        self
    }

    /// Here, the configured hosts by the destination they are named by, and
    /// where placements are kept.
    #[cfg_attr(not(unix), allow(dead_code, reason = "the Unix gateway composes it"))]
    pub(crate) fn new(
        here: Arc<dyn Environment>,
        hosts: BTreeMap<String, Arc<dyn Environment>>,
        placements: Arc<dyn ConversationPlacements>,
    ) -> Self {
        Self {
            workspaces: None,
            here,
            hosts,
            peers: None,
            placements: Some(placements),
            bindings: None,
            commands: None,
            publish_wait: PUBLISH_CALL_WAIT,
            fetch_wait: FETCH_CALL_WAIT,
            reserved: Arc::default(),
        }
    }

    /// These environments, with the paired environments `peers`, and
    /// `bindings`, where which of them holds each `peer:any` conversation is
    /// kept.
    #[cfg_attr(not(unix), allow(dead_code, reason = "the Unix gateway composes it"))]
    pub(crate) fn with_peers(
        mut self,
        peers: Arc<dyn PairedEnvironments>,
        bindings: Arc<dyn ConversationBindings>,
    ) -> Self {
        self.peers = Some(peers);
        self.bindings = Some(bindings);
        self
    }

    /// These environments, with commands granted as `policy` answers: the
    /// command policy composition built over the gateway's configuration.
    #[cfg_attr(not(unix), allow(dead_code, reason = "the Unix gateway composes it"))]
    pub(crate) fn with_commands(mut self, policy: Arc<dyn CommandPolicyEvaluator>) -> Self {
        self.commands = Some(policy);
        self
    }

    /// These environments, whose `artifacts_publish` calls wait `wait`
    /// rather than [`PUBLISH_CALL_WAIT`]. For tests.
    #[cfg(test)]
    pub(crate) fn with_publish_wait(mut self, wait: Duration) -> Self {
        self.publish_wait = wait;
        self
    }

    /// How long an `artifacts_publish` call waits for its file to be kept.
    pub(crate) fn publish_wait(&self) -> Duration {
        self.publish_wait
    }

    /// These environments, whose `artifacts_fetch` calls wait `wait` rather
    /// than [`FETCH_CALL_WAIT`]. For tests.
    #[cfg(test)]
    pub(crate) fn with_fetch_wait(mut self, wait: Duration) -> Self {
        self.fetch_wait = wait;
        self
    }

    /// How long an `artifacts_fetch` call waits for its file to be placed.
    pub(crate) fn fetch_wait(&self) -> Duration {
        self.fetch_wait
    }

    /// Who may run which commands, and where; `None` when this gateway
    /// grants none.
    pub(crate) fn command_policy(&self) -> Option<&Arc<dyn CommandPolicyEvaluator>> {
        self.commands.as_ref()
    }

    /// The configured hosts, by the destination each is named by.
    pub(crate) fn hosts(&self) -> &BTreeMap<String, Arc<dyn Environment>> {
        &self.hosts
    }

    /// Whether a conversation can be placed on `peer:any` now: some paired
    /// environment serves and is not draining.
    pub(crate) fn pool_serves(&self) -> bool {
        self.peers
            .as_ref()
            .is_some_and(|peers| least_loaded(&peers.serving(), &[]).is_some())
    }

    /// Whether `worker` takes no new lease now: its connection is gone, or
    /// it said it is draining.
    pub(crate) fn left_the_pool(&self, worker: &PeerEnvironment) -> bool {
        self.peers.as_ref().is_none_or(|peers| {
            !peers
                .serving()
                .iter()
                .any(|(peer, load)| peer == worker && !load.draining)
        })
    }

    /// The paired environments connected now, by name.
    pub(crate) fn connected_peers(&self) -> Vec<(String, Arc<dyn Environment>)> {
        self.peers.as_ref().map_or_else(Vec::new, |peers| {
            peers
                .connected()
                .iter()
                .map(|peer| (peer.name(), peers.environment(peer)))
                .collect()
        })
    }

    /// The environment away from here that `name` names: a configured host,
    /// or a paired environment, connected or not. The pool names no one
    /// environment.
    pub(crate) fn named(&self, name: &str) -> Option<Arc<dyn Environment>> {
        if name == POOL {
            return None;
        }
        match PeerEnvironment::parse(name) {
            Ok(peer) => self.peers.as_ref().map(|peers| peers.environment(&peer)),
            Err(_) => self.hosts.get(name).cloned(),
        }
    }

    /// Record where a conversation about to be created runs: on `requested`,
    /// which must be a configured host or a connected paired environment,
    /// or here.
    pub(crate) async fn place(
        &self,
        conversation: &ConversationId,
        requested: Option<&str>,
    ) -> Result<(), ConversationError> {
        if let Some(name) = requested {
            let known = name == BOAT && self.workspaces.is_some()
                || name == POOL && self.bindings.is_some() && self.pool_serves()
                || match PeerEnvironment::parse(name) {
                    Ok(peer) => self
                        .peers
                        .as_ref()
                        .is_some_and(|peers| peers.connected().contains(&peer)),
                    Err(_) => self.hosts.contains_key(name),
                };
            if !known {
                return Err(ConversationError::EnvironmentNotConfigured);
            }
        }
        match &self.placements {
            Some(placements) => Ok(placements.place(conversation, requested).await?),
            None if requested.is_none() => Ok(()),
            None => Err(ConversationError::EnvironmentNotConfigured),
        }
    }

    /// The host `conversation` was placed on, `None` for here.
    pub(crate) async fn placement(
        &self,
        conversation: &ConversationId,
    ) -> Result<Option<String>, ConversationError> {
        match &self.placements {
            Some(placements) => Ok(placements.placement(conversation).await?),
            None => Ok(None),
        }
    }

    /// Whether `conversation` runs in a cloud workspace of its own
    /// ([`BOAT`]), whose VM rests while its agent does
    /// ([`Environments::rest`]).
    pub(crate) async fn in_workspace(&self, conversation: &ConversationId) -> bool {
        self.workspaces.is_some()
            && matches!(self.placement(conversation).await, Ok(Some(name)) if name == BOAT)
    }

    /// Stop `conversation`'s VM, keeping its disk, once its agent has
    /// stopped and nothing else uses the VM; the next message resumes it.
    /// Nothing for a conversation that runs anywhere else.
    pub(crate) async fn rest(&self, conversation: &ConversationId) {
        if !self.in_workspace(conversation).await {
            return;
        }
        if let Some(workspaces) = &self.workspaces {
            if workspaces.rest(conversation).await {
                tracing::info!(conversation_id = %conversation, "stopped the conversation's VM; its next message resumes it");
            }
        }
    }

    /// The environment `conversation` runs on now, by name, as the
    /// environment tools name it: its placement, but for a `peer:any` one the
    /// worker it is bound to, and `peer:any` itself, which names no
    /// environment, before it is bound to any; `None` for here.
    pub(crate) async fn runs_on(
        &self,
        conversation: &ConversationId,
    ) -> Result<Option<String>, ConversationError> {
        match self.placement(conversation).await? {
            Some(name) if name == POOL => {
                let (_, bindings) = self.pool()?;
                Ok(Some(
                    bindings
                        .binding(conversation)
                        .await?
                        .map_or(name, |binding| binding.worker.name()),
                ))
            }
            placed => Ok(placed),
        }
    }

    /// The environment `conversation` runs in: for a `peer:any` one, the
    /// worker it is bound to, refused [`ConversationError::Unavailable`]
    /// before it was bound to any.
    pub(crate) async fn of(
        &self,
        conversation: &ConversationId,
    ) -> Result<Arc<dyn Environment>, ConversationError> {
        match self.placement(conversation).await? {
            None => Ok(self.here.clone()),
            Some(name) if name == BOAT => self
                .workspaces
                .as_ref()
                .map(|factory| factory.environment(conversation))
                .ok_or(ConversationError::EnvironmentNotConfigured)?,
            Some(name) if name == POOL => {
                let (peers, bindings) = self.pool()?;
                let binding = bindings
                    .binding(conversation)
                    .await?
                    .ok_or(ConversationError::Unavailable)?;
                Ok(peers.environment(&binding.worker))
            }
            Some(name) => self
                .named(&name)
                .ok_or(ConversationError::EnvironmentNotConfigured),
        }
    }

    /// The pool's workers and where their bindings are kept.
    fn pool(&self) -> Result<Pool<'_>, ConversationError> {
        self.peers
            .as_ref()
            .zip(self.bindings.as_ref())
            .ok_or(ConversationError::EnvironmentNotConfigured)
    }

    /// Where `conversation`'s agent lease is asked now. For a `peer:any`
    /// conversation, the worker it is bound to while that one serves and is
    /// not draining, else the least loaded that does and is not in `avoid`,
    /// bound before this answers; with none, the worker it was bound to,
    /// whose lease is then refused unreachable as any absent worker's is,
    /// or, never bound, [`ConversationError::Unavailable`].
    ///
    /// Asked only when its agent is opened, so only while no lease of it is
    /// live: the worker it leaves holds nothing of it.
    pub(crate) async fn for_lease(
        &self,
        conversation: &ConversationId,
        avoid: &[PeerEnvironment],
    ) -> Result<LeasePlace, ConversationError> {
        if self.placement(conversation).await?.as_deref() != Some(POOL) {
            return Ok(LeasePlace {
                environment: self.of(conversation).await?,
                context: None,
                _reservation: None,
            });
        }
        let (peers, bindings) = self.pool()?;
        let bound = bindings.binding(conversation).await?;
        let stays = |serving: &[(PeerEnvironment, WorkerLoad)], bound: &WorkerBinding| {
            !avoid.contains(&bound.worker)
                && serving
                    .iter()
                    .any(|(peer, load)| *peer == bound.worker && !load.draining)
        };
        // Choosing and reserving are one step, so openings choosing at once
        // spread over the workers.
        let reserved = self.reserved.reserve(peers.serving(), |serving| {
            match bound.as_ref().filter(|bound| stays(serving, bound)) {
                Some(bound) => Some(bound.worker),
                None => least_loaded(serving, avoid),
            }
        });
        let (binding, reservation) = match reserved {
            Some((worker, reservation)) => match &bound {
                Some(bound) if bound.worker == worker => (bound.clone(), Some(reservation)),
                _ => {
                    let binding = WorkerBinding {
                        worker,
                        // What it held elsewhere is not on this worker.
                        context: None,
                    };
                    // A binding not written lets the reservation go.
                    bindings.bind(conversation, &binding).await?;
                    tracing::info!(
                        conversation_id = %conversation,
                        from = bound.as_ref().map(|bound| bound.worker.name()),
                        to = worker.name(),
                        "a conversation placed on any worker is bound to one"
                    );
                    (binding, Some(reservation))
                }
            },
            None => (bound.ok_or(ConversationError::Unavailable)?, None),
        };
        Ok(LeasePlace {
            environment: peers.environment(&binding.worker),
            _reservation: reservation,
            context: Some(Arc::new(WorkerContext {
                conversation: conversation.clone(),
                bindings: bindings.clone(),
                binding: std::sync::Mutex::new(binding),
            })),
        })
    }

    /// Whether `conversation` runs where this machine's files are its own,
    /// so a path on this machine names the same file there: only here. A
    /// host has files of its own, and would read whatever it has at that
    /// path, or nothing.
    pub(crate) async fn reads_this_machine(
        &self,
        conversation: &ConversationId,
    ) -> Result<bool, ConversationError> {
        // Any worker of the pool is another machine, bound yet or not.
        if self.placement(conversation).await?.as_deref() == Some(POOL) {
            return Ok(false);
        }
        Ok(matches!(
            self.of(conversation).await?.declaration().environment,
            EnvironmentRef::Here
        ))
    }

    /// Forget where `conversation` ran, and which worker held it.
    pub(crate) async fn erase(
        &self,
        conversation: &ConversationId,
    ) -> Result<(), ConversationError> {
        if let Some(bindings) = &self.bindings {
            bindings.erase(conversation).await?;
        }
        match &self.placements {
            Some(placements) => Ok(placements.erase(conversation).await?),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/pool.rs"]
mod pool_tests;
