//! Where each conversation runs: in this process, on an SSH host, on a
//! paired environment, on any of them, or in a cloud VM of its own, named
//! when it was created (issues #699, #706, #791).
//!
//! ```text
//! create(environment?) ──▶ Environments::place ──▶ ConversationPlaces::write
//!                            (refused unless the host is configured, (its one record,
//!                             or the paired environment connected,   before the
//!                             or, for peer:any, some worker serves   conversation's)
//!                             and is not draining)
//! start_slot ──▶ Environments::for_lease(conversation) ──▶ ConversationPlaces::read
//!   ──▶ here | that host | that peer | its cloud VM
//!   | peer:any ──▶ the worker the record binds it to,
//!       while that one serves and is not draining
//!     otherwise ──▶ PoolSettingsStore::read (the conversation's channel, or
//!       the Nessa default) ──▶ of the serving, not draining workers it does
//!       not leave out, the first in its order that is not busy, fewest
//!       agent leases among equals; every one busy, the first in order
//!       ──▶ ConversationPlaces::write (written before the lease is asked)
//!   the lease's fence restores the saved provider context only when the
//!   record says this worker opened it (WorkerContext); the context each
//!   session opens there is written back to the record
//! delete ──▶ Environments::erase ──▶ ConversationPlaces::erase
//! ```
//!
//! Arrows are calls, in order. A conversation has one place record
//! ([`PlaceRecord`]): where it was placed, and for a `peer:any` one, which
//! worker holds it now and what provider context it opened there. The
//! record is written before the conversation's record exists, so a
//! conversation never exists without knowing where it runs. Where it was
//! placed never changes; only a pool conversation's worker does. A
//! conversation with no record runs here, as one made before records were
//! kept for here does. A record this build cannot read refuses that
//! conversation's opening alone (ADR 202, record scope). A placement naming a
//! host the configuration no longer names is refused the same way, never run
//! somewhere else. A paired environment is placed on only while it is
//! connected, so a name nobody serves is never kept; once placed, the
//! conversation waits for it to dial in again, and its key's credential is
//! asked before every lease.
//!
//! `peer:any` places a conversation on this gateway's pool: every paired
//! environment that holds a credential to serve it, which is the registry's
//! fact and is kept nowhere here. Which worker holds the conversation now is
//! chosen when its agent's lease is asked, and kept while that worker
//! serves and is not draining. A conversation moves only when its agent is
//! opened, which is only while no lease of it is live, so one worker holds
//! it at a time. Provider sessions stay on the worker that opened them, so
//! a moved conversation's agent opens new context there.
use super::place_decision::{pool_choice, PoolNow};
use super::{ConversationError, Environment, EnvironmentFuture, PoolSettingsStore};
use crate::conversation::domain::{ChannelId, Place, PoolPreference, PoolSettings, SettingsScope};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::leases::{EnvironmentRef, LeaseEndCause, PeerEnvironment};

/// Why a conversation's cloud VM rests: its agent's lease ended in one of
/// the two ways that leave nothing for the VM to do until the next message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceRest {
    /// The agent sat idle and was parked.
    Idle,
    /// The agent closed its thread.
    Closed,
}
impl WorkspaceRest {
    /// Whether a lease that ended for `cause` lets its VM rest, and why.
    /// Any other end (a stop, a revocation, a lost connection) leaves the
    /// VM to its own time limit.
    pub(crate) fn of(cause: LeaseEndCause) -> Option<Self> {
        match cause {
            LeaseEndCause::Idle => Some(Self::Idle),
            LeaseEndCause::Closed => Some(Self::Closed),
            _ => None,
        }
    }
}
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::watch;

/// When a computer counts as busy where no pool preferences are kept: at
/// 90% processor or memory use, as an absent `pool` section says.
const UNCONFIGURED_BUSY_PERMILLE: u32 = 900;

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

/// Which worker of the pool holds a `peer:any` conversation now, and the
/// provider context its agent last opened there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkerBinding {
    pub(crate) worker: PeerEnvironment,
    /// The provider context a session of this conversation last opened on
    /// `worker`; `None` until one has. Only this one is restored there.
    pub(crate) context: Option<ExecutionSessionId>,
}

