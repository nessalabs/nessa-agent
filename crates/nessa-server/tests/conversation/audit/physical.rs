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
}
impl ExecutionAuditFileWriter for HeldFileWriter {
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(entered) = self.entered.lock().unwrap().take() {
            let _ = entered.send(());
        }
        self.gate.wait();
        fs::write(path, bytes).map_err(|_| AgentError::AuditFailure)
    }
}
fn held_writer() -> (Arc<HeldFileWriter>, Receiver<()>, ReleaseOnDrop) {
    let gate = Arc::new(Gate::default());
    let release = ReleaseOnDrop(gate.clone());
    let (entered, waiting) = oneshot::channel();
    let writer = Arc::new(HeldFileWriter {
        gate,
        entered: Mutex::new(Some(entered)),
        calls: AtomicUsize::new(0),
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
