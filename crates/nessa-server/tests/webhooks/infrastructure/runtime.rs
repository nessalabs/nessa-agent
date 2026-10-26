//! Supervisor ordering observed at the actual response wake, before task exit.
use super::*;
use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;
use tokio::sync::Notify;
struct Time;
impl Clock for Time {
    fn unix_milliseconds(&self) -> u64 {
        2_000
    }
}
fn runtime() -> (tempfile::TempDir, WebhookRuntime) {
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("private");
    nessa_local_storage::create_directory(&private).unwrap();
    let credentials = Arc::new(
        LocalCredentialStore::open(
            &private,
            "auth.json",
            Arc::new(CedarPolicyEvaluator::new().unwrap()),
        )
        .unwrap(),
    );
    let clock = Arc::new(Time);
    let store = Arc::new(
        SqlOutbox::open(
            &private.join("outbox.db"),
            private.join("audit"),
            clock.clone(),
        )
        .unwrap(),
    );
    let runtime =
        WebhookRuntime::new(Vec::new(), credentials.clone(), credentials, store, clock).unwrap();
    (root, runtime)
}
struct PanicObserver {
    runtime: WebhookRuntime,
    sealed_when_woken: Mutex<Option<bool>>,
    woken: Notify,
}
impl Wake for PanicObserver {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        let commands = self.runtime.owner.commands.lock().unwrap();
        *self.sealed_when_woken.lock().unwrap() = Some(commands.failed && !commands.accepting);
        self.woken.notify_one();
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_worker_panic_seals_before_the_response_observer_is_woken() {
    let (_root, runtime) = runtime();
    let observer = Arc::new(PanicObserver {
        runtime: runtime.clone(),
        sealed_when_woken: Mutex::new(None),
        woken: Notify::new(),
    });
    let waker = Waker::from(observer.clone());
    let (release, wait) = oneshot::channel::<()>();
    let mut command = Box::pin(runtime.owned::<()>(false, async move {
        wait.await.unwrap();
        panic!("synthetic owned worker panic")
    }));
    assert!(matches!(
        command.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Pending
    ));
    release.send(()).unwrap();
    let woken = tokio::time::timeout(Duration::from_secs(10), observer.woken.notified()).await;
    let result = tokio::time::timeout(Duration::from_secs(10), command).await;
    let drained = tokio::time::timeout(Duration::from_secs(10), runtime.shutdown()).await;
    assert!(woken.is_ok());
    assert_eq!(result.unwrap(), Err(CredentialAdminError::Unavailable));
    assert_eq!(drained.unwrap(), Err(CredentialAdminError::Unavailable));
    assert_eq!(
        *observer.sealed_when_woken.lock().unwrap(),
        Some(true),
        "sealing after task exit permits a racing observer to admit more work"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_capacity_precedes_spawn_and_control_capacity_remains_available() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (_root, runtime) = runtime();
    let effects = Arc::new(AtomicUsize::new(0));
    let mut observers = Vec::new();
    let mut releases = Vec::new();
    for _ in 0..MAX_OUTBOX_WORKERS {
        let (release, wait) = oneshot::channel();
        let effects = effects.clone();
        let mut admitted = Box::pin(runtime.owned(false, async move {
            wait.await.unwrap();
            effects.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        assert!(matches!(
            admitted
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        observers.push(admitted);
        releases.push(release);
    }
    let extra = effects.clone();
    let refused = runtime
        .owned(false, async move {
            extra.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await;
    let before = effects.load(Ordering::SeqCst);
    let control = runtime.owned(true, async { Ok(()) }).await;
    drop(observers);
    for release in releases {
        release.send(()).unwrap();
    }
    tokio::time::timeout(Duration::from_secs(10), runtime.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refused, Err(CredentialAdminError::Capacity));
    assert_eq!(before, 0, "a refused command never reaches its effect");
    assert_eq!(
        control,
        Ok(()),
        "ordinary saturation cannot consume the control lane"
    );
    assert_eq!(
        effects.load(Ordering::SeqCst),
        MAX_OUTBOX_WORKERS,
        "physical admitted work survives all observers"
    );
}
