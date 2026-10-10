//! Reading what each peer granted this gateway, on a cadence: one task,
//! one peer at a time, each cycle holding the peer commands' turn so no
//! enrollment or forget runs beside it. An owner command that wants the turn
//! stops the cycle instead of waiting for it.
//!
//! ```text
//! tick --> PeerRecords::list (schedules) --> each due peer:
//!   take the turn --> re-read its record; gone or revoked --> drop it
//!   pinned status (NativeEnrollmentClient over its PeerSlot)
//!     Active   --> the client saves the credential --> RetainedCache::read
//!                    Refused                    --> pinned status again
//!                    ResetRequired, CacheDamaged --> empty the cache, read once more
//!     Terminal or Unclaimed --> the client removes the cache and marks the
//!                               record revoked; the peer is not read again
//!     otherwise (still pending) --> wait
//!   every change above, as it is made --> PeerCommands::poller_changed
//!                                          (queued by the audit at once)
//!   give back the turn --> PeerCommands::poller_kept (waits for the audit)
//! ```
//! Arrows are calls, in order. No read happens without a fresh Active
//! status, and nothing is removed without a Terminal one: a refused
//! `openProduct` is redacted, so a revoked credential and a full pool look
//! alike until the status is asked (design row PC5, which the device follows
//! too). Each read has a budget on the injected clock and ends incomplete
//! when it runs out having saved something; one that saved nothing is a
//! failure. Each peer is read again after the interval, with
//! jitter, sooner when its last read was incomplete or stopped, and after a
//! failure, after a backoff that doubles up to its cap. Every wait, and the
//! read budget, is on the peer commands' injected monotonic clock, and the
//! jitter is drawn from injected entropy.
//!
//! Each change the poller makes to what is held of a peer goes through one
//! step that reads what is held before and after it: the status step
//! (credential saved, enrollment ended), the cache reset, and the
//! conversations a read withdrew. Each step hands what it found to
//! `poller_changed` while the cycle still holds the turn, and the audit takes
//! the record at once, so the evidence is in the order the turn was held:
//! a change the cycle made comes before the owner command that stopped it.
//! The cycle waits for the audit's answers only once it has given back the
//! turn, so an audit that is slow or stalled never holds the turn.
use super::audit::AUDIT_QUEUE;
use super::commands::{
    CycleStop, CycleTurn, EnrollmentEntropySource, PeerCommands, PeerSync, PollerRecord, SyncState,
};
use super::records::{PeerEntry, PeerPhase, PeerRecord, SlotOutcome, SlotTransition};
use crate::peer_gateways::application::{PollerCause, WITHDRAWN_PER_RECORD};
use nessa_auth::{
    adapters::pairing::NativeIdentity,
    application::{pairing::ClientPendingStore, ports::Clock as WallClock},
    domain::pairing::DeviceKey,
};
use nessa_client_core::pairing::{NativeClientError, NativeEnrollmentClient};
use nessa_client_core::retained::{ReadFailure, ReadReport, ReaderAccess, RetainedCache};
use nessa_protocol::pairing::socket::WAKE_TICK;
use nessa_protocol::pairing::wire::NativePairingStatus;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};
use tokio::{
    sync::watch,
    task::{JoinError, JoinHandle},
};

/// How often each peer is read once it is up to date.
pub const POLL_INTERVAL: Duration = Duration::from_secs(30);
/// The longest a peer waits between reads, failing or not, jitter included.
pub const POLL_BACKOFF_CAP: Duration = Duration::from_secs(15 * 60);
/// The longest one read of a peer runs before it ends incomplete.
pub const READ_BUDGET: Duration = Duration::from_secs(60);
/// The most conversations one read withdraws: one record per
/// [`WITHDRAWN_PER_RECORD`], so a cycle's records (two status steps, a
/// reset, and the two reads around it) fill at most about half of the
/// audit's queue ([`AUDIT_QUEUE`]), and an owner command's record has room
/// behind them while the store keeps up (asserted when the gateway is built).
/// A read that reaches it ends incomplete and the next cycle, soon,
/// withdraws the rest.
pub const WITHDRAWN_PER_READ: usize = AUDIT_QUEUE / 4 * WITHDRAWN_PER_RECORD;
/// The most records one cycle hands the audit.
const RECORDS_PER_CYCLE: usize = 3 + 2 * WITHDRAWN_PER_READ.div_ceil(WITHDRAWN_PER_RECORD);
const _: () = assert!(RECORDS_PER_CYCLE <= AUDIT_QUEUE / 2 + 3);
/// How this gateway names itself to a peer's product session.
const CLIENT_ID: &str = "nessa-peer-gateway";

