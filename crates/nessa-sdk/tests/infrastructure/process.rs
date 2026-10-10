use super::*;
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    task::{Context, Wake, Waker},
};
use tokio::sync::oneshot;

fn waiting_command() -> Command {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "while :; do sleep 1; done"]);
    command
}

fn exiting_command() -> Command {
    Command::new("/usr/bin/true")
}

struct GateReleaser {
    started: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}

impl DirectoryReleaser for GateReleaser {
    fn start(&self, path: PathBuf) -> JoinHandle<io::Result<()>> {
        let _ = self.started.lock().unwrap().take().unwrap().send(());
        let release = self.release.lock().unwrap().take().unwrap();
        tokio::spawn(async move {
            release.await.map_err(io::Error::other)?;
            std::fs::remove_dir_all(path)
        })
    }
}

enum ReleaseStep {
    Fail,
    Remove,
}

struct ScriptedReleaser {
    steps: Mutex<VecDeque<ReleaseStep>>,
}

impl DirectoryReleaser for ScriptedReleaser {
    fn start(&self, path: PathBuf) -> JoinHandle<io::Result<()>> {
        let step = self.steps.lock().unwrap().pop_front().unwrap();
        tokio::spawn(async move {
            match step {
                ReleaseStep::Fail => Err(io::Error::other("injected removal failure")),
                ReleaseStep::Remove => std::fs::remove_dir_all(path),
            }
        })
    }
}

#[tokio::test]
async fn private_directory_is_removed_only_after_process_cleanup_is_confirmed() {
    let (mut scope, directory) =
        ProcessScope::spawn_with_private_directory(|_| waiting_command()).unwrap();
    assert!(directory.is_dir());
    scope
        .cleanup(Duration::from_millis(50), Duration::from_secs(2))
        .await
        .unwrap();
    assert!(!directory.exists());
}

#[tokio::test]
async fn cancelled_directory_release_keeps_the_same_owned_task_for_retry() {
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let releaser = Arc::new(GateReleaser {
        started: Mutex::new(Some(started_tx)),
        release: Mutex::new(Some(release_rx)),
    });
    let (mut scope, directory) =
        ProcessScope::spawn_with_private_directory_using(|_| waiting_command(), releaser).unwrap();

    {
        let cleanup = scope.cleanup(Duration::from_millis(50), Duration::from_secs(2));
        tokio::pin!(cleanup);
        tokio::select! {
            _ = &mut cleanup => panic!("directory release completed before its barrier"),
            result = started_rx => result.unwrap(),
        }
    }
    assert!(scope.retained_directory.is_some());
    assert!(directory.is_dir());

    release_tx.send(()).unwrap();
    scope
        .cleanup(Duration::from_millis(50), Duration::from_secs(2))
        .await
        .unwrap();
    assert!(!directory.exists());
}

#[tokio::test]
async fn timed_out_directory_release_is_awaited_again_without_starting_another() {
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let releaser = Arc::new(GateReleaser {
        started: Mutex::new(Some(started_tx)),
        release: Mutex::new(Some(release_rx)),
    });
    let (mut scope, directory) =
        ProcessScope::spawn_with_private_directory_using(|_| exiting_command(), releaser).unwrap();

    assert_eq!(
        scope.cleanup(Duration::from_secs(2), Duration::ZERO).await,
        Err(AgentError::CleanupUncertain)
    );
    started_rx.await.unwrap();
    assert!(scope.retained_directory.is_some());
    release_tx.send(()).unwrap();
    scope
        .cleanup(Duration::from_millis(50), Duration::from_secs(2))
        .await
        .unwrap();
    assert!(!directory.exists());
}

