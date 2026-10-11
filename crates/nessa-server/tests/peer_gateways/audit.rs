//! The durable peer audit as an ordered writer: a record is queued when it is
//! handed over, whether or not its answer is awaited, and shutdown's drain is
//! bounded by the injected clock (design row R18).
use super::*;
use nessa_auth::domain::pairing::DeviceKey;
use std::sync::Condvar;

/// A wall clock that moves one millisecond each time it is read.
struct Ticks(AtomicU64);
impl Clock for Ticks {
    fn unix_milliseconds(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
}
/// A monotonic clock that moves only when the test moves it.
struct Manual(AtomicU64);
impl MonotonicClock for Manual {
    fn elapsed_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// Holds the writer before it writes record `at`, until opened.
struct Stall {
    at: u64,
    reached: AtomicBool,
    open: Mutex<bool>,
    opened: Condvar,
}
impl Stall {
    fn new(at: u64) -> Arc<Self> {
        Arc::new(Self {
            at,
            reached: AtomicBool::new(false),
            open: Mutex::new(false),
            opened: Condvar::new(),
        })
    }
    fn hook(self: &Arc<Self>) -> BeforeWrite {
        let stall = self.clone();
        Arc::new(move |sequence| {
            if sequence != stall.at {
                return;
            }
            stall.reached.store(true, Ordering::SeqCst);
            let mut open = stall.open.lock().unwrap();
            while !*open {
                open = stall.opened.wait(open).unwrap();
            }
        })
    }
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }
}

fn record() -> PeerAuditRecord {
    PeerAuditRecord::ForgetRequested {
        operation: Uuid::new_v4(),
        initiator: PrincipalId::new("owner").unwrap(),
        peer: DeviceKey::new([7; 32]),
    }
}

fn directory(root: &tempfile::TempDir) -> PathBuf {
    let directory = root.path().join("audit");
    nessa_local_storage::create_directory(&directory).unwrap();
    directory
}

/// The sequences of the records written, in order.
fn written(directory: &Path) -> Vec<u64> {
    let mut sequences: Vec<u64> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let value: Value =
                serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap();
            value["sequence"].as_u64().unwrap()
        })
        .collect();
    sequences.sort_unstable();
    sequences
}

/// Wait, a scheduler tick at a time, until `done`.
async fn until(done: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("never reached");
}

/// Shutdown's drain is one bound on the injected clock. Records before the
/// one the store stalls on are written; once the clock passes the bound the
/// drain answers, the rest are abandoned, and the writer ends as soon as the
/// stalled write returns, leaving no thread behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_drains_within_one_deadline() {
    let root = tempfile::tempdir().unwrap();
    let directory = directory(&root);
    let stall = Stall::new(3);
    let clock = Arc::new(Manual(AtomicU64::new(0)));
    let audit = Arc::new(DurablePeerAudit::with_writer_hook(
        directory.clone(),
        Arc::new(Ticks(AtomicU64::new(1))),
        clock.clone(),
        AUDIT_QUEUE,
        stall.hook(),
    ));
    let answers: Vec<_> = (0..5).map(|_| audit.record(record())).collect();
    until(|| stall.reached.load(Ordering::SeqCst)).await;
    assert_eq!(written(&directory), [1, 2]);

    let drain = tokio::spawn({
        let audit = audit.clone();
        async move { audit.drained(Duration::from_secs(5)).await }
    });
    // While the clock stands the drain waits for the stalled store.
    tokio::time::sleep(WAKE_TICK * 3).await;
    assert!(!drain.is_finished(), "the drain ends only at its bound");
    // A record handed over after close is refused at once.
    assert_eq!(audit.record(record()).await, Err(PeerAuditUnavailable));
    clock.0.store(5_000, Ordering::SeqCst);
    assert!(!drain.await.unwrap(), "the bound passed with records left");

    stall.release();
    until(|| audit.writer_finished()).await;
    assert_eq!(
        written(&directory),
        [1, 2],
        "nothing past the stall is written"
    );
    let mut kept = Vec::new();
    for answer in answers {
        kept.push(answer.await.is_ok());
    }
    assert_eq!(kept, [true, true, false, false, false]);
}

/// A record whose answer is never awaited is still written: it is queued when
/// it is handed over, not when its future is polled. A drain with time to
/// spare writes it and ends the writer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unpolled_poller_record_is_still_written() {
    let root = tempfile::tempdir().unwrap();
    let directory = directory(&root);
    let audit = DurablePeerAudit::new(
        directory.clone(),
        Arc::new(Ticks(AtomicU64::new(1))),
        Arc::new(Manual(AtomicU64::new(0))),
    );
    drop(audit.record(record()));
    drop(audit.record(record()));
    assert!(audit.drained(Duration::from_secs(5)).await);
    assert_eq!(written(&directory), [1, 2]);
}

/// A writer that panics still ends as finished: the drain returns at once,
/// on a clock that never moves, reporting that it did not write what was
/// queued, and the record it was writing is not kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_writer_that_panics_does_not_hold_the_drain() {
    let root = tempfile::tempdir().unwrap();
    let directory = directory(&root);
    let audit = DurablePeerAudit::with_writer_hook(
        directory.clone(),
        Arc::new(Ticks(AtomicU64::new(1))),
        Arc::new(Manual(AtomicU64::new(0))),
        AUDIT_QUEUE,
        Arc::new(|_| std::panic::resume_unwind(Box::new("the writer failed"))),
    );
    assert_eq!(audit.record(record()).await, Err(PeerAuditUnavailable));
    let drained = tokio::time::timeout(
        Duration::from_secs(30),
        audit.drained(Duration::from_secs(5)),
    )
    .await
    .expect("the drain does not wait on a writer that is gone");
    assert!(!drained, "a writer that panicked did not drain");
    assert!(written(&directory).is_empty());
}