/// The poller's cadence.
#[derive(Clone, Copy, Debug)]
pub struct PollPolicy {
    /// The wait between reads of a peer that is up to date.
    pub interval: Duration,
    /// The longest wait between reads, after failures and jitter.
    pub backoff_cap: Duration,
    /// The longest one read runs, by the injected clock.
    pub read_budget: Duration,
}
impl Default for PollPolicy {
    fn default() -> Self {
        Self {
            interval: POLL_INTERVAL,
            backoff_cap: POLL_BACKOFF_CAP,
            read_budget: READ_BUDGET,
        }
    }
}

/// What the poller is given: its cadence, the wall clock that stamps a
/// finished read, and the entropy its jitter is drawn from. Every wait and
/// the read budget are measured on the peer commands' monotonic clock, the
/// one their deadlines and [`super::PREEMPT`] use, so one clock bounds both
/// sides of the turn.
pub struct PollInputs {
    pub policy: PollPolicy,
    pub wall: Arc<dyn WallClock>,
    pub entropy: EnrollmentEntropySource,
}

/// When a peer is next read, and why.
struct Due {
    /// By the injected monotonic clock.
    at: u64,
    failures: u32,
}

/// The running poller, and how to stop it.
pub struct PeerPoller {
    stop: watch::Sender<bool>,
    commands: Arc<PeerCommands>,
    task: JoinHandle<()>,
}
impl PeerPoller {
    /// Start reading `commands`' peers with `inputs`.
    pub fn start(commands: Arc<PeerCommands>, inputs: PollInputs) -> Self {
        let (stop, stopped) = watch::channel(false);
        let poller = Poller {
            commands: commands.clone(),
            inputs,
            stopped,
            due: HashMap::new(),
        };
        Self {
            stop,
            commands,
            task: tokio::spawn(poller.run()),
        }
    }
    /// Ask the poller to stop: its waits end, and a cycle in progress is
    /// stopped, its read's sockets shut at once.
    pub fn signal_stop(&self) {
        self.stop.send_replace(true);
        self.commands.stop_cycle();
    }
    /// Stop, and wait until nothing the poller started is running: its task,
    /// and the blocking read or status worker it was awaiting.
    pub async fn join(self) {
        self.signal_stop();
        let _ = self.task.await;
    }
}

struct Poller {
    commands: Arc<PeerCommands>,
    inputs: PollInputs,
    stopped: watch::Receiver<bool>,
    due: HashMap<DeviceKey, Due>,
}
impl Poller {
    fn stopping(&self) -> bool {
        *self.stopped.borrow()
    }
    fn now(&self) -> u64 {
        self.commands.clock.elapsed_ms()
    }
    async fn run(mut self) {
        while !self.stopping() {
            let now = self.now();
            let listed = self.peers().await;
            for key in listed.iter().flatten().copied() {
                if self.stopping() {
                    return;
                }
                let due = self.due.entry(key).or_insert(Due {
                    at: now,
                    failures: 0,
                });
                if due.at > now {
                    continue;
                }
                let Some(pace) = self.cycle(key).await else {
                    return;
                };
                self.reschedule(key, pace);
            }
            let next = next_wake(
                listed.is_some(),
                self.due.values().map(|due| due.at),
                self.now(),
                self.inputs.policy.interval,
            );
            if !self.wait_until(next).await {
                return;
            }
        }
    }

