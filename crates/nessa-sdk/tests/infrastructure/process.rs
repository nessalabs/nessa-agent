use super::*;

fn waiting_command() -> Command {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "while :; do sleep 1; done"]);
    command
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
async fn failed_directory_release_is_retried_after_process_cleanup() {
    let (mut scope, directory) =
        ProcessScope::spawn_with_private_directory(|_| waiting_command()).unwrap();
    scope.fail_next_directory_release();

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

#[test]
fn spawn_failure_releases_the_private_directory() {
    let observed = std::sync::Mutex::new(None);
    let result = ProcessScope::spawn_with_private_directory(|directory| {
        *observed.lock().unwrap() = Some(directory.to_path_buf());
        Command::new("/definitely/not/a/real/nessa-agent-provider")
    });

    assert!(matches!(result, Err(AgentError::Transport(_))));
    assert!(!observed.lock().unwrap().as_ref().unwrap().exists());
}