/// Where one conversation runs: the place it was placed on, which never
/// changes, and for a `peer:any` conversation the worker that holds it now.
/// Only a pool conversation is ever bound to a worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlaceRecord {
    placed: Place,
    worker: Option<WorkerBinding>,
}

impl PlaceRecord {
    /// A conversation placed on `placed`, bound to no worker yet.
    pub(crate) fn placed(placed: Place) -> Self {
        Self {
            placed,
            worker: None,
        }
    }

    /// A `peer:any` conversation that `worker` holds now.
    pub(crate) fn in_pool(worker: WorkerBinding) -> Self {
        Self {
            placed: Place::Pool,
            worker: Some(worker),
        }
    }

    /// A conversation placed on `placed` and, when that is the pool, bound to
    /// `worker`; a worker named for any other place is a record this build
    /// cannot read.
    ///
    /// # Errors
    /// [`PlacementError::Unreadable`] for a worker beside a place that is
    /// not the pool.
    pub(crate) fn read(
        placed: Place,
        worker: Option<WorkerBinding>,
    ) -> Result<Self, PlacementError> {
        match (placed, worker) {
            (Place::Pool, worker) => Ok(Self {
                placed: Place::Pool,
                worker,
            }),
            (placed, None) => Ok(Self::placed(placed)),
            (_, Some(_)) => Err(PlacementError::Unreadable),
        }
    }

    /// Where it was placed when it was made.
    pub(crate) fn place(&self) -> &Place {
        &self.placed
    }

    /// The worker of the pool that holds it now; `None` before its first
    /// lease, and for a conversation placed anywhere but the pool.
    pub(crate) fn worker(&self) -> Option<&WorkerBinding> {
        self.worker.as_ref()
    }

    /// Where it runs now, as the environment tools name it: where it was
    /// placed, but for a pool one the worker that holds it, and
    /// [`Place::Pool`] itself, which names no environment, before any does.
    pub(crate) fn now(&self) -> Place {
        match &self.worker {
            Some(binding) => Place::Peer(binding.worker),
            None => self.placed.clone(),
        }
    }
}

/// The durable record of where each conversation runs. Implemented in
/// infrastructure.
pub(crate) trait ConversationPlaces: Send + Sync {
    /// Record `record` for `conversation`, in place of whatever it had: before
    /// the conversation exists, and each time a pool conversation's worker or
    /// provider context changes. Durable before it answers.
    fn write<'a>(
        &'a self,
        conversation: &'a ConversationId,
        record: &'a PlaceRecord,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>>;
    /// Where `conversation` runs: placed [`Place::Here`] when it has no
    /// record.
    fn read<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<PlaceRecord, PlacementError>>;
    /// Forget where `conversation` runs, once its history is erased.
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

/// How busy a paired environment's machine last said it is, each in
/// tenths of a percent, 0 to 1000.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MachineUse {
    /// Processor use across all its processors.
    pub(crate) cpu_permille: u32,
    /// Memory in use, of what is installed.
    pub(crate) memory_permille: u32,
}

/// One connected paired environment as the pool sees it now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PoolStanding {
    /// How busy its machine last said it is; `None` before it has said.
    pub(crate) machine: Option<MachineUse>,
    /// Agents it holds a lease for; `None` when its connection serves none.
    pub(crate) agents: Option<usize>,
    /// Its place for new `peer:any` work, 0 first; `None` when it takes
    /// none: left out, draining, or not serving.
    pub(crate) rank: Option<usize>,
    /// Whether it is at or past a busy limit, so the pool passes it over
    /// while another is not.
    pub(crate) busy: bool,
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
    /// How busy `peer`'s machine last said it is; `None` before it has
    /// said, or for one that never does, which is never busy.
    fn machine_use(&self, _peer: &PeerEnvironment) -> Option<MachineUse> {
        None
    }
}