    /// Wait until the injected clock reaches `at`, reading it every
    /// [`WAKE_TICK`]. `false` once the poller is stopped.
    async fn wait_until(&self, at: u64) -> bool {
        let mut stopped = self.stopped.clone();
        loop {
            let left = at.saturating_sub(self.now());
            if left == 0 {
                return true;
            }
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(left).min(WAKE_TICK)) => {}
                () = until_stopped(&mut stopped) => return false,
            }
        }
    }

    /// The peers to schedule: those whose records read and are not revoked;
    /// `None` when the records could not be listed. Who is read is decided
    /// again under the turn.
    async fn peers(&mut self) -> Option<Vec<DeviceKey>> {
        let records = self.commands.records().clone();
        let listed = tokio::task::spawn_blocking(move || records.list()).await;
        let Ok(Ok(entries)) = listed else {
            tracing::warn!("peer gateway records could not be listed");
            return None;
        };
        let live: Vec<DeviceKey> = entries
            .iter()
            .filter_map(|entry| match entry {
                PeerEntry::Readable(record) if record.phase() != &PeerPhase::Revoked => {
                    Some(*record.key())
                }
                _ => None,
            })
            .collect();
        self.due.retain(|key, _| live.contains(key));
        self.commands.keep_syncs(&live);
        Some(live)
    }

    fn reschedule(&mut self, key: DeviceKey, pace: Pace) {
        if pace == Pace::Gone {
            self.due.remove(&key);
            return;
        }
        let draw = draw(&self.inputs.entropy);
        let now = self.now();
        let Some(due) = self.due.get_mut(&key) else {
            return;
        };
        let (wait, failures) = next_wait(&self.inputs.policy, pace, due.failures, draw);
        due.failures = failures;
        due.at = now.saturating_add(millis(wait));
    }

    /// The turn for `cycle`, asked for every [`WAKE_TICK`] until free.
    /// `None` once the poller is stopped.
    async fn take_turn(&self, cycle: &Arc<CycleStop>) -> Option<CycleTurn> {
        let mut stopped = self.stopped.clone();
        loop {
            if let Some(turn) = self.commands.try_cycle_turn(cycle) {
                // A stop asked before the cycle was registered did not reach it.
                return (!self.stopping()).then_some(turn);
            }
            tokio::select! {
                () = tokio::time::sleep(WAKE_TICK) => {}
                () = until_stopped(&mut stopped) => return None,
            }
        }
    }

    /// One peer: under the turn, its record read again, the pinned status,
    /// then a read if it is Active, each change handed to the audit as it is
    /// made; then, with the turn given back, the audit's answers. `None` when
    /// the poller stopped.
    async fn cycle(&self, key: DeviceKey) -> Option<Pace> {
        let cycle = CycleStop::new();
        let turn = self.take_turn(&cycle).await?;
        let mut changes = Vec::new();
        let pace = self.held(key, &cycle, &mut changes).await;
        // Given back before the answers are awaited: an audit, however slow,
        // never holds the turn. The changes stand either way.
        drop(turn);
        self.commands.poller_kept(changes).await;
        pace
    }

    /// The part of a cycle that holds the turn. Each change it makes is
    /// handed to the audit at once and its answer pushed to `changes`.
    async fn held<'a>(
        &'a self,
        key: DeviceKey,
        cycle: &Arc<CycleStop>,
        changes: &mut Vec<PollerRecord<'a>>,
    ) -> Option<Pace> {
        // Forgotten, ended or replaced since the listing: what is read is
        // what the record says now, under the turn.
        let records = self.commands.records().clone();
        let record = match tokio::task::spawn_blocking(move || records.get(&key)).await {
            Ok(Ok(Some(PeerEntry::Readable(record)))) if record.phase() != &PeerPhase::Revoked => {
                record
            }
            Ok(Ok(_)) => {
                self.commands.set_sync(key, None);
                return Some(Pace::Gone);
            }
            _ => return Some(self.unread(key, &Unread::Storage)),
        };
        let Some(status) = self.status(&record, cycle, changes).await else {
            return self.stopped_short();
        };
        let pace = match status {
            Err(error) => self.unread(key, &Unread::Status(error)),
            Ok(Status::Ended) => {
                self.commands.set_sync(key, None);
                Pace::Gone
            }
            Ok(Status::Waiting) => {
                self.commands.set_sync(
                    key,
                    Some(PeerSync {
                        state: SyncState::Waiting,
                        last_synced_ms: None,
                        conversations: None,
                    }),
                );
                Pace::Waiting
            }
            Ok(Status::Active {
                receiver,
                access_epoch,
            }) => {
                let mut result = self
                    .read(&record, &receiver, access_epoch, cycle, changes)
                    .await;
                if let Some(cause) = match &result {
                    Some(Err(ReadFailure::ResetRequired)) => Some(PollerCause::ResetRequired),
                    Some(Err(ReadFailure::CacheDamaged)) => Some(PollerCause::CacheDamaged),
                    _ => None,
                } {
                    // Derived from the peer: emptied and read again from nothing.
                    self.reset(key, cause, changes).await;
                    result = self
                        .read(&record, &receiver, access_epoch, cycle, changes)
                        .await;
                }
                match result {
                    None => return self.stopped_short(),
                    Some(Ok(report)) => self.read_done(key, report),
                    Some(Err(ReadFailure::Refused)) => {
                        match self.status(&record, cycle, changes).await {
                            None => return self.stopped_short(),
                            Some(Ok(Status::Ended)) => {
                                self.commands.set_sync(key, None);
                                Pace::Gone
                            }
                            Some(_) => self.unread(key, &Unread::Read(ReadFailure::Refused)),
                        }
                    }
                    Some(Err(ReadFailure::Stopped)) => return self.stopped_short(),
                    Some(Err(failure)) => self.unread(key, &Unread::Read(failure)),
                }
            }
        };
        Some(pace)
    }

    /// A cycle cut short: by the poller stopping (`None`), or by an owner
    /// command taking the turn, after which the peer is read again soon.
    fn stopped_short(&self) -> Option<Pace> {
        (!self.stopping()).then_some(Pace::Stopped)
    }

    /// The peer's pinned status through its record, as one audited step:
    /// what is held before and after it, and the change if the status began
    /// one, its outcome as the record's store reports it. `None` when the
    /// cycle was stopped.
    async fn status<'a>(
        &'a self,
        record: &PeerRecord,
        cycle: &Arc<CycleStop>,
        changes: &mut Vec<PollerRecord<'a>>,
    ) -> Option<Result<Status, NativeClientError>> {
        let key = *record.key();
        let before = self.commands.holding(key).await;
        let (status, transition, landed) = self.ask_status(record, cycle).await;
        let after = self.commands.holding(key).await;
        let outcome = status_outcome(landed);
        match status_cause(transition, status.as_ref()) {
            // A transition that changed nothing and failed nowhere is no change.
            Some(cause) if after != before || outcome.is_err() => changes.push(
                self.commands
                    .poller_changed(key, cause, before, after, outcome),
            ),
            Some(_) => {}
            None if after != before => {
                tracing::warn!(peer = %hex(&key), before = ?before, after = ?after,
                    "peer gateway holding read differently around a status that began no change");
            }
            None => {}
        }
        Some(status?.map(|(status, _)| status))
    }

    /// The status, the transition its store began, if any, and whether that
    /// transition's writes landed.
    async fn ask_status(
        &self,
        record: &PeerRecord,
        cycle: &Arc<CycleStop>,
    ) -> (
        Option<Result<(Status, String), NativeClientError>>,
        Option<SlotTransition>,
        Option<SlotOutcome>,
    ) {
        let mut stopped = self.stopped.clone();
        // The same connect as an enrollment's: the injected connector, bounded
        // by the injected deadline clock. Its own error is kept.
        let stream = tokio::select! {
            connected = self.commands.dial(record.address()) => match connected {
                Ok(stream) => stream,
                Err(error) => return (Some(Err(NativeClientError::Io(error.kind()))), None, None),
            },
            () = cycle.stopped() => return (None, None, None),
            () = until_stopped(&mut stopped) => return (None, None, None),
        };
        let slot = Arc::new(self.commands.records().slot(*record.key()));
        let client = NativeEnrollmentClient::new(slot.clone(), self.commands.clock.clone());
        let status = tokio::select! {
            status = client.status(stream, None) => Some(status),
            () = cycle.stopped() => None,
            () = until_stopped(&mut stopped) => None,
        };
        // Wakes and waits for the client's blocking worker, whichever ended.
        client.shutdown().await;
        let status = status.map(|status| {
            status.map(|status| match status {
                NativePairingStatus::Active {
                    receiver,
                    access_epoch,
                    ..
                } => (
                    Status::Active {
                        receiver: receiver.as_str().to_owned(),
                        access_epoch,
                    },
                    String::new(),
                ),
                NativePairingStatus::Terminal { cause, .. } => (
                    Status::Ended,
                    format!("terminal: {}", snake(&format!("{cause:?}"))),
                ),
                NativePairingStatus::Unclaimed { outcome, .. } => (
                    Status::Ended,
                    format!("unclaimed: {}", snake(&format!("{outcome:?}"))),
                ),
                NativePairingStatus::Pending(_)
                | NativePairingStatus::Claimed(_)
                | NativePairingStatus::Approved(_)
                | NativePairingStatus::Staging(_) => (Status::Waiting, String::new()),
            })
        });
        (status, slot.transition(), slot.outcome())
    }

    /// Empty the peer's cache for `cause`, as one audited step. The cache is
    /// closed: the read that asked for this has ended.
    async fn reset<'a>(
        &'a self,
        key: DeviceKey,
        cause: PollerCause,
        changes: &mut Vec<PollerRecord<'a>>,
    ) {
        let before = self.commands.holding(key).await;
        let records = self.commands.records().clone();
        let removed = tokio::task::spawn_blocking(move || records.remove_cache(&key)).await;
        let after = self.commands.holding(key).await;
        let outcome = match removed {
            Ok(Ok(())) => Ok(()),
            _ => Err("peer_unavailable"),
        };
        tracing::warn!(peer = %hex(&key), cause = ?cause, removed = outcome.is_ok(),
            "peer gateway cache cannot continue against what the peer serves; emptied, reading again");
        changes.push(
            self.commands
                .poller_changed(key, cause, before, after, outcome),
        );
    }

    /// Read the peer into its cache, on a blocking thread, within the read
    /// budget. The conversations the read withdrew are one change, however
    /// it ended, a panic included, recorded in runs of
    /// [`WITHDRAWN_PER_RECORD`]. `None` when the poller stopped.
    async fn read<'a>(
        &'a self,
        record: &PeerRecord,
        receiver: &str,
        access_epoch: u64,
        cycle: &Arc<CycleStop>,
        changes: &mut Vec<PollerRecord<'a>>,
    ) -> Option<Result<ReadReport, ReadFailure>> {
        let key = *record.key();
        let records = self.commands.records().clone();
        let (address, receiver) = (record.address(), receiver.to_owned());
        let (wall, clock, stop, budget) = (
            self.inputs.wall.clone(),
            self.commands.clock.clone(),
            cycle.read().clone(),
            self.inputs.policy.read_budget,
        );
        let (work, withdrawn) = spawn_read(move |withdrawn| {
            let credential = records
                .slot(key)
                .load_credential()
                .map_err(|_| ReadFailure::Cache)?
                .ok_or(ReadFailure::Cache)?;
            let id = credential.credential().as_str().to_owned();
            let (secret, pin, _) = credential.into_enrollment().into_parts();
            let identity = NativeIdentity::restore(secret).map_err(|_| ReadFailure::Cache)?;
            let mut cache = RetainedCache::open(&records.cache_path(&key), wall, clock)?;
            cache.read(
                ReaderAccess {
                    address,
                    identity: &identity,
                    pin,
                    credential: &id,
                    receiver: &receiver,
                    access_epoch,
                    client_id: CLIENT_ID,
                },
                &stop,
                budget,
                WITHDRAWN_PER_READ,
                withdrawn,
            )
        });
        tokio::pin!(work);
        let mut stopped = self.stopped.clone();
        let ended = tokio::select! {
            ended = &mut work => ended,
            () = until_stopped(&mut stopped) => {
                // Shut the read's sockets and wait for it.
                cycle.stop();
                (&mut work).await
            }
        };
        let (result, withdrawn) = read_ended(ended, &withdrawn);
        if !withdrawn.is_empty() {
            // Read after the fact: each withdrawal removed rows inside the
            // cache, so the record and whether a cache is there are as the
            // read left them, before and after alike.
            let held = self.commands.holding(key).await;
            for conversations in withdrawn.chunks(WITHDRAWN_PER_RECORD) {
                changes.push(self.commands.poller_changed(
                    key,
                    PollerCause::Withdrawn {
                        conversations: conversations.to_vec(),
                    },
                    held.clone(),
                    held.clone(),
                    Ok(()),
                ));
            }
        }
        if self.stopping() {
            return None;
        }
        Some(result)
    }

    fn read_done(&self, key: DeviceKey, report: ReadReport) -> Pace {
        self.commands.set_sync(
            key,
            Some(PeerSync {
                state: if report.complete {
                    SyncState::Synced
                } else {
                    SyncState::Syncing
                },
                last_synced_ms: Some(self.inputs.wall.unix_milliseconds()),
                conversations: u64::try_from(report.conversations).ok(),
            }),
        );
        if report.complete {
            Pace::Settled
        } else {
            Pace::Continue
        }
    }

    /// A failed cycle: listed as `why` says, its last finished read and
    /// count kept as they were, and backed off.
    fn unread(&self, key: DeviceKey, why: &Unread) -> Pace {
        let state = sync_state(why);
        let previous = self.commands.sync_of(&key);
        self.commands.set_sync(
            key,
            Some(PeerSync {
                state,
                last_synced_ms: previous.and_then(|sync| sync.last_synced_ms),
                conversations: previous.and_then(|sync| sync.conversations),
            }),
        );
        tracing::info!(peer = %hex(&key), why = ?why, "peer gateway read failed; backing off");
        Pace::Failed
    }
}

