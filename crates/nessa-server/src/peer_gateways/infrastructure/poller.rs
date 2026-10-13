//! Reading what each peer granted this gateway, on a cadence: one task
//! schedules, and up to [`READS_AT_ONCE`] cycles run side by side. Each
//! cycle holds that peer's turn. An owner command that names the peer stops
//! that cycle instead of waiting for it; a cycle of any other peer keeps
//! going. A peer that fails, or runs until its own [`READ_BUDGET`], is
//! reported and rescheduled on its own. It does not fail the pass or hold
//! the other reads.
//!
//! ```text
//! records changed, or the interval --> PeerRecords::list (schedules)
//!   --> each due peer, up to READS_AT_ONCE:
//!   take that peer's turn --> re-read its record; gone or revoked --> drop it
//!   pinned status (NativeEnrollmentClient over its PeerSlot)
//!     Active   --> the client saves the credential --> RetainedCache::read
//!                    Refused                    --> pinned status again
//!                    ResetRequired, CacheDamaged --> empty the cache, read once more
//!     Terminal or Unclaimed --> the client removes the cache and marks the
//!                               record revoked; the peer is not read again
//!     otherwise (still pending) --> wait
//!   every change above, as it is made --> PeerCommands::poller_changed
//!                                          (queued by the audit at once)
//!   give back the turn, still holding the read slot --> PeerCommands::poller_kept
//!   free the read slot
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
//! the record at once, so a change the cycle made comes before the owner
//! command that stopped that peer. Cycles running side by side hand their
//! records to the audit's one writer, which keeps the order they were handed
//! over.
//! The cycle waits for the audit's answers only once it has given back the
//! turn, and it keeps its read slot until those answers come or
//! [`super::AUDIT_DEADLINE`] passes, so an audit that is slow or stalled
//! never holds the turn and a further cycle cannot enqueue while these
//! records are still queued. A stop
//! during the status wait does not drop the worker's answer: that answer
//! names an ended enrollment's cause, and the worker may already have
//! committed the ending. The cycle still does not go on to read.
//! `a_stop_after_an_ended_transition_commits_keeps_the_status_cause` is
//! that race, with the stop winning.
use super::audit::{AUDIT_HEADROOM, AUDIT_QUEUE};
use super::commands::{
    CycleStop, CycleTurn, EnrollmentEntropySource, PeerCommands, PeerSync, PollerRecord, SyncState,
};
use super::records::{PeerEntry, PeerPhase, PeerRecord, SlotOutcome, SlotTransition};
use crate::peer_gateways::application::{PeerHolding, PollerCause, WITHDRAWN_PER_RECORD};
use nessa_auth::{
    adapters::pairing::NativeIdentity,
    application::{pairing::ClientPendingStore, ports::Clock as WallClock},
    domain::pairing::DeviceKey,
};
use nessa_client_core::pairing::{NativeClientError, NativeEnrollmentClient};
use nessa_client_core::retained::{
    ReadFailure, ReadReport, ReaderAccess, ReaderRole, RetainedCache,
};
use nessa_protocol::pairing::socket::WAKE_TICK;
use nessa_protocol::pairing::wire::NativePairingStatus;
use nessa_protocol::relay::RelayRefusal;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};
use tokio::{
    sync::{watch, OwnedSemaphorePermit, Semaphore},
    task::{Id, JoinError, JoinHandle, JoinSet},
};