/// The workers of `serving` that may take a new lease under `preference`:
/// not draining, not in `avoid`, and not left out.
fn open<'a>(
    serving: &'a [(PeerEnvironment, WorkerLoad)],
    avoid: &'a [PeerEnvironment],
    preference: &'a PoolPreference,
) -> impl Iterator<Item = &'a (PeerEnvironment, WorkerLoad)> + 'a {
    serving.iter().filter(move |(peer, load)| {
        !load.draining && !avoid.contains(peer) && preference.includes(peer)
    })
}

/// Whether `peer`'s machine is too busy for new work under `settings`.
fn busy(peers: &dyn PairedEnvironments, settings: &PoolSettings, peer: &PeerEnvironment) -> bool {
    let used = peers.machine_use(peer);
    settings.busy(
        used.map(|used| used.cpu_permille),
        used.map(|used| used.memory_permille),
    )
}

/// The worker of `serving` a new lease goes to: of those [`open`] finds,
/// the first in the preference's order that is not busy, the fewest leases
/// of an agent among equals, then the lowest key. When every one is busy,
/// the same choice among all of them: busy is a reason to go elsewhere,
/// never a reason not to run.
fn choose(
    peers: &dyn PairedEnvironments,
    serving: &[(PeerEnvironment, WorkerLoad)],
    avoid: &[PeerEnvironment],
    settings: &PoolSettings,
) -> Option<PeerEnvironment> {
    let preference = settings.effective();
    let order =
        |(peer, load): &&(PeerEnvironment, WorkerLoad)| (preference.rank(peer), load.leases, *peer);
    open(serving, avoid, preference)
        .filter(|(peer, _)| !busy(peers, settings, peer))
        .min_by_key(order)
        .or_else(|| open(serving, avoid, preference).min_by_key(order))
        .map(|(peer, _)| *peer)
}

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
/// context a session opens there, which it writes to the place record.
pub(crate) struct WorkerContext {
    conversation: ConversationId,
    places: Arc<dyn ConversationPlaces>,
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

    /// A session opened `context` on the worker: kept in the place record,
    /// so the next agent there restores it. Written before the opening
    /// answers, so it lands before anything can move or erase the
    /// conversation; a write that fails costs only that context, opened anew
    /// next time, never a restore of what the worker does not hold.
    pub(crate) async fn opened(&self, context: &ExecutionSessionId) {
        let binding = {
            let mut binding = self.lock();
            if binding.context.as_ref() == Some(context) {
                return;
            }
            binding.context = Some(context.clone());
            binding.clone()
        };
        let record = PlaceRecord::in_pool(binding);
        if let Err(error) = self.places.write(&self.conversation, &record).await {
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
    /// uses it; the next opening resumes it. `cause` is how its agent's
    /// lease ended, kept with the record of the stop. Answers whether it
    /// stopped one.
    fn rest<'a>(
        &'a self,
        conversation: &'a ConversationId,
        cause: WorkspaceRest,
    ) -> EnvironmentFuture<'a, bool>;
    /// Whether a new conversation can get a workspace now: a cloud account
    /// is connected.
    fn available(&self) -> EnvironmentFuture<'_, bool>;
}
/// Bound retained workspace environments independently of saved thread history.
pub(crate) const MAX_RETAINED_ENVIRONMENTS: usize = 256;

pub(crate) struct Environments {
    workspaces: Option<Arc<dyn ThreadEnvironments>>,
    here: Arc<dyn Environment>,
    hosts: BTreeMap<String, Arc<dyn Environment>>,
    /// `None` when this gateway pairs no environments.
    peers: Option<Arc<dyn PairedEnvironments>>,
    /// Where each conversation runs: written before it exists, and again
    /// when a pool conversation's worker changes.
    places: Arc<dyn ConversationPlaces>,
    /// Pool leases chosen for a worker and not yet asked there.
    reserved: Arc<Reserved>,
    /// The default device saved from Devices: where a new conversation that
    /// names no place runs, while it is offered; `None` is here.
    default: Option<watch::Receiver<Option<String>>>,
    /// Which workers the pool tries first and which it leaves out, by
    /// channel; `None` puts every worker in, none first.
    pool_settings: Option<Arc<dyn PoolSettingsStore>>,
}

#[cfg(test)]
impl From<Arc<dyn Environment>> for Environments {
    /// Here alone: every conversation runs here, as nothing else is
    /// offered. For tests; composition always keeps place records.
    fn from(here: Arc<dyn Environment>) -> Self {
        Self::new(here, BTreeMap::new(), Arc::new(HereOnlyPlaces))
    }
}

/// A place store for tests built with [`Environments::from`], where nothing
/// but here is offered: every conversation reads as here, and anything else
/// is refused as the store being unavailable.
#[cfg(test)]
struct HereOnlyPlaces;

#[cfg(test)]
impl ConversationPlaces for HereOnlyPlaces {
    fn write<'a>(
        &'a self,
        _conversation: &'a ConversationId,
        record: &'a PlaceRecord,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>> {
        let written = match record.place() {
            Place::Here => Ok(()),
            _ => Err(PlacementError::Unavailable),
        };
        Box::pin(async move { written })
    }
    fn read<'a>(
        &'a self,
        _conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<PlaceRecord, PlacementError>> {
        Box::pin(async { Ok(PlaceRecord::placed(Place::Here)) })
    }
    fn erase<'a>(
        &'a self,
        _conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>> {
        Box::pin(async { Ok(()) })
    }
}