/// Why a cycle did not read.
#[derive(Debug)]
enum Unread {
    /// The record could not be read.
    Storage,
    Status(NativeClientError),
    Read(ReadFailure),
}

/// Why a status step changed what is held: from the transition its store
/// began, which names the change whether or not its write landed, and the
/// status's own cause when it said one. `None` when the status began none.
fn status_cause(
    transition: Option<SlotTransition>,
    status: Option<&Result<(Status, String), NativeClientError>>,
) -> Option<PollerCause> {
    Some(match transition? {
        SlotTransition::Approved => PollerCause::Approved,
        SlotTransition::Ended => PollerCause::Ended {
            detail: match status {
                Some(Ok((Status::Ended, detail))) => Some(detail.clone()),
                _ => None,
            },
        },
    })
}

/// Whether a status step's change landed: from the record's store, which
/// alone knows whether the transition it began was written, never from
/// whether the status read then succeeded.
fn status_outcome(landed: Option<SlotOutcome>) -> Result<(), &'static str> {
    match landed {
        Some(SlotOutcome::Landed) => Ok(()),
        Some(SlotOutcome::NotLanded) | None => Err("peer_unavailable"),
    }
}

/// The conversations a read has withdrawn so far, outside its worker.
type Withdrawn = Arc<Mutex<Vec<String>>>;

