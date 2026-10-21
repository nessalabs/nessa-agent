use super::{finished, DurableExecutionAudit, ExecutionAudit, ExecutionOutcome, TestClock};
use crate::conversation::infrastructure::ExecutionAuditFileWriter;
use futures_util::FutureExt;
use nessa_auth::application::ports::Clock;
use nessa_sdk::application::agent_execution::agents::AgentError;
use serde_json::Value;
use std::{
    fs,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    time::Duration,
};
use tokio::{
    runtime::Builder,
    sync::oneshot::{self, Receiver, Sender},
};

const WATCHDOG: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Gate {
    released: Mutex<bool>,
    changed: Condvar,
}
impl Gate {
    fn wait(&self) {
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.changed.wait(released).unwrap();
        }
    }
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.changed.notify_all();
    }
}
struct ReleaseOnDrop(Arc<Gate>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct HeldFileWriter {
    gate: Arc<Gate>,
    entered: Mutex<Option<Sender<()>>>,
    calls: AtomicUsize,
    hold_at: usize,
}
impl ExecutionAuditFileWriter for HeldFileWriter {
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), AgentError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.hold_at {
            if let Some(entered) = self.entered.lock().unwrap().take() {
                let _ = entered.send(());
            }
            self.gate.wait();
        }
        fs::write(path, bytes).map_err(|_| AgentError::AuditFailure)
    }
}
fn held_writer() -> (Arc<HeldFileWriter>, Receiver<()>, ReleaseOnDrop) {
    held_writer_at(1)
}
fn held_writer_at(hold_at: usize) -> (Arc<HeldFileWriter>, Receiver<()>, ReleaseOnDrop) {
    let gate = Arc::new(Gate::default());
    let release = ReleaseOnDrop(gate.clone());
    let (entered, waiting) = oneshot::channel();
    let writer = Arc::new(HeldFileWriter {
        gate,
        entered: Mutex::new(Some(entered)),
        calls: AtomicUsize::new(0),
        hold_at,
    });
    (writer, waiting, release)
}
#[derive(Default)]
struct CountingClock(AtomicUsize);
impl Clock for CountingClock {
    fn unix_milliseconds(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) as u64
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dropped_audit_observers_do_not_grow_physical_work() {
    let root = tempfile::tempdir().unwrap();
    let (writer, entered, release) = held_writer();
    let clock = Arc::new(CountingClock::default());
    let sink = DurableExecutionAudit::with_writer(
        root.path().join("audit"),
        clock.clone(),
        writer.clone(),
    )
    .unwrap();
    drop(sink.record(finished(ExecutionOutcome::Completed)));
    assert_eq!(clock.0.load(Ordering::SeqCst), 0);
    let mut first = sink.record(finished(ExecutionOutcome::Completed));
    assert!(futures_util::poll!(&mut first).is_pending());
    tokio::time::timeout(WATCHDOG, entered)
        .await
        .unwrap()
        .unwrap();
    drop(first);
    for _ in 0..64 {
        let mut cancelled = sink.record(finished(ExecutionOutcome::Cancelled));
        assert!(futures_util::poll!(&mut cancelled).is_pending());
        drop(cancelled);
        assert!(tokio::time::timeout(
            Duration::ZERO,
            sink.record(finished(ExecutionOutcome::OutputLimit)),
        )
        .await
        .is_err());
    }
    assert_eq!(writer.calls.load(Ordering::SeqCst), 1);
    assert_eq!(clock.0.load(Ordering::SeqCst), 1);
    release.0.release();
    tokio::time::timeout(WATCHDOG, sink.record(finished(ExecutionOutcome::Refused)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(writer.calls.load(Ordering::SeqCst), 2);
    assert_eq!(clock.0.load(Ordering::SeqCst), 2);
    let mut rows: Vec<Value> = fs::read_dir(root.path().join("audit"))
        .unwrap()
        .map(|entry| serde_json::from_slice(&fs::read(entry.unwrap().path()).unwrap()).unwrap())
        .collect();
    rows.sort_by_key(|row| row["sequence"].as_u64().unwrap());
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["sequence"], 0);
    assert_eq!(rows[1]["sequence"], 1);
    assert_eq!(rows[0]["observedAtMs"], 0);
    assert_eq!(rows[1]["observedAtMs"], 1);
    assert_eq!(rows[0]["record"]["result"]["outcome"], "completed");
    assert_eq!(rows[1]["record"]["result"]["outcome"], "refused");
    assert_eq!(rows[0]["writerId"], rows[1]["writerId"]);
}

#[test]
fn queued_audit_keeps_admission_after_observer_drop() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let pool_gate = Arc::new(Gate::default());
        let pool_release = ReleaseOnDrop(pool_gate.clone());
        let (entered, waiting) = oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            let _ = entered.send(());
            pool_gate.wait();
        });
        tokio::time::timeout(WATCHDOG, waiting)
            .await
            .unwrap()
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        let (writer, _, release) = held_writer();
        release.0.release();
        let clock = Arc::new(CountingClock::default());
        let sink = DurableExecutionAudit::with_writer(
            root.path().join("audit"),
            clock.clone(),
            writer.clone(),
        )
        .unwrap();
        let mut queued = sink.record(finished(ExecutionOutcome::Completed));
        assert!(futures_util::poll!(&mut queued).is_pending());
        drop(queued);
        for _ in 0..16 {
            assert!(tokio::time::timeout(
                Duration::ZERO,
                sink.record(finished(ExecutionOutcome::Cancelled)),
            )
            .await
            .is_err());
        }
        assert_eq!(writer.calls.load(Ordering::SeqCst), 0);
        assert_eq!(clock.0.load(Ordering::SeqCst), 0);
        pool_release.0.release();
        tokio::time::timeout(WATCHDOG, blocker)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(WATCHDOG, sink.record(finished(ExecutionOutcome::Refused)))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(writer.calls.load(Ordering::SeqCst), 2);
        assert_eq!(fs::read_dir(root.path().join("audit")).unwrap().count(), 2);
    });
}

