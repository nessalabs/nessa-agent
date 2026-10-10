//! Reading what each peer granted this gateway, on a cadence: one task,
//! one peer at a time, each read holding the peer commands' turn so no
//! enrollment or forget runs beside it.
//!
//! ```text
//! tick --> PeerRecords::list --> each due peer, pending or active:
//!   pinned status (NativeEnrollmentClient over its PeerSlot)
//!     Active   --> the client saves the credential --> RetainedCache::read
//!                    Refused        --> pinned status again
//!                    ResetRequired  --> remove the cache, read once more
//!     Terminal or Unclaimed --> the client marks the record revoked
//!                           --> remove the cache; the peer is not read again
//!     otherwise (still pending) --> wait
//! ```
//! Arrows are calls, in order. No read happens without a fresh Active
//! status, and nothing is removed without a Terminal one: a refused
//! `openProduct` is redacted, so a revoked credential and a full pool look
//! alike until the status is asked (design row PC5, which the device follows
//! too). Each peer is read again after the interval, with jitter, and after
//! a failure, after a backoff that doubles up to its cap.
use super::commands::{PeerCommands, PeerSync, SyncState};
use super::records::{PeerEntry, PeerPhase, PeerRecord};
use nessa_auth::{
    adapters::pairing::NativeIdentity,
    application::{pairing::ClientPendingStore, ports::Clock as WallClock},
    domain::pairing::DeviceKey,
};
use nessa_client_core::pairing::{NativeClientError, NativeEnrollmentClient};
use nessa_client_core::retained::{ReadFailure, ReadReport, ReadStop, ReaderAccess, RetainedCache};
use nessa_protocol::pairing::wire::NativePairingStatus;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    sync::watch,
    task::JoinHandle,
    time::{sleep_until, Instant},
};

/// How often each peer is read once it is up to date.
pub const POLL_INTERVAL: Duration = Duration::from_secs(30);
/// The longest a failing peer waits before it is tried again.
pub const POLL_BACKOFF_CAP: Duration = Duration::from_secs(15 * 60);
/// How this gateway names itself to a peer's product session.
const CLIENT_ID: &str = "nessa-peer-gateway";

/// The poller's cadence.
#[derive(Clone, Copy, Debug)]
pub struct PollPolicy {
    /// The wait between reads of a peer that is up to date.
    pub interval: Duration,
    /// The longest wait after failures.
    pub backoff_cap: Duration,
}
impl Default for PollPolicy {
    fn default() -> Self {
        Self {
            interval: POLL_INTERVAL,
            backoff_cap: POLL_BACKOFF_CAP,
        }
    }
}

/// When a peer is next read, and why.
struct Due {
    at: Instant,
    failures: u32,
    /// The last read stopped short: read again even if the head is unchanged.
    settle: bool,
}

/// The running poller, and how to stop it.
pub struct PeerPoller {
    stop: watch::Sender<bool>,
    reads: Arc<ReadStop>,
    task: JoinHandle<()>,
}
impl PeerPoller {
    /// Start reading `commands`' peers under `policy`. `wall` stamps when a
    /// read finished.
    pub fn start(
        commands: Arc<PeerCommands>,
        policy: PollPolicy,
        wall: Arc<dyn WallClock>,
    ) -> Self {
        let (stop, stopped) = watch::channel(false);
        let reads = ReadStop::new();
        let poller = Poller {
            commands,
            policy,
            wall,
            reads: reads.clone(),
            stopped,
            due: HashMap::new(),
        };
        Self {
            stop,
            reads,
            task: tokio::spawn(poller.run()),
        }
    }
    /// Ask the poller to stop: its wait ends, and a read in progress has its
    /// socket shut at once.
    pub fn signal_stop(&self) {
        self.stop.send_replace(true);
        self.reads.stop();
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
    policy: PollPolicy,
    wall: Arc<dyn WallClock>,
    reads: Arc<ReadStop>,
    stopped: watch::Receiver<bool>,
    due: HashMap<DeviceKey, Due>,
}
impl Poller {
    fn stopping(&self) -> bool {
        *self.stopped.borrow()
    }
    async fn run(mut self) {
        while !self.stopping() {
            let now = Instant::now();
            for record in self.peers().await {
                if self.stopping() {
                    return;
                }
                let key = *record.key();
                let due = self.due.entry(key).or_insert(Due {
                    at: now,
                    failures: 0,
                    settle: false,
                });
                if due.at > now {
                    continue;
                }
                let settle = due.settle;
                let Some(outcome) = self.cycle(record, settle).await else {
                    continue;
                };
                self.reschedule(key, outcome);
            }
            let next = self
                .due
                .values()
                .map(|due| due.at)
                .min()
                .unwrap_or(now + self.policy.interval)
                .min(Instant::now() + self.policy.interval);
            let mut stopped = self.stopped.clone();
            tokio::select! {
                _ = sleep_until(next) => {}
                _ = until_stopped(&mut stopped) => return,
            }
        }
    }