#[tokio::test]
async fn failed_directory_release_is_retried_after_process_cleanup() {
    let releaser = Arc::new(ScriptedReleaser {
        steps: Mutex::new(VecDeque::from([ReleaseStep::Fail, ReleaseStep::Remove])),
    });
    let (mut scope, directory) =
        ProcessScope::spawn_with_private_directory_using(|_| waiting_command(), releaser).unwrap();

    assert_eq!(
        scope
            .cleanup(Duration::from_millis(50), Duration::from_secs(2))
            .await,
        Err(AgentError::CleanupUncertain)
    );
    assert!(scope.outcome.is_some(), "the process cleanup was confirmed");
    assert!(scope.retained_directory.is_some());
    assert!(
        directory.is_dir(),
        "the owed directory release was retained"
    );
    scope
        .cleanup(Duration::from_millis(50), Duration::from_secs(2))
        .await
        .unwrap();
    assert!(!directory.exists());
}

#[tokio::test]
async fn spawn_failure_retains_directory_until_failed_release_is_retried() {
    let releaser = Arc::new(ScriptedReleaser {
        steps: Mutex::new(VecDeque::from([ReleaseStep::Fail, ReleaseStep::Remove])),
    });
    let observed = Mutex::new(None);
    let failure = match ProcessScope::spawn_with_private_directory_using(
        |directory| {
            *observed.lock().unwrap() = Some(directory.to_path_buf());
            Command::new("/definitely/not/a/real/nessa-agent-provider")
        },
        releaser,
    ) {
        Ok(_) => panic!("invalid executable unexpectedly spawned"),
        Err(failure) => failure,
    };
    let (cause, recovery) = failure.into_parts();
    assert!(matches!(cause, AgentError::Transport(_)));
    let mut recovery = recovery.unwrap();
    let directory = observed.lock().unwrap().clone().unwrap();
    assert!(directory.is_dir());

    assert_eq!(
        recovery.release(Duration::from_secs(2)).await,
        Err(AgentError::CleanupUncertain)
    );
    assert!(directory.is_dir());
    recovery.release(Duration::from_secs(2)).await.unwrap();
    assert!(!directory.exists());
}