struct FaultOnceWriter {
    calls: AtomicUsize,
    panic: bool,
}
impl ExecutionAuditFileWriter for FaultOnceWriter {
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), AgentError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            assert!(!self.panic, "physical writer fault");
            return Err(AgentError::AuditFailure);
        }
        fs::write(path, bytes).map_err(|_| AgentError::AuditFailure)
    }
}
struct FaultOnceClock(AtomicUsize);
impl Clock for FaultOnceClock {
    fn unix_milliseconds(&self) -> u64 {
        assert_ne!(self.0.fetch_add(1, Ordering::SeqCst), 0, "clock fault");
        1234
    }
}
#[tokio::test]
async fn physical_audit_faults_are_typed_and_release_capacity() {
    for panic in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let sink = DurableExecutionAudit::with_writer(
            root.path().join("audit"),
            Arc::new(TestClock),
            Arc::new(FaultOnceWriter {
                calls: AtomicUsize::new(0),
                panic,
            }),
        )
        .unwrap();
        assert_eq!(
            sink.record(finished(ExecutionOutcome::Completed)).await,
            Err(AgentError::AuditFailure)
        );
        tokio::time::timeout(WATCHDOG, sink.record(finished(ExecutionOutcome::Completed)))
            .await
            .unwrap()
            .unwrap();
    }
    let root = tempfile::tempdir().unwrap();
    let sink = DurableExecutionAudit::new(
        root.path().join("audit"),
        Arc::new(FaultOnceClock(AtomicUsize::new(0))),
    )
    .unwrap();
    assert_eq!(
        sink.record(finished(ExecutionOutcome::Completed)).await,
        Err(AgentError::AuditFailure)
    );
    tokio::time::timeout(WATCHDOG, sink.record(finished(ExecutionOutcome::Completed)))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn independent_audit_instances_do_not_share_admission() {
    let root = tempfile::tempdir().unwrap();
    let (writer, entered, release) = held_writer();
    let first =
        DurableExecutionAudit::with_writer(root.path().join("first"), Arc::new(TestClock), writer)
            .unwrap();
    let mut stalled = first.record(finished(ExecutionOutcome::Completed));
    assert!(futures_util::poll!(&mut stalled).is_pending());
    tokio::time::timeout(WATCHDOG, entered)
        .await
        .unwrap()
        .unwrap();
    drop(stalled);
    let second =
        DurableExecutionAudit::new(root.path().join("second"), Arc::new(TestClock)).unwrap();
    tokio::time::timeout(
        WATCHDOG,
        second.record(finished(ExecutionOutcome::Completed)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(fs::read_dir(root.path().join("second")).unwrap().count(), 1);
    release.0.release();
    tokio::time::timeout(
        WATCHDOG,
        first.record(finished(ExecutionOutcome::Completed)),
    )
    .await
    .unwrap()
    .unwrap();
}

#[test]
fn audit_without_a_runtime_is_a_typed_pre_effect_failure() {
    let root = tempfile::tempdir().unwrap();
    let clock = Arc::new(CountingClock::default());
    let sink = DurableExecutionAudit::new(root.path().join("audit"), clock.clone()).unwrap();
    assert_eq!(
        sink.record(finished(ExecutionOutcome::Completed))
            .now_or_never(),
        Some(Err(AgentError::AuditFailure))
    );
    assert_eq!(clock.0.load(Ordering::SeqCst), 0);
    assert_eq!(fs::read_dir(root.path().join("audit")).unwrap().count(), 0);
}

#[path = "physical/attachment_deadlines.rs"]
mod attachment_deadlines;

fn decline_bytes(writer: &str, sequence: u64, at: u64, actor: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "recordId":"decline-stable-identity", "writerId":writer, "sequence":sequence,
        "observedAtMs":at,
        "record":{"kind":"turn_resumed","sessionId":"session","executionId":"turn",
            "cause":"interrupted","dispatched":true,"submittedBy":{"principalId":"person","surfaceId":"panel","requestId":"submit"},
            "actor":{"principalId":"person","surfaceId":"panel","requestId":actor},"resumption":{"kind":"declined"}}
    })).unwrap()
}

