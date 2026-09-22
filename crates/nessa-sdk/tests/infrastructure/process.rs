use super::*;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
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
    assert_eq!(scope.retained_directory_path(), Some(directory.as_path()));
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
    assert_eq!(scope.retained_directory_path(), Some(directory.as_path()));
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
    assert_eq!(scope.retained_directory_path(), Some(directory.as_path()));
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
    let group = scope.group;
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