    /// Readable peers still enrolled, pending or active. Revoked ones have
    /// their cache removed here too, should an earlier removal have failed.
    async fn peers(&mut self) -> Vec<PeerRecord> {
        let records = self.commands.records.clone();
        let listed = tokio::task::spawn_blocking(move || records.list()).await;
        let Ok(Ok(entries)) = listed else {
            tracing::warn!("peer gateway records could not be listed");
            return Vec::new();
        };
        let mut live = Vec::new();
        for entry in entries {
            let PeerEntry::Readable(record) = entry else {
                continue;
            };
            match record.phase() {
                PeerPhase::Revoked => self.clear_revoked(*record.key()).await,
                PeerPhase::Pending | PeerPhase::Active { .. } => live.push(record),
            }
        }
        self.due
            .retain(|key, _| live.iter().any(|record| record.key() == key));
        live
    }

    async fn clear_revoked(&mut self, key: DeviceKey) {
        self.due.remove(&key);
        self.commands.set_sync(key, None);
        let records = self.commands.records.clone();
        let Ok(_turn) = self.commands.turn.clone().try_acquire_owned() else {
            return;
        };
        let _ = tokio::task::spawn_blocking(move || {
            if records.has_cache(&key)? {
                records.remove_cache(&key)?;
                tracing::info!(peer = %hex(&key), "peer gateway cache removed after revocation");
            }
            Ok::<_, nessa_auth::application::pairing::PrivateStateError>(())
        })
        .await;
    }

    fn reschedule(&mut self, key: DeviceKey, outcome: Cycle) {
        let Some(due) = self.due.get_mut(&key) else {
            return;
        };
        let interval = self.policy.interval;
        let wait = match outcome {
            Cycle::Ended => {
                self.due.remove(&key);
                return;
            }
            Cycle::Settled { complete } => {
                due.failures = 0;
                due.settle = !complete;
                // An incomplete read continues soon; a settled peer waits.
                if complete {
                    interval
                } else {
                    interval / 8
                }
            }
            Cycle::Waiting => {
                due.failures = 0;
                interval
            }
            Cycle::Failed => {
                due.failures = due.failures.saturating_add(1);
                interval
                    .saturating_mul(1 << due.failures.min(16))
                    .min(self.policy.backoff_cap)
            }
        };
        due.at = Instant::now() + jitter(wait);
    }

