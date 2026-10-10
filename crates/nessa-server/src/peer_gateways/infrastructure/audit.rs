//! Durable peer command evidence: one private file per record, named by the
//! record's own id, published only if that name is unused, then synced with
//! its directory before the record is acknowledged. Separate files cannot
//! tear each other, and a record is never replaced.
//!
//! ```text
//! record() --(sequence, recordId, observedAtMs under one lock; try_send)--> queue (AUDIT_QUEUE)
//!   queue --> one writer thread: temp file, sync, publish, sync directory --> ack
//! close() --> nothing more is taken; drained(limit) waits for the writer on the
//!             injected monotonic clock, and past it abandons what is left
//! ```
//! Arrows are handoffs, in order. This adapter is the one authority over the
//! order of peer evidence: each record is given its `sequence` the moment
//! `record` is called, in the caller's own order, under the same lock that
//! puts it on the queue, and one writer thread writes the queue in that
//! order. The future `record` returns only reports whether the record was
//! kept: a caller that drops it, or never polls it, has still handed the
//! record over. The poller relies on that to order its records by its turn
//! (design row R18).
//!
//! The record id, `sequence` and `observedAtMs` are assigned here: the id
//! fresh, the sequence counting from 1 in each process, the time from the
//! injected wall clock when this adapter took the record. That is not claimed
//! as the time of the peer's own effect. A full queue refuses the record at
//! once, without waiting, and logs the sequence it would have had: past
//! [`AUDIT_QUEUE`] waiting records the store is stalled, and an answer that
//! waited would only hold its caller. A process that stops between a
//! command's effect and its outcome record leaves the intent alone: the
//! intent says what was asked, and the peer record on disk says what it came
//! to. A record still queued when shutdown's drain runs out is not written
//! (logged by sequence); one the writer had begun may still land.
use crate::peer_gateways::application::{
    CacheState, PeerAudit, PeerAuditFuture, PeerAuditRecord, PeerAuditUnavailable, PeerHolding,
    PeerState, PollerCause,
};
use nessa_auth::{
    application::ports::Clock,
    domain::{pairing::DeviceKey, PrincipalId},
};
use nessa_local_storage::{sync_directory, PrivateTempFile};
use nessa_protocol::clock::Clock as MonotonicClock;
use nessa_protocol::pairing::socket::WAKE_TICK;
use serde_json::{json, Value};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex, PoisonError,
    },
    thread::JoinHandle,
    time::Duration,
};
use tokio::sync::{oneshot, watch};
use uuid::Uuid;

/// The most records waiting to be written. Each waits for the ones before it,
/// so an owner command's record can wait behind this many: at about 20 ms a
/// synced write that is about 1.3 s, well inside the command's own
/// [`super::AUDIT_DEADLINE`] (5 s).
pub const AUDIT_QUEUE: usize = 64;

/// Called by the writer with each record's sequence before it writes it; a
/// test stalls the store with it.
type BeforeWrite = Arc<dyn Fn(u64) + Send + Sync>;

/// Peer command evidence in one private directory, written in the order it
/// was handed over.
pub struct DurablePeerAudit {
    wall: Arc<dyn Clock>,
    clock: Arc<dyn MonotonicClock>,
    /// The next sequence and the queue's sending end, under one lock, so the
    /// order of sequences is the order of the queue. No sender: closed.
    queue: Mutex<Queue>,
    writer: Mutex<Option<JoinHandle<()>>>,
    shared: Arc<Shared>,
}
struct Queue {
    next: u64,
    sender: Option<SyncSender<Queued>>,
}
/// One record on its way to the writer.
struct Queued {
    sequence: u64,
    id: String,
    value: Value,
    kept: oneshot::Sender<Result<(), PeerAuditUnavailable>>,
}
/// What the adapter and its writer thread both see.
struct Shared {
    /// The last sequence the writer finished with, written or not.
    handled: AtomicU64,
    /// Shutdown's drain ran out: write nothing more.
    abandoned: AtomicBool,
    /// Set by the writer thread as the last thing it does.
    finished: watch::Sender<bool>,
}
impl DurablePeerAudit {
    /// Records go in `directory`, which composition has already created
    /// private; an unsafe or missing one fails each write, never repaired.
    /// `wall` stamps each record's observation time; `clock` bounds
    /// shutdown's drain, the same monotonic clock the peer commands' own
    /// deadlines use.
    pub fn new(directory: PathBuf, wall: Arc<dyn Clock>, clock: Arc<dyn MonotonicClock>) -> Self {
        Self::start(directory, wall, clock, AUDIT_QUEUE, None)
    }