/// How often each peer is read once it is up to date.
pub const POLL_INTERVAL: Duration = Duration::from_secs(30);
/// The longest wait a peer is scheduled for between reads, failing or not,
/// jitter included. A peer that is due while every read slot is taken waits
/// for a slot on top of this.
pub const POLL_BACKOFF_CAP: Duration = Duration::from_secs(15 * 60);
/// The longest one read of a peer runs before it ends incomplete. Each peer
/// has this timeout of its own.
pub const READ_BUDGET: Duration = Duration::from_secs(60);
/// How many peers are read at once. One more stays due until a read finishes
/// and frees a place; it is not failed, and it is not skipped. A slow peer
/// holds only its own place, and only until its own [`READ_BUDGET`].
pub const READS_AT_ONCE: usize = 4;
const _: () = assert!(READS_AT_ONCE >= 1);
/// The most conversations one read withdraws: sixteen records of
/// [`WITHDRAWN_PER_RECORD`]. Fixed, so the audit queue can be sized for
/// every cycle running at once without a cycle growing with it. A read that
/// reaches it ends incomplete and the next cycle, soon, withdraws the rest.
pub const WITHDRAWN_PER_READ: usize = 16 * WITHDRAWN_PER_RECORD;
/// The most records one cycle hands the audit: two status steps, a reset,
/// and the two reads around it.
const RECORDS_PER_CYCLE: usize = 3 + 2 * WITHDRAWN_PER_READ.div_ceil(WITHDRAWN_PER_RECORD);
/// The queue holds every cycle that can be enqueueing plus
/// [`AUDIT_HEADROOM`] owner record. A full queue still refuses at once
/// (design row R22).
const _: () = assert!(READS_AT_ONCE * RECORDS_PER_CYCLE + AUDIT_HEADROOM <= AUDIT_QUEUE);
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
    /// How many reads run at once. Zero is read as one
    /// (`a_schedule_of_no_places_still_reads_one`).
    pub reads_at_once: usize,
}
impl Default for PollPolicy {
    fn default() -> Self {
        Self {
            interval: POLL_INTERVAL,
            backoff_cap: POLL_BACKOFF_CAP,
            read_budget: READ_BUDGET,
            reads_at_once: READS_AT_ONCE,
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
    /// How many times this poller has scanned the peer records.
    #[cfg(test)]
    lists: Arc<AtomicUsize>,
}
impl PeerPoller {
    /// Start reading `commands`' peers with `inputs`.
    pub fn start(commands: Arc<PeerCommands>, inputs: PollInputs) -> Self {
        let (stop, stopped) = watch::channel(false);
        let reads = Arc::new(Semaphore::new(read_places(inputs.policy.reads_at_once)));
        #[cfg(test)]
        let lists = Arc::new(AtomicUsize::new(0));
        let shared = Arc::new(Shared {
            commands: commands.clone(),
            inputs,
            stopped,
        });
        let poller = Poller {
            shared,
            reads,
            due: HashMap::new(),
            running: JoinSet::new(),
            running_keys: HashMap::new(),
            known: Vec::new(),
            listed_generation: 0,
            listed_at: None,
            #[cfg(test)]
            lists: Arc::clone(&lists),
        };
        Self {
            stop,
            commands,
            task: tokio::spawn(poller.run()),
            #[cfg(test)]
            lists,
        }
    }
    /// How many times the peer records have been scanned.
    #[cfg(test)]
    pub(crate) fn lists(&self) -> usize {
        self.lists.load(Ordering::Relaxed)
    }
    /// Ask the poller to stop: its waits end, and every cycle in progress is
    /// stopped, its read's sockets shut at once.
    pub fn signal_stop(&self) {
        self.stop.send_replace(true);
        self.commands.stop_all_cycles();
    }
    /// Stop, and wait until nothing the poller started is running: its task,
    /// and every blocking read or status worker a cycle was awaiting.
    pub async fn join(self) {
        self.signal_stop();
        let _ = self.task.await;
    }
}

/// What every cycle shares. The schedule stays on [`Poller`].
struct Shared {
    commands: Arc<PeerCommands>,
    inputs: PollInputs,
    stopped: watch::Receiver<bool>,
}

struct Poller {
    shared: Arc<Shared>,
    /// Places for reads running at once. A cycle holds one from the moment
    /// it starts until its audit records are answered or the audit deadline
    /// passes, so at most [`READS_AT_ONCE`] cycles are enqueueing together.
    reads: Arc<Semaphore>,
    due: HashMap<DeviceKey, Due>,
    running: JoinSet<Option<Pace>>,
    running_keys: HashMap<Id, DeviceKey>,
    /// The peers the last successful list found.
    known: Vec<DeviceKey>,
    /// The record generation that list was taken against.
    listed_generation: u64,
    /// When that list finished, on the injected clock. `None` until one does.
    listed_at: Option<u64>,
    /// How many directory scans this poller has done.
    #[cfg(test)]
    lists: Arc<AtomicUsize>,
}
impl Poller {
    fn stopping(&self) -> bool {
        self.shared.stopping()
    }
    fn now(&self) -> u64 {
        self.shared.now()
    }
    async fn run(mut self) {
        loop {
            if self.stopping() {
                break;
            }
            let now = self.now();
            // Captured before the scan, so a write during it is seen next.
            let generation = self.shared.commands.records().listed_generation();
            let list_now = match self.listed_at {
                None => true,
                Some(at) => list_peers(
                    generation != self.listed_generation,
                    now.saturating_sub(at),
                    millis(self.shared.inputs.policy.interval),
                ),
            };
            let have = if list_now {
                match self.peers().await {
                    Some(live) => {
                        self.known = live;
                        self.listed_generation = generation;
                        self.listed_at = Some(self.now());
                        true
                    }
                    None => false,
                }
            } else {
                true
            };
            let known = have.then(|| self.known.clone());
            self.start_due(now, known.as_deref());
            if !self.wait(have).await {
                break;
            }
        }
        self.shared.commands.stop_all_cycles();
        self.join_running().await;
    }

    /// Start each due peer that is not already running, up to the free read
    /// slots. A due peer with no free slot stays due.
    fn start_due(&mut self, now: u64, listed: Option<&[DeviceKey]>) {
        let Some(listed) = listed else {
            return;
        };
        for key in listed {
            if self.stopping() {
                return;
            }
            if self.running_keys.values().any(|running| running == key) {
                continue;
            }
            let due = self.due.entry(*key).or_insert(Due {
                at: now,
                failures: 0,
            });
            if due.at > now {
                continue;
            }
            let Ok(permit) = self.reads.clone().try_acquire_owned() else {
                continue;
            };
            let shared = Arc::clone(&self.shared);
            let key = *key;
            let handle = self
                .running
                .spawn(async move { shared.cycle(key, permit).await });
            self.running_keys.insert(handle.id(), key);
        }
    }

    /// Wait until the schedule says to look again. `false` once the poller
    /// is stopped. A read in flight, or a due peer that did not start, wakes
    /// within one [`WAKE_TICK`] ([`look_again_soon`]), so a record change is
    /// noticed without waiting out that read, and a full cap does not spin.
    /// The directory is scanned only when [`list_peers`] says so. Idle, and
    /// a list that failed, this waits out `next` on the injected clock, one
    /// tick at a time, and does not list between ticks.
    async fn wait(&mut self, listed: bool) -> bool {
        let next = next_wake(
            listed,
            self.due.values().map(|due| due.at),
            self.now(),
            self.shared.inputs.policy.interval,
        );
        let mut stopped = self.shared.stopped.clone();
        loop {
            let left = next.saturating_sub(self.now());
            let running = !self.running.is_empty();
            let soon = look_again_soon(listed, running, left);
            let pause = if left == 0 {
                WAKE_TICK
            } else {
                Duration::from_millis(left).min(WAKE_TICK)
            };
            tokio::select! {
                biased;
                () = until_stopped(&mut stopped) => return false,
                Some(finished) = self.running.join_next_with_id(), if running => {
                    self.finish(finished);
                    if soon {
                        return true;
                    }
                }
                () = tokio::time::sleep(pause) => {
                    if soon || next.saturating_sub(self.now()) == 0 {
                        return true;
                    }
                }
            }
        }
    }

    fn finish(&mut self, finished: Result<(Id, Option<Pace>), JoinError>) {
        let (id, pace) = match finished {
            Ok((id, pace)) => (id, pace),
            Err(error) => {
                // The id is on the error, so the peer is still rescheduled.
                let id = error.id();
                tracing::error!(
                    peer = self.running_keys.get(&id).map(hex).as_deref().unwrap_or("unknown"),
                    %error,
                    "peer gateway read ended abruptly; that peer backs off"
                );
                (id, Some(Pace::Failed))
            }
        };
        let Some(key) = self.running_keys.remove(&id) else {
            return;
        };
        if let Some(pace) = pace {
            self.reschedule(key, pace);
        }
    }

    async fn join_running(&mut self) {
        while let Some(finished) = self.running.join_next_with_id().await {
            self.finish(finished);
        }
    }

    /// The peers to schedule: those whose records read and are not revoked.
    /// An unreadable record is skipped. `None` when the records could not be
    /// listed, which waits out the interval and does not stop a read already
    /// running. Who is read is decided again under that peer's turn.
    async fn peers(&mut self) -> Option<Vec<DeviceKey>> {
        #[cfg(test)]
        self.lists.fetch_add(1, Ordering::Relaxed);
        let records = self.shared.commands.records().clone();
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
        self.shared.commands.keep_syncs(&live);
        Some(live)
    }

    fn reschedule(&mut self, key: DeviceKey, pace: Pace) {
        if pace == Pace::Gone {
            self.due.remove(&key);
            return;
        }
        let draw = draw(&self.shared.inputs.entropy);
        let now = self.now();
        let Some(due) = self.due.get_mut(&key) else {
            return;
        };
        let (wait, failures) = next_wait(&self.shared.inputs.policy, pace, due.failures, draw);
        due.failures = failures;
        due.at = now.saturating_add(millis(wait));
    }
}

impl Shared {
    fn stopping(&self) -> bool {
        *self.stopped.borrow()
    }
    fn now(&self) -> u64 {
        self.commands.clock.elapsed_ms()
    }

    /// That peer's turn, asked for every [`WAKE_TICK`] until free.
    /// `None` once the poller is stopped.
    async fn take_turn(&self, key: DeviceKey, cycle: &Arc<CycleStop>) -> Option<CycleTurn> {
        let mut stopped = self.stopped.clone();
        loop {
            if let Some(turn) = self.commands.try_cycle_turn(key, cycle) {
                // A stop asked before the cycle was registered did not reach it.
                return (!self.stopping()).then_some(turn);
            }
            tokio::select! {
                () = tokio::time::sleep(WAKE_TICK) => {}
                () = until_stopped(&mut stopped) => return None,
            }
        }
    }

    /// One peer: under its turn, its record read again, the pinned status,
    /// then a read if it is Active, each change handed to the audit as it is
    /// made. The turn is given back before the audit's answers are awaited,
    /// and the read slot is held until they are, or [`super::AUDIT_DEADLINE`]
    /// passes. `None` when the poller stopped. A failure is this peer's
    /// pace; it is not the pass's.
    async fn cycle(&self, key: DeviceKey, permit: OwnedSemaphorePermit) -> Option<Pace> {
        let cycle = CycleStop::new();
        let turn = match self.take_turn(key, &cycle).await {
            Some(turn) => turn,
            None => return None,
        };
        let mut changes = Vec::new();
        let pace = self.held(key, &cycle, &mut changes).await;
        // The turn is given back first: an audit, however slow, never holds
        // it. The read slot stays until the answers, so another cycle cannot
        // enqueue while these records are still queued. The queue holds
        // every such cycle plus one owner record
        // (`a_queue_of_64_refuses_two_full_cycles_and_the_sized_queue_keeps_every_cycle_and_an_owner_record`).
        // The changes stand either way.
        drop(turn);
        self.commands.poller_kept(changes).await;
        drop(permit);
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
            Err(why) => self.unread(key, &why),
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
    /// cycle was stopped. A stop still audits an ending the worker committed,
    /// with the cause that worker's answer names.
    async fn status<'a>(
        &'a self,
        record: &PeerRecord,
        cycle: &Arc<CycleStop>,
        changes: &mut Vec<PollerRecord<'a>>,
    ) -> Option<Result<Status, Unread>> {
        let key = *record.key();
        let before = self.commands.holding(key).await;
        let asked = self.ask_status(record, cycle).await;
        let away = asked.away;
        let after = self.commands.holding(key).await;
        let recorded = status_record(asked, &before, &after);
        if let Some(cause) = recorded.cause {
            changes.push(
                self.commands
                    .poller_changed(key, cause, before, after, recorded.outcome),
            );
        } else if after != before {
            tracing::warn!(peer = %hex(&key), before = ?before, after = ?after,
                "peer gateway holding read differently around a status that began no change");
        }
        recorded.answer.map(|answer| {
            answer.map_err(|error| {
                if away {
                    Unread::Away
                } else {
                    Unread::Status(error)
                }
            })
        })
    }

    /// The status, the transition its store began, if any, and whether that
    /// transition's writes landed. `stopped` when a stop won the wait: the
    /// worker's answer is still here, so the cause of an ending it committed
    /// is not lost, and the cycle does not continue into a read.
    async fn ask_status(&self, record: &PeerRecord, cycle: &Arc<CycleStop>) -> AskedStatus {
        let mut stopped = self.stopped.clone();
        // The same connect as an enrollment's: the injected connector, bounded
        // by the injected deadline clock. Its own error is kept.
        let stream = tokio::select! {
            connected = self.commands.dial_peer(record) => match connected {
                Ok(stream) => stream,
                Err(error) => return AskedStatus::dial_failed(error),
            },
            () = cycle.stopped() => return AskedStatus::stopped_before_dial(),
            () = until_stopped(&mut stopped) => return AskedStatus::stopped_before_dial(),
        };
        let slot = Arc::new(self.commands.records().slot(*record.key()));
        let client = NativeEnrollmentClient::new(slot.clone(), self.commands.clock.clone());
        // `shutdown` wakes a worker still blocked in the read and waits until
        // it has finished. `asked_status` polls it when a stop wins; this
        // second call drains a status that answered first.
        let asked = asked_status(
            client.status(stream, None),
            cycle.stopped(),
            &mut stopped,
            client.shutdown(),
            || (slot.transition(), slot.outcome()),
        )
        .await;
        client.shutdown().await;
        asked
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
        let (address, relay, receiver) = (
            record.address(),
            record.relay().cloned(),
            receiver.to_owned(),
        );
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
                    role: ReaderRole::GrantedPeer,
                    address,
                    relay: relay.as_ref(),
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
    /// The status dial did not connect, and the peer's relay said the peer
    /// is not registered with it.
    Away,
    Read(ReadFailure),
}

/// What [`Poller::ask_status`] learned. `stopped` means a stop won the wait.
/// The worker's answer is still in `status` when it gave one, including
/// after that stop.
struct AskedStatus {
    status: Option<Result<(Status, String), NativeClientError>>,
    transition: Option<SlotTransition>,
    landed: Option<SlotOutcome>,
    stopped: bool,
    /// The dial did not connect and the peer's relay said the peer is not
    /// registered with it.
    away: bool,
}
impl AskedStatus {
    /// The dial wait stopped before a status was asked.
    fn stopped_before_dial() -> Self {
        Self {
            status: None,
            transition: None,
            landed: None,
            stopped: true,
            away: false,
        }
    }

    /// The dial failed before a status was asked. No transition began.
    fn dial_failed(error: std::io::Error) -> Self {
        Self {
            away: RelayRefusal::carried(&error) == Some(RelayRefusal::Offline),
            status: Some(Err(NativeClientError::Io(error.kind()))),
            transition: None,
            landed: None,
            stopped: false,
        }
    }
}

/// The status call's answer as the poller keeps it: the cause text, the
/// transition `noted` reads once the worker has returned, and whether a
/// stop won the wait. A stop does not drop the answer.
/// `a_stop_after_an_ended_transition_commits_keeps_the_status_cause` passes
/// a stand-in call and checks the cause on `AskedStatus`.
async fn asked_status<S, C, P>(
    status: S,
    cycle_stopped: C,
    poller_stopped: &mut watch::Receiver<bool>,
    settle: P,
    noted: impl FnOnce() -> (Option<SlotTransition>, Option<SlotOutcome>),
) -> AskedStatus
where
    S: Future<Output = Result<NativePairingStatus, NativeClientError>>,
    C: Future<Output = ()>,
    P: Future<Output = ()>,
{
    let (status, stopped_early) = take_status(status, cycle_stopped, poller_stopped, settle).await;
    let (transition, landed) = noted();
    AskedStatus {
        status: Some(status.map(status_text)),
        transition,
        landed,
        stopped: stopped_early,
        away: false,
    }
}

/// `(Status, cause text)` for one native status. The text is what an ending's
/// audit detail keeps; a status that has not ended carries an empty string.
fn status_text(status: NativePairingStatus) -> (Status, String) {
    match status {
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
    }
}

/// The status worker's answer, and whether a stop won the wait before it.
/// A stop does not drop the answer: `settle` wakes the worker (the client's
/// shutdown) and this then takes whatever the worker returns.
async fn take_status<S, C, P>(
    status: S,
    cycle_stopped: C,
    poller_stopped: &mut watch::Receiver<bool>,
    settle: P,
) -> (S::Output, bool)
where
    S: Future,
    C: Future<Output = ()>,
    P: Future<Output = ()>,
{
    tokio::pin!(status);
    tokio::pin!(cycle_stopped);
    tokio::pin!(settle);
    tokio::select! {
        answered = &mut status => return (answered, false),
        () = &mut cycle_stopped => {}
        () = until_stopped(poller_stopped) => {}
    }
    let answered = tokio::select! {
        answered = &mut status => Some(answered),
        () = &mut settle => None,
    };
    let answered = match answered {
        Some(answered) => answered,
        // Runs only when no worker started: shutdown found the permit free,
        // and this call is taken now against a client already closed.
        None => status.await,
    };
    (answered, true)
}

/// What a status step records, and the status the cycle may continue with.
/// The cause is taken before a stop returns `None`, so an ending a stop
/// won after is still recorded and the cycle does not go on to read.
/// `a_stopped_status_records_its_cause_and_does_not_read` is that stop.
struct StatusRecord {
    /// The change to hand the audit, when the step began one worth keeping.
    cause: Option<PollerCause>,
    /// The store's answer for that change.
    outcome: Result<(), &'static str>,
    /// `None` when the cycle must not go on to a read.
    answer: Option<Result<Status, NativeClientError>>,
}

fn status_record(asked: AskedStatus, before: &PeerHolding, after: &PeerHolding) -> StatusRecord {
    let outcome = status_outcome(asked.landed);
    // A transition that changed nothing and failed nowhere is no change.
    let cause = match status_cause(asked.transition, asked.status.as_ref()) {
        Some(cause) if after != before || outcome.is_err() => Some(cause),
        _ => None,
    };
    let answer = if asked.stopped {
        None
    } else {
        asked.status.map(|status| status.map(|(status, _)| status))
    };
    StatusRecord {
        cause,
        outcome,
        answer,
    }
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
        Unread::Away | Unread::Read(ReadFailure::Away) => SyncState::Away,
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
/// longest wait any peer is scheduled for.
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

/// Whether the scheduler wakes within a tick. A read in flight, or a due
/// peer that did not start (`left` is 0), is looked at again so an
/// enrollment beside a read is not held until that read ends, and so a full
/// cap does not spin. Waking does not scan the directory; [`list_peers`]
/// does. Idle, and a list that failed, wait out the schedule.
fn look_again_soon(listed: bool, running: bool, left: u64) -> bool {
    listed && (running || left == 0)
}

/// Whether to scan the peer records. A wake while a read runs is every
/// tick; the directory is scanned when the records have changed since the
/// last list, or one interval has passed
/// (`a_held_read_lists_when_the_records_change_not_on_every_tick`).
fn list_peers(changed: bool, since_list_ms: u64, interval_ms: u64) -> bool {
    changed || since_list_ms >= interval_ms
}

/// How many reads `asked` runs at once. Zero is one: a schedule of no
/// places would leave every due peer waiting for a slot that cannot exist
/// (`a_schedule_of_no_places_still_reads_one`).
fn read_places(asked: usize) -> usize {
    asked.max(1)
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