    /// One peer: the pinned status, then a read if it is Active. `None` when
    /// the poller stopped part way.
    async fn cycle(&mut self, record: PeerRecord, settle: bool) -> Option<Cycle> {
        let key = *record.key();
        let mut stopped = self.stopped.clone();
        let turn = tokio::select! {
            turn = self.commands.turn.clone().acquire_owned() => turn.ok()?,
            _ = until_stopped(&mut stopped) => return None,
        };
        let status = self.status(&record).await?;
        let outcome = match status {
            Err(error) => self.unread(&record, Some(error), None),
            Ok(Status::Ended(cause)) => self.ended(&record, &cause).await,
            Ok(Status::Waiting) => {
                self.commands.set_sync(
                    key,
                    Some(PeerSync {
                        state: SyncState::Waiting,
                        last_synced_ms: None,
                        conversations: None,
                    }),
                );
                Cycle::Waiting
            }
            Ok(Status::Active {
                receiver,
                access_epoch,
            }) => match self.read(&record, &receiver, access_epoch, settle).await? {
                Ok(report) => self.read_done(&record, report),
                Err(ReadFailure::Refused) => match self.status(&record).await? {
                    Ok(Status::Ended(cause)) => self.ended(&record, &cause).await,
                    _ => self.unread(&record, None, Some(ReadFailure::Refused)),
                },
                Err(ReadFailure::Stopped) => return None,
                Err(failure) => self.unread(&record, None, Some(failure)),
            },
        };
        drop(turn);
        Some(outcome)
    }

    /// The peer's pinned status through its record. `None` when stopped.
    async fn status(&self, record: &PeerRecord) -> Option<Result<Status, NativeClientError>> {
        let address = record.address();
        let mut stopped = self.stopped.clone();
        // The same connect as an enrollment's: the injected connector, bounded
        // by the injected deadline clock.
        let stream = tokio::select! {
            connected = self.commands.connect(address) => match connected {
                Ok(stream) => stream,
                Err(_) => return Some(Err(NativeClientError::Io(std::io::ErrorKind::TimedOut))),
            },
            _ = until_stopped(&mut stopped) => return None,
        };
        let client = NativeEnrollmentClient::new(
            Arc::new(self.commands.records.slot(*record.key())),
            self.commands.clock.clone(),
        );
        let status = tokio::select! {
            status = client.status(stream, None) => Some(status),
            _ = until_stopped(&mut stopped) => None,
        };
        // Wakes and waits for the client's blocking worker, whichever ended.
        client.shutdown().await;
        Some(status?.map(|status| match status {
            NativePairingStatus::Active {
                receiver,
                access_epoch,
                ..
            } => Status::Active {
                receiver: receiver.as_str().to_owned(),
                access_epoch,
            },
            NativePairingStatus::Terminal { cause, .. } => Status::Ended(format!("{cause:?}")),
            NativePairingStatus::Unclaimed { outcome, .. } => {
                Status::Ended(format!("unclaimed: {outcome:?}"))
            }
            NativePairingStatus::Pending(_)
            | NativePairingStatus::Claimed(_)
            | NativePairingStatus::Approved(_)
            | NativePairingStatus::Staging(_) => Status::Waiting,
        }))
    }

    /// The client has marked the record revoked; its cache goes now.
    async fn ended(&mut self, record: &PeerRecord, cause: &str) -> Cycle {
        let key = *record.key();
        self.commands.set_sync(key, None);
        let records = self.commands.records.clone();
        let removed = tokio::task::spawn_blocking(move || records.remove_cache(&key)).await;
        tracing::info!(
            peer = %hex(&key),
            address = %record.address(),
            cause,
            cache_removed = matches!(removed, Ok(Ok(()))),
            "peer gateway ended this gateway's enrollment; no longer read"
        );
        Cycle::Ended
    }