impl Environments {
    #[cfg(unix)]
    pub(crate) fn with_workspaces(mut self, workspaces: Arc<dyn ThreadEnvironments>) -> Self {
        self.workspaces = Some(workspaces);
        self
    }

    /// Here, the configured hosts by the destination they are named by, and
    /// where place records are kept.
    #[cfg_attr(not(unix), allow(dead_code, reason = "the Unix gateway composes it"))]
    pub(crate) fn new(
        here: Arc<dyn Environment>,
        hosts: BTreeMap<String, Arc<dyn Environment>>,
        places: Arc<dyn ConversationPlaces>,
    ) -> Self {
        Self {
            workspaces: None,
            here,
            hosts,
            peers: None,
            places,
            reserved: Arc::default(),
            default: None,
            pool_settings: None,
        }
    }

    /// These environments, with the paired environments `peers`.
    #[cfg_attr(not(unix), allow(dead_code, reason = "the Unix gateway composes it"))]
    pub(crate) fn with_peers(mut self, peers: Arc<dyn PairedEnvironments>) -> Self {
        self.peers = Some(peers);
        self
    }

    /// These environments, whose new conversations that name no place run
    /// on the device `default` holds while it is offered.
    #[cfg_attr(not(unix), allow(dead_code, reason = "the Unix gateway composes it"))]
    pub(crate) fn with_default(mut self, default: watch::Receiver<Option<String>>) -> Self {
        self.default = Some(default);
        self
    }

    /// The default device, when a conversation could be placed on it now:
    /// a configured host, or a paired environment that is connected. A
    /// paired one that is not leaves a new conversation here rather than
    /// refused, since nobody named it for that conversation.
    pub(crate) fn default_place(&self) -> Option<Place> {
        let name = self.default.as_ref()?.borrow().clone()?;
        Place::named(&name).ok().filter(|place| self.offers(place))
    }

    /// Whether `place` is a configured host or a connected paired
    /// environment.
    pub(crate) fn offers(&self, place: &Place) -> bool {
        match place {
            Place::Peer(peer) => self
                .peers
                .as_ref()
                .is_some_and(|peers| peers.connected().contains(peer)),
            Place::Host(host) => self.hosts.contains_key(host.as_str()),
            Place::Here | Place::Pool | Place::CloudVm => false,
        }
    }

    /// These environments, whose pool follows the preferences `settings`
    /// keeps: which workers go first and which are left out.
    #[cfg_attr(not(unix), allow(dead_code, reason = "the Unix gateway composes it"))]
    pub(crate) fn with_pool_settings(mut self, settings: Arc<dyn PoolSettingsStore>) -> Self {
        self.pool_settings = Some(settings);
        self
    }