    /// The same adapter with a queue of `capacity`, whose writer calls
    /// `before_write` with each record's sequence before writing it.
    #[cfg(test)]
    pub(crate) fn with_writer_hook(
        directory: PathBuf,
        wall: Arc<dyn Clock>,
        clock: Arc<dyn MonotonicClock>,
        capacity: usize,
        before_write: BeforeWrite,
    ) -> Self {
        Self::start(directory, wall, clock, capacity, Some(before_write))
    }

    fn start(
        directory: PathBuf,
        wall: Arc<dyn Clock>,
        clock: Arc<dyn MonotonicClock>,
        capacity: usize,
        before_write: Option<BeforeWrite>,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let shared = Arc::new(Shared {
            handled: AtomicU64::new(0),
            abandoned: AtomicBool::new(false),
            finished: watch::channel(false).0,
        });
        let spawned = std::thread::Builder::new()
            .name("peer-audit".into())
            .spawn({
                let shared = shared.clone();
                move || write_in_order(&directory, &receiver, &shared, before_write.as_ref())
            });
        let (sender, writer) = match spawned {
            Ok(writer) => (Some(sender), Some(writer)),
            Err(error) => {
                // Every record is refused: no evidence, so no owner effect.
                tracing::error!(%error, "peer gateway audit writer could not start");
                shared.finished.send_replace(true);
                (None, None)
            }
        };
        Self {
            wall,
            clock,
            queue: Mutex::new(Queue { next: 1, sender }),
            writer: Mutex::new(writer),
            shared,
        }
    }

    /// Take nothing more: every later record is refused at once. What is
    /// queued is still written, in order.
    pub fn close(&self) {
        drop(self.lock_queue().sender.take());
    }

    /// Close, and wait for the writer to write what was queued, until the
    /// injected clock has moved `limit` past now. `true` once the writer
    /// thread has ended. Past the limit the records still queued are not
    /// written (logged by sequence), the writer is left to finish the write
    /// it is in or end with the process, and this answers `false`.
    pub async fn drained(&self, limit: Duration) -> bool {
        self.close();
        let deadline = self
            .clock
            .elapsed_ms()
            .saturating_add(u64::try_from(limit.as_millis()).unwrap_or(u64::MAX));
        let mut finished = self.shared.finished.subscribe();
        loop {
            if *finished.borrow_and_update() {
                let writer = self.lock_writer().take();
                if let Some(writer) = writer {
                    // It has returned from its loop; the join only reaps it.
                    let _ = tokio::task::spawn_blocking(move || writer.join()).await;
                }
                return true;
            }
            if self.clock.elapsed_ms() >= deadline {
                self.shared.abandoned.store(true, Ordering::SeqCst);
                let last = self.lock_queue().next.saturating_sub(1);
                let handled = self.shared.handled.load(Ordering::SeqCst);
                tracing::error!(
                    first = handled.saturating_add(1),
                    last,
                    "peer gateway audit records not written before shutdown"
                );
                drop(self.lock_writer().take());
                return false;
            }
            tokio::select! {
                _ = finished.changed() => {}
                () = tokio::time::sleep(WAKE_TICK) => {}
            }
        }
    }

    /// Whether the writer thread has ended.
    #[cfg(test)]
    pub(crate) fn writer_finished(&self) -> bool {
        *self.shared.finished.borrow()
    }

