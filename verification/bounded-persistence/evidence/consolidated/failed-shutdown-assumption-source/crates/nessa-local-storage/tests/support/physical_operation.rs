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
    probe: Arc<Probe>,
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

#[derive(Default)]
pub(super) struct Probe {
    pub(super) submitted: AtomicUsize,
    pub(super) inputs: AtomicUsize,
    pub(super) inputs_at_release: AtomicUsize,
    pub(super) started: AtomicUsize,
    pub(super) cleaned: AtomicUsize,
    pub(super) before: Mutex<Option<Arc<Gate>>>,
    pub(super) after: Mutex<Option<Arc<Gate>>>,
    pub(super) input_drop: Mutex<Option<Arc<Gate>>>,
    pub(super) payload_before: AtomicBool,
    pub(super) payload_dropped: AtomicUsize,
    pub(super) panic_input_drop: AtomicBool,
    pub(super) panic_before: AtomicBool,
    pub(super) panic_after: AtomicBool,
}

impl Probe {
    // Observe the actual owned clone constructor, not a later independent counter.
    pub(super) fn clone_input<T>(&self, clone: impl FnOnce() -> T) -> T {
        self.inputs.fetch_add(1, Ordering::SeqCst);
        clone()
    }

    pub(super) fn wrap<F, R>(self: &Arc<Self>, operation: F) -> impl FnMut() -> R + Send
    where
        F: FnMut() -> R + Send,
        R: Send,
    {
        self.submitted.fetch_add(1, Ordering::SeqCst);
        let mut capture = ObservedCapture {
            operation,
            gate: self.input_drop.lock().unwrap().take(),
            fault: self.panic_input_drop.swap(false, Ordering::SeqCst),
            completion: Completion(self.clone()),
        };
        move || capture.call()
    }
}

// This fixture owns real captured F and observes its call/destruction. It contains
// no permit, executor, admission, join mapping, or panic-containment policy.
struct ObservedCapture<F> {
    operation: F,
    gate: Option<Arc<Gate>>,
    fault: bool,
    completion: Completion,
}
impl<F> ObservedCapture<F> {
    fn call<R>(&mut self) -> R
    where
        F: FnMut() -> R,
    {
        let probe = &self.completion.0;
        probe.started.fetch_add(1, Ordering::SeqCst);
        let gate = probe.before.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.enter();
        }
        if probe.payload_before.swap(false, Ordering::SeqCst) {
            std::panic::panic_any(Payload(probe.clone()));
        }
        assert!(
            !probe.panic_before.swap(false, Ordering::SeqCst),
            "physical pre-effect fault"
        );
        let output = (self.operation)();
        let gate = probe.after.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.enter();
        }
        assert!(
            !probe.panic_after.swap(false, Ordering::SeqCst),
            "physical post-effect fault"
        );
        output
    }
}
impl<F> Drop for ObservedCapture<F> {
    fn drop(&mut self) {
        if let Some(gate) = &self.gate {
            gate.enter();
        }
        if self.fault {
            std::panic::panic_any(Payload(self.completion.0.clone()));
        }
        // Automatic fields drop actual capture before the completion observer.
    }
}
struct Completion(Arc<Probe>);
impl Drop for Completion {
    fn drop(&mut self) {
        self.0.cleaned.fetch_add(1, Ordering::SeqCst);
    }
}
struct Payload(Arc<Probe>);
impl Drop for Payload {
    fn drop(&mut self) {
        self.0.payload_dropped.fetch_add(1, Ordering::SeqCst);
        panic!("faulting panic payload destructor");
    }
}