    /// The pool's preferences for a conversation in `channel` (`None`: the
    /// default channel, which follows the Nessa default): every worker in,
    /// none first, when none are kept.
    ///
    /// # Errors
    /// [`ConversationError::Unavailable`] when they cannot be read: a worker
    /// that was left out is never chosen because its preference was not.
    pub(crate) async fn pool_settings(
        &self,
        channel: Option<&ChannelId>,
    ) -> Result<PoolSettings, ConversationError> {
        let scope = channel.map_or(SettingsScope::Global, |channel| {
            SettingsScope::Channel(channel.clone())
        });
        match &self.pool_settings {
            Some(settings) => settings
                .read(&scope)
                .await
                .map_err(|_| ConversationError::Unavailable),
            None => Ok(PoolSettings::new(
                scope,
                PoolPreference::default(),
                None,
                UNCONFIGURED_BUSY_PERMILLE,
                UNCONFIGURED_BUSY_PERMILLE,
            )),
        }
    }

    /// Where the pool's preferences are kept; `None` when this gateway
    /// keeps none and every worker is in, none first.
    pub(crate) fn pool_store(&self) -> Option<&Arc<dyn PoolSettingsStore>> {
        self.pool_settings.as_ref()
    }

    /// The configured hosts, by the destination each is named by.
    pub(crate) fn hosts(&self) -> &BTreeMap<String, Arc<dyn Environment>> {
        &self.hosts
    }

    /// Whether a conversation can be placed on `peer:any` now: some paired
    /// environment serves, is not draining, and is not left out by
    /// `settings`.
    pub(crate) fn pool_serves(&self, settings: &PoolSettings) -> bool {
        self.peers.as_ref().is_some_and(|peers| {
            open(&peers.serving(), &[], settings.effective())
                .next()
                .is_some()
        })
    }

    /// Whether every worker that could take a conversation on `peer:any`
    /// now is busy: there is at least one, and none is below its limits.
    fn pool_busy(&self, settings: &PoolSettings) -> bool {
        self.peers.as_ref().is_some_and(|peers| {
            let serving = peers.serving();
            let mut open = open(&serving, &[], settings.effective()).peekable();
            open.peek().is_some() && open.all(|(peer, _)| busy(peers.as_ref(), settings, peer))
        })
    }