/// Start `read` on a blocking thread, with the conversations it withdraws
/// kept outside that thread: a read that panics part way has still removed
/// them from the cache, and its caller still learns of them.
fn spawn_read(
    read: impl FnOnce(&mut Vec<String>) -> Result<ReadReport, ReadFailure> + Send + 'static,
) -> (JoinHandle<Result<ReadReport, ReadFailure>>, Withdrawn) {
    let withdrawn = Arc::new(Mutex::new(Vec::new()));
    let work = tokio::task::spawn_blocking({
        let withdrawn = withdrawn.clone();
        move || read(&mut withdrawn.lock().unwrap_or_else(PoisonError::into_inner))
    });
    (work, withdrawn)
}

/// How a read started by [`spawn_read`] ended, and what it withdrew, however
/// it ended: a worker that panicked is a cache failure, and what it withdrew
/// before is kept.
fn read_ended(
    joined: Result<Result<ReadReport, ReadFailure>, JoinError>,
    withdrawn: &Mutex<Vec<String>>,
) -> (Result<ReadReport, ReadFailure>, Vec<String>) {
    let withdrawn = std::mem::take(&mut *withdrawn.lock().unwrap_or_else(PoisonError::into_inner));
    (joined.unwrap_or(Err(ReadFailure::Cache)), withdrawn)
}