    /// Give `record` its sequence, id and observation time and queue it, all
    /// under the queue's lock. The sequence it is given, or why not.
    fn enqueue(
        &self,
        record: &PeerAuditRecord,
        kept: oneshot::Sender<Result<(), PeerAuditUnavailable>>,
    ) -> Result<u64, PeerAuditUnavailable> {
        let mut queue = self.lock_queue();
        let sequence = queue.next;
        let Some(sender) = &queue.sender else {
            tracing::error!(
                sequence,
                "peer gateway audit record refused: the audit is closed"
            );
            return Err(PeerAuditUnavailable);
        };
        let id = Uuid::new_v4().to_string();
        let mut value = record_value(record);
        value["recordId"] = json!(id);
        value["sequence"] = json!(sequence);
        value["observedAtMs"] = json!(self.wall.unix_milliseconds());
        match sender.try_send(Queued {
            sequence,
            id,
            value,
            kept,
        }) {
            Ok(()) => {
                queue.next = sequence.saturating_add(1);
                Ok(sequence)
            }
            Err(TrySendError::Full(_)) => {
                tracing::error!(
                    sequence,
                    "peer gateway audit record refused: the queue is full"
                );
                Err(PeerAuditUnavailable)
            }
            Err(TrySendError::Disconnected(_)) => {
                tracing::error!(sequence, "peer gateway audit record refused: no writer");
                Err(PeerAuditUnavailable)
            }
        }
    }

