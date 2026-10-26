//! Real processes: a helper left by one host is taken back by the next only
//! while it is the same process under the same key, and is ended otherwise.
use super::*;
use crate::timer::TokioTimer;

fn host(directory: &Path) -> Arc<KeptHost> {
    KeptHost::new(
        directory.to_owned(),
        "sleeper",
        shepherd::process_observer(),
        Arc::new(TokioTimer::new()),
    )
}

fn sleeper(directory: &Path, seconds: &str) -> Launch {
    Launch {
        program: "/bin/sleep".into(),
        args: vec![seconds.into()],
        environment: vec![],
        directory: directory.to_owned(),
    }
}

/// A halt the test sends.
fn halt() -> (oneshot::Sender<Halt>, PortFuture<'static, Halt>) {
    let (send, receive) = oneshot::channel();
    (send, Box::pin(async move { receive.await.unwrap_or(Halt::End) }))
}

fn alive(pid: u32) -> bool {
    !gone(pid)
}

async fn until_gone(pid: u32) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while alive(pid) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the process is gone");
}

/// Start a sleeper under `key` and leave it; its pid.
async fn left(host: &Arc<KeptHost>, directory: &Path, key: &str) -> u32 {
    let (started, running) = oneshot::channel();
    let (stop, halted) = halt();
    let run = tokio::spawn(host.run(key.into(), sleeper(directory, "60"), started, halted));
    running.await.unwrap();
    let pid = host.read().expect("recorded").pid;
    stop.send(Halt::Leave).unwrap();
    assert_eq!(run.await.unwrap(), HostedEnd::Left);
    assert!(alive(pid), "left running");
    pid
}

#[tokio::test]
async fn a_left_helper_is_taken_back_under_its_key_and_ended_when_asked() {
    let directory = tempfile::tempdir().unwrap();
    let pid = left(&host(directory.path()), directory.path(), "key").await;
    // The next run's host.
    let next = host(directory.path());
    let (started, running) = oneshot::channel();
    let (stop, halted) = halt();
    let (launch, run) = next
        .adopt("key".into(), started, halted)
        .await
        .expect("taken back");
    assert_eq!(launch, sleeper(directory.path(), "60"));
    running.await.unwrap();
    stop.send(Halt::End).unwrap();
    assert_eq!(run.await, HostedEnd::Stopped);
    until_gone(pid).await;
    assert!(next.read().is_none(), "forgotten once ended");
}

#[tokio::test]
async fn a_helper_left_under_another_key_is_ended_not_taken_back() {
    let directory = tempfile::tempdir().unwrap();
    let pid = left(&host(directory.path()), directory.path(), "old").await;
    let next = host(directory.path());
    let (started, _running) = oneshot::channel();
    let (_stop, halted) = halt();
    assert!(next.adopt("new".into(), started, halted).await.is_none());
    until_gone(pid).await;
    assert!(next.read().is_none());
}

#[tokio::test]
async fn a_record_whose_pid_is_now_another_process_is_forgotten_untouched() {
    let directory = tempfile::tempdir().unwrap();
    let first = host(directory.path());
    let pid = left(&first, directory.path(), "key").await;
    // The pid now stands for a process started at another time.
    let mut record = first.read().unwrap();
    record.identity.as_mut().unwrap().started_at -= 1;
    first.write(&record).unwrap();
    let next = host(directory.path());
    let (started, _running) = oneshot::channel();
    let (_stop, halted) = halt();
    assert!(next.adopt("key".into(), started, halted).await.is_none());
    assert!(next.read().is_none(), "forgotten");
    assert!(alive(pid), "a stranger is never signalled");
    signal_group(pid, libc::SIGKILL);
    until_gone(pid).await;
}

#[tokio::test]
async fn an_exit_is_seen_and_forgotten() {
    let directory = tempfile::tempdir().unwrap();
    let host = host(directory.path());
    let (started, running) = oneshot::channel();
    let (_stop, halted) = halt();
    let run = host.run("key".into(), sleeper(directory.path(), "0"), started, halted);
    let end = tokio::time::timeout(Duration::from_secs(15), run).await.unwrap();
    running.await.unwrap();
    assert_eq!(end, HostedEnd::Exited);
    assert!(host.read().is_none());
}

#[tokio::test]
async fn release_ends_the_left_helper() {
    let directory = tempfile::tempdir().unwrap();
    let pid = left(&host(directory.path()), directory.path(), "key").await;
    host(directory.path()).release().await;
    until_gone(pid).await;
    assert!(!directory.path().join(RECORD).exists());
}

#[tokio::test]
async fn a_left_helper_writes_its_output_to_its_log_privately() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let host = host(directory.path());
    let pid = left(&host, directory.path(), "key").await;
    let log = directory.path().join("sleeper.log");
    assert_eq!(std::fs::metadata(&log).unwrap().permissions().mode() & 0o077, 0);
    let record = std::fs::metadata(directory.path().join(RECORD)).unwrap();
    assert_eq!(record.permissions().mode() & 0o077, 0);
    host.release().await;
    until_gone(pid).await;
}