/// How a cycle that did not read is listed.
fn sync_state(why: &Unread) -> SyncState {
    match why {
        Unread::Status(NativeClientError::Io(_) | NativeClientError::Handshake(_))
        | Unread::Read(ReadFailure::Unreachable) => SyncState::Unreachable,
        Unread::Read(ReadFailure::Quota) => SyncState::Quota,
        _ => SyncState::Failed,
    }
}

/// Resolves once a stop is asked for, holding no borrow of the flag.
async fn until_stopped(stopped: &mut watch::Receiver<bool>) {
    let _ = stopped.wait_for(|stop| *stop).await;
}

/// What a status read said, as the poller acts on it.
enum Status {
    Active {
        receiver: String,
        access_epoch: u64,
    },
    /// The peer ended this enrollment; the client has removed the cache and
    /// marked the record revoked.
    Ended,
    /// Still pending on the peer.
    Waiting,
}

/// What one peer's cycle came to, as its next wait follows from it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pace {
    /// Read to the end: the interval.
    Settled,
    /// Read up to a bound: soon.
    Continue,
    /// Stopped by an owner command: soon; failures unchanged.
    Stopped,
    /// Still pending on the peer: the interval.
    Waiting,
    /// Failed: a backoff that doubles with each failure in a row.
    Failed,
    /// Forgotten or ended: not read again.
    Gone,
}

