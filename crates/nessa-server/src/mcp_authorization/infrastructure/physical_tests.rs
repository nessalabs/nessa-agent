//! Physical-operation gates with an independent wall-clock watchdog.
use std::future::Future;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::Duration;

pub(super) struct Gate {
    entered: mpsc::Sender<()>,
    once: AtomicBool,
    release: Mutex<mpsc::Receiver<()>>,
}
impl Gate {
    pub(super) fn enter(&self) {
        if self.once.swap(true, Ordering::SeqCst) {
            return;
        }
        self.entered.send(()).unwrap();
        self.release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
    }
}

pub(super) fn heartbeat<F, Fut>(operation: F)
where
    F: FnOnce(Arc<Gate>) -> Fut,
    Fut: Future<Output = ()>,
{
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let gate = Arc::new(Gate {
        entered: entered_tx,
        once: AtomicBool::new(false),
        release: Mutex::new(release_rx),
    });
    let ticks = Arc::new(AtomicUsize::new(0));
    let observed = ticks.clone();
    let watchdog = std::thread::spawn(move || {
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let before = observed.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(120));
        let after = observed.load(Ordering::SeqCst);
        release_tx.send(()).unwrap();
        (before, after)
    });
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            let heartbeat = tokio::spawn(async move {
                loop {
                    ticks.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            });
            tokio::task::yield_now().await;
            operation(gate).await;
            heartbeat.abort();
            let _ = heartbeat.await;
        });
    let (before, after) = watchdog.join().unwrap();
    eprintln!("627 heartbeat: {before} -> {after} ticks during OS-held 120 ms physical operation");
    assert!(
        after > before,
        "current-thread heartbeat froze during physical I/O: {before} -> {after}"
    );
}

/// Releases physical work on an OS thread even if the sole runtime thread freezes.
pub(super) fn hold(
    probe: Arc<super::blocking::Probe>,
) -> (Arc<Gate>, std::thread::JoinHandle<(usize, usize, usize)>) {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let gate = Arc::new(Gate {
        entered: entered_tx,
        once: AtomicBool::new(false),
        release: Mutex::new(release_rx),
    });
    let watchdog = std::thread::spawn(move || {
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        probe
            .inputs_at_release
            .store(probe.inputs.load(Ordering::SeqCst), Ordering::SeqCst);
        let counts = (
            probe.submitted.load(Ordering::SeqCst),
            probe.started.load(Ordering::SeqCst),
            probe.cleaned.load(Ordering::SeqCst),
        );
        release_tx.send(()).unwrap();
        counts
    });
    (gate, watchdog)
}

pub(super) async fn reached(counter: &AtomicUsize, expected: usize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while counter.load(Ordering::SeqCst) < expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("physical worker reached test boundary");
}

struct ThreadWake(std::thread::Thread);
impl std::task::Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

pub(super) fn plain_thread<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = std::task::Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = std::task::Context::from_waker(&waker);
    let start = std::time::Instant::now();
    loop {
        if let std::task::Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "plain-thread executor watchdog expired"
        );
        std::thread::park_timeout(Duration::from_millis(10));
    }
}