#[tokio::test]
async fn dropping_an_unconfirmed_process_retains_its_private_directory() {
    let (scope, directory) =
        ProcessScope::spawn_with_private_directory(|_| waiting_command()).unwrap();
    let group = scope.group();
    drop(scope);
    assert!(directory.is_dir());
    tokio::time::timeout(Duration::from_secs(2), async {
        while group_exists(group).unwrap_or(false) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let (mut replacement, replacement_directory) =
        ProcessScope::spawn_with_private_directory(|_| waiting_command()).unwrap();
    assert_ne!(replacement_directory, directory);
    assert!(
        directory.is_dir(),
        "a new launch did not reclaim an uncertain root"
    );
    replacement
        .cleanup(Duration::from_millis(50), Duration::from_secs(2))
        .await
        .unwrap();
    assert!(
        directory.is_dir(),
        "replacement cleanup did not delete the old root"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

/// Bites on macOS, where a group holding only an exited, unreaped leader
/// refuses signals with EPERM. On Linux the same calls see the zombie as a
/// member and deliver, so this passes there without reaching the EPERM arm.
/// Whether cleanup in the live race reports `forced: false` follows from the
/// `NotDelivered` asserted here; the race itself cannot be scheduled from a test.
#[tokio::test]
async fn signalling_an_exited_unreaped_group_is_not_a_cleanup_failure() {
    let mut scope = ProcessScope::spawn(exiting_command()).unwrap();
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // WNOWAIT observes the exit and leaves the leader unreaped: the state the
    // group is in when the adapter quits on stdin EOF just before the signal.
    let waited = unsafe {
        libc::waitid(
            libc::P_PID,
            scope.group() as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOWAIT,
        )
    };
    assert_eq!(waited, 0);
    if cfg!(target_os = "macos") {
        assert_eq!(
            signal_group(scope.group(), false),
            Ok(SignalDelivery::NotDelivered)
        );
        assert_eq!(
            signal_group(scope.group(), true),
            Ok(SignalDelivery::NotDelivered)
        );
    } else {
        assert!(signal_group(scope.group(), false).is_ok());
    }
    assert_eq!(
        scope.cleanup(Duration::ZERO, Duration::from_secs(2)).await,
        Ok(CloseOutcome { forced: false })
    );
}

/// A process group outside our permission (a root daemon's) is the real
/// refusal: the signal is not delivered and the probe never confirms it gone.
/// Skipped when running as root or when no such group is found.
#[test]
fn a_group_that_refuses_signals_is_never_confirmed_gone() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    // Group 1 is excluded: kill(-1, _) means every process, not group 1.
    let refused = (2..4096).find_map(|pid| {
        let group = unsafe { libc::getpgid(pid) };
        (group > 1
            && unsafe { libc::kill(-group, 0) } == -1
            && io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
        .then_some(group as u32)
    });
    let Some(group) = refused else {
        return;
    };
    assert_eq!(signal_group(group, false), Ok(SignalDelivery::NotDelivered));
    assert_eq!(group_exists(group), Err(AgentError::CleanupUncertain));
}

struct PanicWake {
    seen: Mutex<Option<oneshot::Sender<()>>>,
}
impl Wake for PanicWake {
    fn wake(self: Arc<Self>) {
        if let Some(seen) = self.seen.lock().unwrap().take() {
            let _ = seen.send(());
        }
        panic!("caller waker");
    }
}

/// The timer that `wait_scope` sleeps on, then the process reaper, wake this
/// wait. A panic there must leave a later process able to clean up.
#[tokio::test]
async fn panicking_cleanup_waiter_does_not_stop_later_cleanup() {
    let mut first = ProcessScope::spawn(waiting_command()).unwrap();
    let (seen, notified) = oneshot::channel();
    let waker = Waker::from(Arc::new(PanicWake {
        seen: Mutex::new(Some(seen)),
    }));
    let mut cleanup = Box::pin(first.cleanup(Duration::from_millis(50), Duration::from_secs(2)))
        as Pin<Box<dyn Future<Output = Result<CloseOutcome, AgentError>> + Send>>;
    assert!(
        cleanup
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending(),
        "cleanup must be parked on the timer before the process exits"
    );
    tokio::time::timeout(Duration::from_secs(3), notified)
        .await
        .expect("the timer woke the cleanup")
        .unwrap();
    let mut second = ProcessScope::spawn(waiting_command()).unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        second.cleanup(Duration::from_millis(50), Duration::from_secs(2)),
    )
    .await
    .expect("a later cleanup still finishes")
    .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), cleanup).await;
}

struct ScriptedGroupKill {
    steps: Mutex<VecDeque<Result<GroupSignal, AgentError>>>,
    calls: AtomicUsize,
}

impl ScriptedGroupKill {
    fn new(steps: Vec<Result<GroupSignal, AgentError>>) -> Self {
        Self {
            steps: Mutex::new(VecDeque::from(steps)),
            calls: AtomicUsize::new(0),
        }
    }

    fn kill(&self, group: u32, signal: i32) -> Result<GroupSignal, AgentError> {
        assert_eq!(group, 4242);
        assert_eq!(signal, libc::SIGTERM);
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.steps
            .lock()
            .expect("scripted group kill")
            .pop_front()
            .expect("kill was asked after the script ended")
    }
}

fn kill_script(script: &ScriptedGroupKill) -> Result<GroupSignal, AgentError> {
    let verdict = kill_group(4242, libc::SIGTERM, |group, signal| {
        script.kill(group, signal)
    })?;
    assert!(
        script.steps.lock().expect("scripted group kill").is_empty(),
        "the retry stopped before the scripted verdict"
    );
    Ok(verdict)
}

/// The injected signal is what is retried. One interrupt is asked again, a
/// refusal or an unreadable result is asked once, and sixteen interrupts are
/// the bound.
#[test]
fn an_interrupted_group_kill_retries_then_returns_the_verdict() {
    let interrupted_then_gone =
        ScriptedGroupKill::new(vec![Ok(GroupSignal::Interrupted), Ok(GroupSignal::Empty)]);
    assert_eq!(
        kill_script(&interrupted_then_gone).unwrap(),
        GroupSignal::Empty
    );
    assert_eq!(interrupted_then_gone.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        signal_verdict(GroupSignal::Empty),
        SignalDelivery::NotDelivered
    );

    let interrupted_then_reached =
        ScriptedGroupKill::new(vec![Ok(GroupSignal::Interrupted), Ok(GroupSignal::Reached)]);
    assert_eq!(
        kill_script(&interrupted_then_reached).unwrap(),
        GroupSignal::Reached
    );
    assert_eq!(interrupted_then_reached.calls.load(Ordering::SeqCst), 2);

    let refused = ScriptedGroupKill::new(vec![Ok(GroupSignal::Refused)]);
    assert_eq!(kill_script(&refused).unwrap(), GroupSignal::Refused);
    assert_eq!(refused.calls.load(Ordering::SeqCst), 1);

    let unreadable = ScriptedGroupKill::new(vec![Err(AgentError::CleanupUncertain)]);
    assert_eq!(
        kill_group(4242, libc::SIGTERM, |group, signal| {
            unreadable.kill(group, signal)
        }),
        Err(AgentError::CleanupUncertain)
    );
    assert_eq!(unreadable.calls.load(Ordering::SeqCst), 1);

    let exhausted = ScriptedGroupKill::new(vec![Ok(GroupSignal::Interrupted); 16]);
    assert_eq!(kill_script(&exhausted).unwrap(), GroupSignal::Interrupted);
    assert_eq!(exhausted.calls.load(Ordering::SeqCst), 16);
    assert_eq!(
        signal_verdict(GroupSignal::Interrupted),
        SignalDelivery::NotDelivered
    );
}

#[test]
fn an_interrupted_group_signal_is_not_a_cleanup_failure() {
    assert_eq!(
        classify_kill(-1, Some(libc::EINTR)).unwrap(),
        GroupSignal::Interrupted
    );
    assert_eq!(
        signal_verdict(GroupSignal::Interrupted),
        SignalDelivery::NotDelivered
    );
    assert_eq!(
        signal_verdict(GroupSignal::Empty),
        SignalDelivery::NotDelivered
    );
    assert_eq!(
        signal_verdict(GroupSignal::Refused),
        SignalDelivery::NotDelivered
    );
    assert_eq!(
        signal_verdict(GroupSignal::Reached),
        SignalDelivery::Delivered
    );
}

#[test]
fn a_group_probe_classifies_delivery_refusal_and_absence() {
    assert_eq!(classify_kill(0, None).unwrap(), GroupSignal::Reached);
    assert_eq!(
        classify_kill(-1, Some(libc::ESRCH)).unwrap(),
        GroupSignal::Empty
    );
    assert_eq!(
        classify_kill(-1, Some(libc::EPERM)).unwrap(),
        GroupSignal::Refused
    );
    assert_eq!(
        classify_kill(-1, Some(libc::EIO)),
        Err(AgentError::CleanupUncertain)
    );
    assert_eq!(classify_kill(-1, None), Err(AgentError::CleanupUncertain));
}

#[test]
fn an_interrupted_reap_keeps_the_same_cleanup_budget() {
    let interrupted = io::Error::from_raw_os_error(libc::EINTR);
    assert_eq!(
        watch_scope(Err(interrupted), Ok(false)),
        ScopeWatch::Pending,
        "one interrupted reap must not end the wait"
    );
    let refused = Err(AgentError::CleanupUncertain);
    assert_eq!(watch_scope(Ok(()), refused), ScopeWatch::Pending);
    assert_eq!(watch_scope(Ok(()), Ok(true)), ScopeWatch::Pending);
    assert_eq!(watch_scope(Ok(()), Ok(false)), ScopeWatch::Gone);
    let lost = io::Error::from_raw_os_error(libc::EIO);
    assert_eq!(watch_scope(Err(lost), Ok(false)), ScopeWatch::Lost);
}