/// The wait after `pace`, with `failures` failures in a row before it, and
/// the failures in a row after it. `draw` spreads it over 80%–120%, so peers
/// read together drift apart; then it is held to the cap, so the cap is the
/// longest any peer waits.
fn next_wait(policy: &PollPolicy, pace: Pace, failures: u32, draw: u32) -> (Duration, u32) {
    let (base, failures) = match pace {
        Pace::Settled | Pace::Waiting | Pace::Gone => (policy.interval, 0),
        Pace::Continue => (policy.interval / 8, 0),
        Pace::Stopped => (policy.interval / 8, failures),
        Pace::Failed => {
            let failures = failures.saturating_add(1);
            (
                policy
                    .interval
                    .saturating_mul(1 << failures.min(16))
                    .min(policy.backoff_cap),
                failures,
            )
        }
    };
    let spread = base.mul_f64(0.8 + f64::from(draw % 401) / 1000.0);
    (spread.min(policy.backoff_cap), failures)
}

/// One draw for the jitter; the midpoint when entropy fails.
fn draw(entropy: &EnrollmentEntropySource) -> u32 {
    let mut bytes = [0u8; 4];
    match entropy().try_fill_bytes(&mut bytes) {
        Ok(()) => u32::from_le_bytes(bytes),
        Err(_) => 200,
    }
}

/// When the poller next wakes, on the injected clock: the soonest peer due,
/// and never later than one interval from `now`. When the records could not
/// be listed, no peer was read, so a due already past would wake it at once,
/// again and again; it waits the interval instead.
fn next_wake(listed: bool, due: impl Iterator<Item = u64>, now: u64, interval: Duration) -> u64 {
    let latest = now.saturating_add(millis(interval));
    if !listed {
        return latest;
    }
    due.min().unwrap_or(u64::MAX).min(latest)
}

fn millis(wait: Duration) -> u64 {
    u64::try_from(wait.as_millis()).unwrap_or(u64::MAX)
}

/// `TerminalCause::CredentialRevoked`'s Debug name as `credential_revoked`.
fn snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (index, character) in name.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(character.to_ascii_lowercase());
        } else if character.is_ascii_alphanumeric() || character == '_' {
            out.push(character);
        } else {
            break;
        }
    }
    out
}

fn hex(key: &DeviceKey) -> String {
    key.bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
#[path = "../../../tests/peer_gateways/poller.rs"]
mod tests;