    /// What the pool knows of each paired environment connected now, by
    /// name, under `settings`: how busy its machine said it is, how many
    /// agents it carries, and where it stands for new `peer:any` work.
    pub(crate) fn pool_standing(&self, settings: &PoolSettings) -> BTreeMap<String, PoolStanding> {
        let Some(peers) = self.peers.as_ref() else {
            return BTreeMap::new();
        };
        let serving = peers.serving();
        let preference = settings.effective();
        peers
            .connected()
            .into_iter()
            .map(|peer| {
                let load = serving
                    .iter()
                    .find(|(serving, _)| *serving == peer)
                    .map(|(_, load)| *load);
                let takes_work =
                    load.is_some_and(|load| !load.draining) && preference.includes(&peer);
                let standing = PoolStanding {
                    machine: peers.machine_use(&peer),
                    agents: load.map(|load| load.leases),
                    rank: takes_work.then(|| preference.rank(&peer)),
                    busy: busy(peers.as_ref(), settings, &peer),
                };
                (peer.name(), standing)
            })
            .collect()
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

    /// The environment away from here that `place` names: a configured
    /// host, or a paired environment, connected or not. Here, the pool and
    /// a cloud VM name no one environment by themselves.
    pub(crate) fn named(&self, place: &Place) -> Option<Arc<dyn Environment>> {
        match place {
            Place::Peer(peer) => self.peers.as_ref().map(|peers| peers.environment(peer)),
            Place::Host(host) => self.hosts.get(host.as_str()).cloned(),
            Place::Here | Place::Pool | Place::CloudVm => None,
        }
    }

    /// Record where a conversation about to be created runs: on `requested`,
    /// which must be a configured host or a connected paired environment,
    /// or here. The pool (any paired computer) asked for with none online
    /// that `channel`'s preferences let it use places it in a cloud
    /// workspace of its own instead, when a cloud account is connected; so
    /// does the pool with every such computer busy, as overflow. One
    /// computer asked for by name never moves: the person chose where it
    /// runs.
    pub(crate) async fn place(
        &self,
        conversation: &ConversationId,
        requested: &Place,
        channel: Option<&ChannelId>,
    ) -> Result<(), ConversationError> {
        let placed = match requested {
            Place::Here => Place::Here,
            Place::Pool => self.place_in_pool(conversation, channel).await?,
            Place::CloudVm if self.boat_connected().await => Place::CloudVm,
            named if self.offers(named) => named.clone(),
            _ => return Err(ConversationError::EnvironmentNotConfigured),
        };
        Ok(self
            .places
            .write(conversation, &PlaceRecord::placed(placed))
            .await?)
    }

    /// Where a conversation that asked for the pool is placed, as
    /// [`pool_choice`] decides from the pool now, under `channel`'s
    /// preferences, and whether a cloud VM can be had.
    async fn place_in_pool(
        &self,
        conversation: &ConversationId,
        channel: Option<&ChannelId>,
    ) -> Result<Place, ConversationError> {
        let pool = match self.peers {
            Some(_) => {
                let settings = self.pool_settings(channel).await?;
                if self.pool_busy(&settings) {
                    PoolNow::Busy
                } else if self.pool_serves(&settings) {
                    PoolNow::Free
                } else {
                    PoolNow::Offline
                }
            }
            None => PoolNow::Offline,
        };
        // The cloud is asked about only when the pool cannot take it.
        let cloud = pool != PoolNow::Free && self.boat_connected().await;
        let chosen = pool_choice(pool, cloud)?;
        if chosen == Place::CloudVm {
            tracing::info!(conversation_id = %conversation, asked = %Place::Pool.name(), ?pool, "the pool cannot take the conversation now; it runs in a cloud workspace");
        }
        Ok(chosen)
    }

    /// Whether a cloud workspace can be had now: this gateway builds them
    /// and a Boat account is connected. Asked before placing on [`Place::CloudVm`]
    /// and before each lease there, so a chat whose account is not
    /// connected is refused as not configured, not as an unreachable host.
    async fn boat_connected(&self) -> bool {
        match &self.workspaces {
            Some(workspaces) => workspaces.available().await,
            None => false,
        }
    }

    /// Where `conversation` runs: where it was placed and, for a pool one,
    /// the worker that holds it now. One read of its record.
    pub(crate) async fn record(
        &self,
        conversation: &ConversationId,
    ) -> Result<PlaceRecord, ConversationError> {
        Ok(self.places.read(conversation).await?)
    }

    /// Where `conversation` was placed.
    pub(crate) async fn placement(
        &self,
        conversation: &ConversationId,
    ) -> Result<Place, ConversationError> {
        Ok(self.record(conversation).await?.placed)
    }

    /// Whether `conversation` runs in a cloud workspace of its own
    /// ([`Place::CloudVm`]), whose VM rests while its agent does
    /// ([`Environments::rest`]).
    pub(crate) async fn in_workspace(&self, conversation: &ConversationId) -> bool {
        self.workspaces.is_some()
            && matches!(self.placement(conversation).await, Ok(Place::CloudVm))
    }

    /// Stop `conversation`'s VM, keeping its disk, once its agent has
    /// stopped and nothing else uses the VM; the next message resumes it.
    /// Nothing for a conversation that runs anywhere else.
    pub(crate) async fn rest(&self, conversation: &ConversationId, cause: WorkspaceRest) {
        if !self.in_workspace(conversation).await {
            return;
        }
        if let Some(workspaces) = &self.workspaces {
            if workspaces.rest(conversation, cause).await {
                tracing::info!(conversation_id = %conversation, "stopped the conversation's VM; its next message resumes it");
            }
        }
    }

    /// Where `conversation` runs now, as the environment tools name it
    /// ([`PlaceRecord::now`]).
    pub(crate) async fn runs_on(
        &self,
        conversation: &ConversationId,
    ) -> Result<Place, ConversationError> {
        let record = self.record(conversation).await?;
        if record.placed == Place::Pool {
            // A pool this gateway no longer has is not configured.
            self.pool()?;
        }
        Ok(record.now())
    }

    /// The environment `conversation` runs in: for a `peer:any` one, the
    /// worker it is bound to, refused [`ConversationError::Unavailable`]
    /// before it was bound to any.
    pub(crate) async fn of(
        &self,
        conversation: &ConversationId,
    ) -> Result<Arc<dyn Environment>, ConversationError> {
        let record = self.record(conversation).await?;
        self.environment_for(conversation, &record)
    }

    /// The environment `record` says `conversation` runs in, as
    /// [`Environments::of`] answers, without reading the record again.
    pub(crate) fn environment_for(
        &self,
        conversation: &ConversationId,
        record: &PlaceRecord,
    ) -> Result<Arc<dyn Environment>, ConversationError> {
        match &record.placed {
            Place::Here => Ok(self.here.clone()),
            Place::CloudVm => self
                .workspaces
                .as_ref()
                .map(|factory| factory.environment(conversation))
                .ok_or(ConversationError::EnvironmentNotConfigured)?,
            Place::Pool => {
                let peers = self.pool()?;
                let binding = record.worker().ok_or(ConversationError::Unavailable)?;
                Ok(peers.environment(&binding.worker))
            }
            named => self
                .named(named)
                .ok_or(ConversationError::EnvironmentNotConfigured),
        }
    }

    /// The pool's workers.
    fn pool(&self) -> Result<&Arc<dyn PairedEnvironments>, ConversationError> {
        self.peers
            .as_ref()
            .ok_or(ConversationError::EnvironmentNotConfigured)
    }

    /// Where `conversation`'s agent lease is asked now. For a `peer:any`
    /// conversation, the worker it is bound to while that one serves, is
    /// not draining and is not left out by `channel`'s preferences, else
    /// the one [`choose`] picks of those not in `avoid`,
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
        channel: Option<&ChannelId>,
    ) -> Result<LeasePlace, ConversationError> {
        let record = self.record(conversation).await?;
        if record.placed == Place::CloudVm && !self.boat_connected().await {
            return Err(ConversationError::EnvironmentNotConfigured);
        }
        if record.placed != Place::Pool {
            return Ok(LeasePlace {
                environment: self.environment_for(conversation, &record)?,
                context: None,
                _reservation: None,
            });
        }
        let peers = self.pool()?;
        let settings = self.pool_settings(channel).await?;
        let bound = record.worker;
        // A conversation stays on its worker while that one may take it,
        // busy or not: moving costs it the provider context held there.
        let stays = |serving: &[(PeerEnvironment, WorkerLoad)], bound: &WorkerBinding| {
            open(serving, avoid, settings.effective()).any(|(peer, _)| *peer == bound.worker)
        };
        // Choosing and reserving are one step, so openings choosing at once
        // spread over the workers.
        let reserved = self.reserved.reserve(peers.serving(), |serving| {
            match bound.as_ref().filter(|bound| stays(serving, bound)) {
                Some(bound) => Some(bound.worker),
                None => choose(peers.as_ref(), serving, avoid, &settings),
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
                    // A record not written lets the reservation go.
                    self.places
                        .write(conversation, &PlaceRecord::in_pool(binding.clone()))
                        .await?;
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
                places: self.places.clone(),
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
        let record = self.record(conversation).await?;
        // Any worker of the pool is another machine, bound yet or not.
        if record.placed == Place::Pool {
            return Ok(false);
        }
        Ok(matches!(
            self.environment_for(conversation, &record)?
                .declaration()
                .environment,
            EnvironmentRef::Here
        ))
    }

    /// Forget where `conversation` ran, and which worker held it.
    pub(crate) async fn erase(
        &self,
        conversation: &ConversationId,
    ) -> Result<(), ConversationError> {
        Ok(self.places.erase(conversation).await?)
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/pool.rs"]
mod pool_tests;