    /// Read the peer into its cache, on a blocking thread. A cache that
    /// cannot continue is emptied and read once more. `None` when stopped.
    async fn read(
        &self,
        record: &PeerRecord,
        receiver: &str,
        access_epoch: u64,
        settle: bool,
    ) -> Option<Result<ReadReport, ReadFailure>> {
        let key = *record.key();
        let records = self.commands.records.clone();
        let (address, receiver) = (record.address(), receiver.to_owned());
        let (wall, clock, stop) = (
            self.wall.clone(),
            self.commands.clock.clone(),
            self.reads.clone(),
        );
        let work = tokio::task::spawn_blocking(move || {
            let slot = records.slot(key);
            let credential = slot
                .load_credential()
                .map_err(|_| ReadFailure::Cache)?
                .ok_or(ReadFailure::Cache)?;
            let id = credential.credential().as_str().to_owned();
            let (secret, pin, _) = credential.into_enrollment().into_parts();
            let identity = NativeIdentity::restore(secret).map_err(|_| ReadFailure::Cache)?;
            let path = records.cache_path(&key);
            let access = || ReaderAccess {
                address,
                identity: &identity,
                pin,
                credential: &id,
                receiver: &receiver,
                access_epoch,
                client_id: CLIENT_ID,
            };
            let mut cache = RetainedCache::open(&path, wall.clone(), clock.clone())?;
            match cache.read(access(), settle, &stop) {
                Err(ReadFailure::ResetRequired) => {
                    drop(cache);
                    tracing::warn!(peer = %hex(&key), "peer gateway cache reset: what it held cannot continue against what the peer serves; reading again");
                    records.remove_cache(&key).map_err(|_| ReadFailure::Cache)?;
                    let mut cache = RetainedCache::open(&path, wall, clock)?;
                    cache.read(access(), true, &stop)
                }
                result => result,
            }
        });
        tokio::pin!(work);
        let mut stopped = self.stopped.clone();
        let result = tokio::select! {
            result = &mut work => result,
            _ = until_stopped(&mut stopped) => {
                // The read's sockets are shut by `signal_stop`; wait for it.
                let _ = (&mut work).await;
                return None;
            }
        };
        Some(result.unwrap_or(Err(ReadFailure::Cache)))
    }

    fn read_done(&self, record: &PeerRecord, report: ReadReport) -> Cycle {
        let key = *record.key();
        for conversation in &report.withdrawn {
            tracing::info!(peer = %hex(&key), conversation_id = conversation.as_str(),
                "peer gateway no longer grants a conversation; removed from its cache");
        }
        self.commands.set_sync(
            key,
            Some(PeerSync {
                state: if report.complete {
                    SyncState::Synced
                } else {
                    SyncState::Syncing
                },
                last_synced_ms: Some(self.wall.unix_milliseconds()),
                conversations: u64::try_from(report.conversations).ok(),
            }),
        );
        Cycle::Settled {
            complete: report.complete,
        }
    }

    fn unread(
        &self,
        record: &PeerRecord,
        status: Option<NativeClientError>,
        read: Option<ReadFailure>,
    ) -> Cycle {
        let key = *record.key();
        let unreachable = matches!(
            status,
            Some(NativeClientError::Io(_) | NativeClientError::Handshake(_))
        ) || read == Some(ReadFailure::Unreachable);
        let previous = self.commands.sync_of(&key);
        self.commands.set_sync(
            key,
            Some(PeerSync {
                state: if unreachable {
                    SyncState::Unreachable
                } else {
                    SyncState::Failed
                },
                last_synced_ms: previous.and_then(|sync| sync.last_synced_ms),
                conversations: previous.and_then(|sync| sync.conversations),
            }),
        );
        tracing::info!(peer = %hex(&key), address = %record.address(),
            status = ?status, read = ?read, "peer gateway read failed; backing off");
        Cycle::Failed
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
    /// The peer ended this enrollment, for the cause given.
    Ended(String),
    /// Still pending on the peer.
    Waiting,
}

/// What one peer's cycle came to.
enum Cycle {
    Settled { complete: bool },
    Waiting,
    Failed,
    Ended,
}

/// `wait` spread over 80%–120%, so peers polled together drift apart.
fn jitter(wait: Duration) -> Duration {
    let spread = getrandom::u32().unwrap_or(u32::MAX / 2) % 401;
    wait.mul_f64(0.8 + f64::from(spread) / 1000.0)
}

fn hex(key: &DeviceKey) -> String {
    key.bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
