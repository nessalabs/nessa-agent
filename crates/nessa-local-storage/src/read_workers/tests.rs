use super::{ReadWorkerError, ReadWorkers};
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::Poll;
use std::time::Duration;
use tokio::sync::Notify;

struct TestReadGate {
    entered: Notify,
    panic_after_release: bool,
    open: Mutex<bool>,
    released: Condvar,
}

impl TestReadGate {
    fn wait(&self) {
        self.entered.notify_one();
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.released.wait(open).unwrap();
        }
        // Panic models physical work failure, not poisoning the fixture gate.
        drop(open);
        assert!(!self.panic_after_release, "injected read worker panic");
    }

    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.released.notify_all();
    }
}

#[tokio::test]
async fn reported_worker_panic_fences_reads_and_retains_other_work() {
    let workers = ReadWorkers::new();
    let gate = Arc::new(TestReadGate {
        entered: Notify::new(),
        panic_after_release: false,
        open: Mutex::new(false),
        released: Condvar::new(),
    });
    let live = {
        let workers = workers.clone();
        let gate = gate.clone();
        tokio::spawn(async move { workers.run("held-physical-read", move || gate.wait()).await })
    };
    tokio::time::timeout(Duration::from_secs(10), gate.entered.notified())
        .await
        .unwrap();
    let result = workers
        .run("failed-physical-read", || panic!("physical read failed"))
        .await;
    // Observe the actual admission boundary before another run could reap the handle.
    let admission = workers.admit();
    let executions = Arc::new(AtomicUsize::new(0));
    let attempted = executions.clone();
    let next = workers
        .run("read-after-panic", move || {
            attempted.fetch_add(1, Ordering::SeqCst);
        })
        .await;
    let mut drain = Box::pin(workers.shutdown());
    let pending =
        std::future::poll_fn(|cx| Poll::Ready(matches!(drain.as_mut().poll(cx), Poll::Pending)))
            .await;
    drop(drain);
    let mut replacement = Box::pin(workers.shutdown());
    let replacement_pending = std::future::poll_fn(|cx| {
        Poll::Ready(matches!(replacement.as_mut().poll(cx), Poll::Pending))
    })
    .await;
    // Release actual physical work before assertions so a failed probe is finite.
    gate.release();
    let shutdown = replacement.await;
    live.await.unwrap().unwrap();
    assert_eq!(result, Err(ReadWorkerError::WorkerPanicked));
    assert_eq!(admission, Err(ReadWorkerError::WorkerPanicked));
    assert_eq!(next, Err(ReadWorkerError::WorkerPanicked));
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert!(
        pending && replacement_pending,
        "cancelled shutdown retains the live physical join"
    );
    assert_eq!(shutdown, Err(ReadWorkerError::WorkerPanicked));
    assert_eq!(
        workers.shutdown().await,
        Err(ReadWorkerError::WorkerPanicked)
    );
}

async fn cancelled_read_panic_fences_without_reaping(result_panics: bool) {
    struct ReadResult(bool);
    impl Drop for ReadResult {
        fn drop(&mut self) {
            assert!(!self.0, "undeliverable read result cleanup failed");
        }
    }
    let workers = ReadWorkers::new();
    let gate = Arc::new(TestReadGate {
        entered: Notify::new(),
        panic_after_release: !result_panics,
        open: Mutex::new(false),
        released: Condvar::new(),
    });
    let read = {
        let workers = workers.clone();
        let gate = gate.clone();
        tokio::spawn(async move {
            workers
                .run("cancelled-panic-read", move || {
                    gate.wait();
                    ReadResult(result_panics)
                })
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), gate.entered.notified())
        .await
        .unwrap();
    read.abort();
    assert!(matches!(read.await, Err(error) if error.is_cancelled()));
    gate.release();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !workers.state.lock().unwrap().joins[0].is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Neither the cancelled observer nor a later run has persisted/reaped the fault.
    let admission = workers.admit();
    let shutdown = workers.shutdown().await;
    assert_eq!(admission, Err(ReadWorkerError::WorkerPanicked));
    assert_eq!(shutdown, Err(ReadWorkerError::WorkerPanicked));
    assert_eq!(
        workers.shutdown().await,
        Err(ReadWorkerError::WorkerPanicked)
    );
}

#[tokio::test]
async fn cancelled_read_waiter_panic_fences_without_reaping() {
    cancelled_read_panic_fences_without_reaping(false).await;
}
#[tokio::test]
async fn cancelled_read_result_drop_panic_fences_without_reaping() {
    cancelled_read_panic_fences_without_reaping(true).await;
}

#[tokio::test]
async fn shutdown_joins_later_gated_work_after_panic_and_cancelled_waiter() {
    let workers = ReadWorkers::new();
    let failed_gate = Arc::new(TestReadGate {
        entered: Notify::new(),
        panic_after_release: true,
        open: Mutex::new(false),
        released: Condvar::new(),
    });
    let held_gate = Arc::new(TestReadGate {
        entered: Notify::new(),
        panic_after_release: false,
        open: Mutex::new(false),
        released: Condvar::new(),
    });
    struct ReleaseGate(Arc<TestReadGate>);
    impl Drop for ReleaseGate {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    let _failed_release = ReleaseGate(failed_gate.clone());
    let _held_release = ReleaseGate(held_gate.clone());
    let failed = {
        let workers = workers.clone();
        let gate = failed_gate.clone();
        tokio::spawn(async move { workers.run("first-failed-read", move || gate.wait()).await })
    };
    tokio::time::timeout(Duration::from_secs(10), failed_gate.entered.notified())
        .await
        .unwrap();
    let finished = Arc::new(AtomicUsize::new(0));
    let held = {
        let workers = workers.clone();
        let gate = held_gate.clone();
        let finished = finished.clone();
        tokio::spawn(async move {
            workers
                .run("later-held-read", move || {
                    gate.wait();
                    finished.fetch_add(1, Ordering::SeqCst);
                })
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), held_gate.entered.notified())
        .await
        .unwrap();
    failed_gate.release();
    let fault = failed.await.unwrap();
    let mut first = Box::pin(workers.shutdown());
    let first_pending =
        std::future::poll_fn(|cx| Poll::Ready(matches!(first.as_mut().poll(cx), Poll::Pending)))
            .await;
    drop(first);
    let mut second = Box::pin(workers.shutdown());
    let second_pending =
        std::future::poll_fn(|cx| Poll::Ready(matches!(second.as_mut().poll(cx), Poll::Pending)))
            .await;
    let before_release = finished.load(Ordering::SeqCst);
    held_gate.release();
    let shutdown = second.await;
    held.await.unwrap().unwrap();
    assert_eq!(fault, Err(ReadWorkerError::WorkerPanicked));
    assert!(
        first_pending && second_pending,
        "cancelled wait retains the later physical join"
    );
    assert_eq!(before_release, 0);
    assert_eq!(finished.load(Ordering::SeqCst), 1);
    assert_eq!(shutdown, Err(ReadWorkerError::WorkerPanicked));
    assert_eq!(
        workers.shutdown().await,
        Err(ReadWorkerError::WorkerPanicked)
    );
}
