//! Logical read budgets and an independent failing wall-time watchdog (#997).
use nessa_client_core::retained::ReadStop;
use nessa_device_client::NativeSession;
use nessa_protocol::clock::Clock;
use std::path::Path;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct ManualClock(AtomicU64);
impl Clock for ManualClock {
    fn elapsed_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
impl ManualClock {
    /// Catalogue progress is durable before the source can enter this head.
    pub(super) fn expire_at_head(&self, phase: &Path, stop: &ReadStop) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if phase.try_exists().expect("synthetic head phase") {
                self.0.store(5_000, Ordering::SeqCst);
                return true;
            }
            if Instant::now() >= deadline || stop.is_stopped() {
                stop.stop();
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

/// A logical clock must never allow a stalled integration test to pass or hang.
pub(super) fn watched<T>(native: &NativeSession, operation: impl FnOnce() -> T) -> T {
    thread::scope(|scope| {
        let (finished, completion) = mpsc::channel();
        let watchdog = scope.spawn(move || {
            if completion.recv_timeout(Duration::from_secs(30))
                == Err(mpsc::RecvTimeoutError::Timeout)
            {
                let _ = native.request_close();
                true
            } else {
                false
            }
        });
        let result = operation();
        let _ = finished.send(());
        assert!(
            !watchdog.join().expect("native watchdog"),
            "native operation exceeded wall watchdog"
        );
        result
    })
}