    fn lock_queue(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn lock_writer(&self) -> std::sync::MutexGuard<'_, Option<JoinHandle<()>>> {
        self.writer.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
impl PeerAudit for DurablePeerAudit {
    /// Queued now, before the future is polled: dropping the future never
    /// drops the record.
    fn record(&self, record: PeerAuditRecord) -> PeerAuditFuture<'_> {
        let (kept, answer) = oneshot::channel();
        let queued = self.enqueue(&record, kept);
        Box::pin(async move {
            queued?;
            answer.await.map_err(|_| PeerAuditUnavailable)?
        })
    }
}

/// The writer thread: each queued record in turn, until the queue is closed
/// and empty.
fn write_in_order(
    directory: &Path,
    queue: &Receiver<Queued>,
    shared: &Shared,
    before_write: Option<&BeforeWrite>,
) {
    for queued in queue {
        if let Some(before_write) = before_write {
            before_write(queued.sequence);
        }
        let written = if shared.abandoned.load(Ordering::SeqCst) {
            tracing::error!(
                sequence = queued.sequence,
                "peer gateway audit record not written: shutdown"
            );
            Err(PeerAuditUnavailable)
        } else {
            write_one(directory, &queued).map_err(|error| {
                tracing::error!(%error, sequence = queued.sequence,
                    "peer gateway audit record was not committed");
                PeerAuditUnavailable
            })
        };
        shared.handled.store(queued.sequence, Ordering::SeqCst);
        // The caller may have stopped listening; the record stands either way.
        let _ = queued.kept.send(written);
    }
    shared.finished.send_replace(true);
}

fn write_one(directory: &Path, queued: &Queued) -> std::io::Result<()> {
    let mut file = PrivateTempFile::new_in(directory)?;
    serde_json::to_writer(file.as_file_mut(), &queued.value)?;
    file.as_file_mut().write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.publish(&directory.join(format!("{}.json", queued.id)))?;
    sync_directory(directory)
}

fn hex(key: &DeviceKey) -> String {
    key.bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn person(initiator: &PrincipalId) -> Value {
    json!({"kind": "principal", "principalId": initiator.as_str()})
}

fn state(of: &PeerState) -> Value {
    phase(of)
}

fn phase(state: &PeerState) -> Value {
    match state {
        PeerState::Absent => json!({"phase": "absent"}),
        PeerState::Pending { address } => {
            json!({"phase": "pending", "address": address.to_string()})
        }
        PeerState::Active {
            address,
            credential,
            receiver,
        } => json!({"phase": "active", "address": address.to_string(),
            "credentialId": credential, "receiverId": receiver}),
        PeerState::Revoked { address } => {
            json!({"phase": "revoked", "address": address.to_string()})
        }
        PeerState::Unreadable => json!({"phase": "unreadable"}),
        PeerState::Unknown => json!({"phase": "unknown"}),
        PeerState::NotRead => json!({"phase": "not_read"}),
        PeerState::CacheUnconfirmed { record } => {
            let mut value = phase(record);
            value["cache"] = json!("unconfirmed");
            value
        }
    }
}

fn holding(holding: &PeerHolding) -> Value {
    let mut value = state(&holding.record);
    value["cache"] = json!(match holding.cache {
        CacheState::Present => "present",
        CacheState::Absent => "absent",
        CacheState::Unknown => "unknown",
    });
    value
}

/// The cause's name, and what it names: the status's own cause or outcome.
fn cause(cause: &PollerCause) -> (&'static str, Option<&str>) {
    match cause {
        PollerCause::Approved => ("peer_approved", None),
        PollerCause::Ended { detail } => ("peer_ended", detail.as_deref()),
        PollerCause::ResetRequired => ("cache_reset_required", None),
        PollerCause::CacheDamaged => ("cache_damaged", None),
        PollerCause::Withdrawn { .. } => ("peer_withdrew", None),
    }
}

fn outcome(outcome: &Result<(), &'static str>) -> Value {
    match outcome {
        Ok(()) => json!({"result": "succeeded"}),
        Err(code) => json!({"result": "refused", "code": code}),
    }
}

/// Everything except the record's id, sequence and observation time.
pub(super) fn record_value(record: &PeerAuditRecord) -> Value {
    match record {
        PeerAuditRecord::EnrollRequested {
            operation,
            initiator,
            address,
        } => json!({
            "kind": "peer_enroll_requested",
            "operationId": operation.to_string(),
            "target": {"address": address.to_string()},
            "cause": "owner_requested",
            "initiator": person(initiator),
        }),
        PeerAuditRecord::EnrollFinished {
            operation,
            initiator,
            address,
            peer,
            before,
            after,
            outcome: result,
        } => json!({
            "kind": "peer_enroll_finished",
            "operationId": operation.to_string(),
            "target": {"address": address.to_string(), "peerKey": peer.as_ref().map(hex)},
            "transition": {"before": state(before), "after": state(after)},
            "outcome": outcome(result),
            "cause": "owner_requested",
            "initiator": person(initiator),
        }),
        PeerAuditRecord::ForgetRequested {
            operation,
            initiator,
            peer,
        } => json!({
            "kind": "peer_forget_requested",
            "operationId": operation.to_string(),
            "target": {"peerKey": hex(peer)},
            "cause": "owner_requested",
            "initiator": person(initiator),
        }),
        PeerAuditRecord::ForgetFinished {
            operation,
            initiator,
            peer,
            before,
            after,
            outcome: result,
        } => json!({
            "kind": "peer_forget_finished",
            "operationId": operation.to_string(),
            "target": {"peerKey": hex(peer)},
            "transition": {"before": state(before), "after": state(after)},
            "outcome": outcome(result),
            "cause": "owner_requested",
            "initiator": person(initiator),
        }),
        PeerAuditRecord::PollerChanged {
            operation,
            peer,
            cause: why,
            before,
            after,
            outcome: result,
        } => {
            let (name, detail) = cause(why);
            let conversations = match why {
                PollerCause::Withdrawn { conversations } => Some(conversations),
                _ => None,
            };
            json!({
                "kind": "peer_poller_changed",
                "operationId": operation.to_string(),
                "target": {"peerKey": hex(peer), "conversationIds": conversations},
                "transition": {"before": holding(before), "after": holding(after)},
                "outcome": outcome(result),
                "cause": name,
                "causeDetail": detail,
                "initiator": {"kind": "system", "component": "peer_poller"},
            })
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/peer_gateways/audit.rs"]
mod tests;
