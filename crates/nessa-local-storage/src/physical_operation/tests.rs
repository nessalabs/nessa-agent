use super::*;
#[cfg(target_os = "linux")]
use std::sync::mpsc;
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
#[path = "../../tests/support/physical_operation.rs"]
mod physical_tests;
use physical_tests::{hold, Probe};

#[tokio::test]
async fn unused_admission_releases_slot() {
    let worker = Worker::new();
    let admission = worker.admit().await.unwrap();
    assert_eq!(worker.slot.available_permits(), 0);
    drop(admission);
    assert_eq!(worker.slot.available_permits(), 1);
    let output = worker.admit().await.unwrap().submit(|| 7).await.unwrap();
    assert_eq!(output, 7);
}

#[test]
fn no_runtime_admission_is_typed() {
    let worker = Worker::new();
    assert!(physical_tests::plain_thread(worker.admit()).is_err());
    assert_eq!(worker.slot.available_permits(), 1);
}

#[tokio::test]
async fn closed_slot_admission_is_typed() {
    let worker = Worker::new();
    worker.slot.close();
    assert!(worker.admit().await.is_err());
}

#[test]
fn queued_send_future_uses_origin() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let worker = Arc::new(Worker::new());
    let held = runtime.block_on(worker.admit()).unwrap();
    let later = worker.clone();
    let mut future = Box::pin(async move { later.admit().await?.submit(|| 9).await });
    runtime.block_on(std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    }));
    drop(held);
    assert_eq!(
        std::thread::spawn(move || physical_tests::plain_thread(future))
            .join()
            .unwrap()
            .unwrap(),
        9
    );
}

#[tokio::test]
async fn unpolled_result_drop_retains_physical_slot() {
    let worker = Worker::new();
    let probe = Arc::new(Probe::default());
    let (gate, watchdog) = hold(probe.clone());
    *probe.before.lock().unwrap() = Some(gate);
    let input = probe.clone_input(|| Arc::new(AtomicUsize::new(0)));
    let operation = probe.wrap(move || {
        input.fetch_add(1, Ordering::SeqCst);
    });
    let unpolled = worker.admit().await.unwrap().submit(operation);
    drop(unpolled);
    let admission = worker.admit().await.unwrap();
    assert_eq!(probe.cleaned.load(Ordering::SeqCst), 1);
    assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
    drop(admission);
}

#[test]
fn physical_call_keeps_current_thread_heartbeat_running() {
    physical_tests::heartbeat(|gate| async move {
        Worker::new()
            .admit()
            .await
            .unwrap()
            .submit(move || gate.enter())
            .await
            .unwrap();
    });
}

#[test]
fn queued_abort_capture_cleanup_keeps_slot() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    let worker = Worker::new();
    let probe = Arc::new(Probe::default());
    let (pool_gate, pool_watchdog) = hold(probe.clone());
    let (drop_gate, drop_watchdog) = hold(probe.clone());
    *probe.input_drop.lock().unwrap() = Some(drop_gate);
    runtime.block_on(async {
        let blocker = tokio::task::spawn_blocking(move || pool_gate.enter());
        let queued = worker.admit().await.unwrap().spawn(probe.wrap(|| ()));
        queued.abort();
        let next = worker.admit().await.unwrap();
        assert!(queued.await.unwrap_err().is_cancelled());
        assert_eq!(probe.cleaned.load(Ordering::SeqCst), 1);
        drop(next);
        blocker.await.unwrap();
    });
    assert_eq!(pool_watchdog.join().unwrap(), (1, 0, 0));
    assert_eq!(drop_watchdog.join().unwrap(), (1, 0, 0));
}