#[test]
fn stable_decline_publication_preserves_first_metadata_and_rejects_conflicting_input() {
    use crate::conversation::infrastructure::audit::PrivateExecutionAuditFileWriter;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("decline.json");
    let first = decline_bytes("11111111-1111-4111-8111-111111111111", 0, 100, "close");
    let retry = decline_bytes("22222222-2222-4222-8222-222222222222", 8, 200, "close");
    PrivateExecutionAuditFileWriter
        .write_once(&path, &first)
        .unwrap();
    let original = fs::read(&path).unwrap();
    PrivateExecutionAuditFileWriter
        .write_once(&path, &retry)
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(
        PrivateExecutionAuditFileWriter.write_once(
            &path,
            &decline_bytes(
                "22222222-2222-4222-8222-222222222222",
                9,
                300,
                "other-close"
            )
        ),
        Err(AgentError::AuditFailure)
    );
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn concurrent_decline_writers_acknowledge_one_create_only_winner() {
    use crate::conversation::infrastructure::audit::PrivateExecutionAuditFileWriter;
    let root = tempfile::tempdir().unwrap();
    let path = Arc::new(root.path().join("decline.json"));
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let writes: Vec<_> = [
        "11111111-1111-4111-8111-111111111111",
        "22222222-2222-4222-8222-222222222222",
    ]
    .into_iter()
    .map(|id| {
        let path = path.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            let bytes = decline_bytes(id, 0, 100, "close");
            PrivateExecutionAuditFileWriter.write_once(&path, &bytes)
        })
    })
    .collect();
    for write in writes {
        write.join().unwrap().unwrap();
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    let value: Value = serde_json::from_slice(&fs::read(path.as_ref()).unwrap()).unwrap();
    assert_eq!(value["record"]["actor"]["requestId"], "close");
}

#[test]
fn a_lost_decline_write_ack_rejoins_the_same_durable_record() {
    use crate::conversation::infrastructure::audit::PrivateExecutionAuditFileWriter;
    struct LoseAck(AtomicUsize);
    impl ExecutionAuditFileWriter for LoseAck {
        fn write(&self, _: &Path, _: &[u8]) -> Result<(), AgentError> {
            unreachable!()
        }
        fn write_once(&self, path: &Path, bytes: &[u8]) -> Result<(), AgentError> {
            PrivateExecutionAuditFileWriter.write_once(path, bytes)?;
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(AgentError::AuditFailure)
            } else {
                Ok(())
            }
        }
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("decline.json");
    let writer = LoseAck(AtomicUsize::new(0));
    let bytes = decline_bytes("11111111-1111-4111-8111-111111111111", 0, 100, "close");
    assert_eq!(
        writer.write_once(&path, &bytes),
        Err(AgentError::AuditFailure)
    );
    let original = fs::read(&path).unwrap();
    writer.write_once(&path, &bytes).unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn malformed_oversized_or_duplicate_decline_receipts_are_never_replaced() {
    use crate::conversation::infrastructure::audit::PrivateExecutionAuditFileWriter;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("decline.json");
    let bytes = decline_bytes("11111111-1111-4111-8111-111111111111", 0, 100, "close");
    PrivateExecutionAuditFileWriter
        .write_once(&path, &bytes)
        .unwrap();
    let duplicate = String::from_utf8(bytes.clone())
        .unwrap()
        .replacen("\"sequence\":0", "\"sequence\":0,\"sequence\":0", 1)
        .into_bytes();
    let nested = String::from_utf8(bytes.clone())
        .unwrap()
        .replacen(
            "\"kind\":\"declined\"",
            "\"kind\":\"declined\",\"kind\":\"declined\"",
            1,
        )
        .into_bytes();
    for invalid in [
        b"{}".to_vec(),
        vec![b' '; bytes.len() + 4097],
        duplicate,
        nested,
    ] {
        fs::write(&path, &invalid).unwrap();
        assert_eq!(
            PrivateExecutionAuditFileWriter.write_once(&path, &bytes),
            Err(AgentError::AuditFailure)
        );
        assert_eq!(fs::read(&path).unwrap(), invalid);
    }
}