fn isolated(name: &str, body: impl FnOnce()) {
    const CHILD: &str = "NESSA_627_CENTRAL_FAULT_CASE";
    if std::env::var(CHILD).ok().as_deref() == Some(name) {
        body();
        return;
    }
    let test = format!("physical_operation::tests::{name}");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &test])
        .env(CHILD, name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "central fault subprocess failed");
            return;
        }
        if started.elapsed() > Duration::from_secs(5) {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("central fault subprocess watchdog expired");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn detached_call_fault_payload_is_not_dropped() {
    isolated("detached_call_fault_payload_is_not_dropped", || {
        let worker = Worker::new();
        let probe = Arc::new(Probe::default());
        let (gate, watchdog) = hold(probe.clone());
        *probe.before.lock().unwrap() = Some(gate);
        probe.payload_before.store(true, Ordering::SeqCst);
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                let unpolled = worker.admit().await.unwrap().submit(probe.wrap(|| ()));
                drop(unpolled);
                let next = worker.admit().await.unwrap();
                drop(next);
            });
        assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
        assert_eq!(probe.payload_dropped.load(Ordering::SeqCst), 0);
        assert_eq!(probe.cleaned.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn queued_abort_fault_payload_is_not_dropped() {
    isolated("queued_abort_fault_payload_is_not_dropped", || {
        let worker = Worker::new();
        let probe = Arc::new(Probe::default());
        let (pool_gate, pool_watchdog) = hold(probe.clone());
        let (drop_gate, drop_watchdog) = hold(probe.clone());
        *probe.input_drop.lock().unwrap() = Some(drop_gate);
        probe.panic_input_drop.store(true, Ordering::SeqCst);
        tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .build()
            .unwrap()
            .block_on(async {
                let blocker = tokio::task::spawn_blocking(move || pool_gate.enter());
                let queued = worker.admit().await.unwrap().spawn(probe.wrap(|| ()));
                queued.abort();
                drop(queued); // No result observer remains to dispose a panic payload.
                let next = worker.admit().await.unwrap();
                drop(next);
                blocker.await.unwrap();
            });
        assert_eq!(pool_watchdog.join().unwrap(), (1, 0, 0));
        assert_eq!(drop_watchdog.join().unwrap(), (1, 0, 0));
        assert_eq!(probe.payload_dropped.load(Ordering::SeqCst), 0);
        assert_eq!(probe.cleaned.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn call_and_capture_faults_are_separate_boundaries() {
    isolated("call_and_capture_faults_are_separate_boundaries", || {
        let worker = Worker::new();
        let probe = Arc::new(Probe::default());
        probe.payload_before.store(true, Ordering::SeqCst);
        probe.panic_input_drop.store(true, Ordering::SeqCst);
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                assert!(worker
                    .admit()
                    .await
                    .unwrap()
                    .submit(probe.wrap(|| ()))
                    .await
                    .is_err());
                assert_eq!(worker.slot.available_permits(), 1);
            });
        assert_eq!(probe.payload_dropped.load(Ordering::SeqCst), 0);
        assert_eq!(probe.cleaned.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn ready_capture_fault_downgrades_and_disposes_output_under_slot() {
    isolated(
        "ready_capture_fault_downgrades_and_disposes_output_under_slot",
        || {
            struct OutputPayload(Arc<AtomicUsize>);
            impl Drop for OutputPayload {
                fn drop(&mut self) {
                    self.0.fetch_add(1, Ordering::SeqCst);
                    panic!("output panic payload destructor");
                }
            }
            struct Output {
                worker: Arc<Worker>,
                disposed: Arc<AtomicUsize>,
                payload_dropped: Arc<AtomicUsize>,
            }
            impl Drop for Output {
                fn drop(&mut self) {
                    assert_eq!(self.worker.slot.available_permits(), 0);
                    self.disposed.fetch_add(1, Ordering::SeqCst);
                    std::panic::panic_any(OutputPayload(self.payload_dropped.clone()));
                }
            }
            let worker = Arc::new(Worker::new());
            let probe = Arc::new(Probe::default());
            let disposed = Arc::new(AtomicUsize::new(0));
            let payload_dropped = Arc::new(AtomicUsize::new(0));
            let (drop_gate, watchdog) = hold(probe.clone());
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(async {
                    for detached in [false, true] {
                        probe.panic_input_drop.store(true, Ordering::SeqCst);
                        if detached {
                            *probe.input_drop.lock().unwrap() = Some(drop_gate.clone());
                        }
                        let output_worker = worker.clone();
                        let output_disposed = disposed.clone();
                        let output_payload = payload_dropped.clone();
                        let operation = probe.wrap(move || Output {
                            worker: output_worker.clone(),
                            disposed: output_disposed.clone(),
                            payload_dropped: output_payload.clone(),
                        });
                        let result = worker.admit().await.unwrap().submit(operation);
                        if detached {
                            drop(result);
                            drop(worker.admit().await.unwrap());
                        } else {
                            assert!(result.await.is_err());
                            assert_eq!(worker.slot.available_permits(), 1);
                        }
                    }
                });
            assert_eq!(watchdog.join().unwrap(), (2, 2, 1));
            assert_eq!(disposed.load(Ordering::SeqCst), 2);
            assert_eq!(payload_dropped.load(Ordering::SeqCst), 0);
            assert_eq!(probe.payload_dropped.load(Ordering::SeqCst), 0);
        },
    );
}

#[tokio::test]
async fn cancellation_while_waiting_submits_no_capture() {
    let worker = Arc::new(Worker::new());
    let probe = Arc::new(Probe::default());
    let held = worker.admit().await.unwrap();
    let later = worker.clone();
    let observed = probe.clone();
    let caller = tokio::spawn(async move {
        let admission = later.admit().await.unwrap();
        let operation = observed.wrap(|| ());
        admission.submit(operation).await
    });
    tokio::task::yield_now().await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert_eq!(probe.submitted.load(Ordering::SeqCst), 0);
    drop(held);
    assert!(worker.admit().await.unwrap().submit(|| ()).await.is_ok());
}

#[tokio::test]
async fn independent_worker_instances_progress() {
    let first = Worker::new();
    let second = Worker::new();
    let held = first.admit().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(1), async {
        second.admit().await.unwrap().submit(|| 3).await.unwrap()
    })
    .await
    .expect("independent worker completes while first admission is held");
    assert_eq!(result, 3);
    drop(held);
}

#[tokio::test]
async fn ordinary_pre_and_post_call_faults_release_slot() {
    let worker = Worker::new();
    let probe = Arc::new(Probe::default());
    probe.panic_before.store(true, Ordering::SeqCst);
    assert!(worker
        .admit()
        .await
        .unwrap()
        .submit(probe.wrap(|| ()))
        .await
        .is_err());
    physical_tests::reached(&probe.cleaned, 1).await;
    probe.panic_after.store(true, Ordering::SeqCst);
    assert!(worker
        .admit()
        .await
        .unwrap()
        .submit(probe.wrap(|| ()))
        .await
        .is_err());
}

struct SubmissionPayload(Arc<AtomicUsize>);
impl Drop for SubmissionPayload {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("submission payload destructor fault");
    }
}

#[test]
fn submission_fault_payload_is_not_deferred_to_observer() {
    isolated(
        "submission_fault_payload_is_not_deferred_to_observer",
        || {
            for poll_observer in [false, true] {
                let dropped = Arc::new(AtomicUsize::new(0));
                let payload_counter = dropped.clone();
                // A name callback panic can poison Tokio's pool. Never run its Drop,
                // including when a reverted submit lets this test unwind.
                let runtime = std::mem::ManuallyDrop::new(
                    tokio::runtime::Builder::new_current_thread()
                        .thread_name_fn(move || {
                            std::panic::panic_any(SubmissionPayload(payload_counter.clone()))
                        })
                        .build()
                        .unwrap(),
                );
                let worker = Worker::new();
                let admission = runtime.block_on(worker.admit()).unwrap();
                let submitted = catch_unwind(AssertUnwindSafe(|| admission.submit(|| ())));
                let observer = match submitted {
                    Ok(observer) => observer,
                    Err(payload) => {
                        std::mem::forget(payload);
                        assert_eq!(
                            dropped.load(Ordering::SeqCst),
                            0,
                            "submission payload was destroyed"
                        );
                        panic!("submission escaped the typed boundary");
                    }
                };
                assert_eq!(dropped.load(Ordering::SeqCst), 0);
                assert_eq!(worker.slot.available_permits(), 0);
                if poll_observer {
                    assert!(runtime.block_on(observer).is_err());
                } else {
                    drop(observer);
                }
                assert_eq!(dropped.load(Ordering::SeqCst), 0);
                runtime.block_on(std::future::poll_fn(|cx| {
                    let next = worker.admit();
                    tokio::pin!(next);
                    assert!(next.poll(cx).is_pending());
                    std::task::Poll::Ready(())
                }));
                // This isolated fixture proves entry containment, not OS recovery,
                // runtime shutdown drainage, or progress of a poisoned Tokio pool.
            }
        },
    );
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "manual initial pthread EAGAIN injection; no shim/compiler CI dependency"]
fn os_refusal_retains_job_until_origin_kick() {
    struct Capture {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
        cleaned: Arc<AtomicUsize>,
    }
    impl Drop for Capture {
        fn drop(&mut self) {
            self.entered.send(()).unwrap();
            self.release.recv_timeout(Duration::from_secs(3)).unwrap();
            self.cleaned.fetch_add(1, Ordering::SeqCst);
        }
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    let worker = Worker::new();
    let effect = Arc::new(AtomicUsize::new(0));
    let cleaned = Arc::new(AtomicUsize::new(0));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let capture = Capture {
        entered: entered_tx,
        release: release_rx,
        cleaned: cleaned.clone(),
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("physical-effect");
    let output_path = path.clone();
    let operation_effect = effect.clone();
    let operation = move || {
        let _capture = &capture;
        std::fs::write(&output_path, b"landed once").unwrap();
        assert_eq!(operation_effect.fetch_add(1, Ordering::SeqCst), 0);
    };
    // No gate/watchdog threads precede submit: libtest thread #1, first blocking
    // worker #2. The external probe runner supplies the process watchdog.
    let mut observer = Some(runtime.block_on(worker.admit()).unwrap().submit(operation));
    let refused = std::env::var_os("NESSA_627_INITIAL_THREAD_REFUSAL").is_some();
    if refused {
        assert!(runtime.block_on(observer.take().unwrap()).is_err());
        assert_eq!(effect.load(Ordering::SeqCst), 0);
        assert_eq!(cleaned.load(Ordering::SeqCst), 0);
        assert!(!path.exists());
        runtime.block_on(std::future::poll_fn(|cx| {
            let next = worker.admit();
            tokio::pin!(next);
            assert!(next.poll(cx).is_pending());
            std::task::Poll::Ready(())
        }));
    }
    // The injected refusal is one-shot. This independent same-origin kick can
    // create its first worker; do not await it while the older capture is held.
    let kick = runtime.handle().spawn_blocking(|| ());
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(effect.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(&path).unwrap(), b"landed once");
    assert_eq!(cleaned.load(Ordering::SeqCst), 0);
    runtime.block_on(std::future::poll_fn(|cx| {
        let next = worker.admit();
        tokio::pin!(next);
        assert!(next.poll(cx).is_pending());
        std::task::Poll::Ready(())
    }));
    release_tx.send(()).unwrap();
    if let Some(observer) = observer {
        runtime.block_on(observer).unwrap();
    }
    runtime.block_on(kick).unwrap();
    let successor = runtime.block_on(worker.admit()).unwrap();
    assert_eq!(cleaned.load(Ordering::SeqCst), 1);
    assert_eq!(effect.load(Ordering::SeqCst), 1);
    drop(successor);
}
