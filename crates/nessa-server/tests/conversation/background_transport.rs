use super::*;
use crate::conversation::domain::TaskCause;
use crate::conversation::infrastructure::ShepherdBackgroundTasks;
use std::sync::atomic::{AtomicBool, Ordering};

fn runner() -> ShepherdBackgroundTasks {
    ShepherdBackgroundTasks::new(
        shepherd::SupervisorBuilder::new().build(),
        vec![],
        Arc::new(shepherd::SystemClock),
    )
}

#[tokio::test]
async fn ssh_eof_cleans_the_shepherd_scope_before_reporting() {
    let directory = tempfile::tempdir().unwrap();
    let pidfile = directory.path().join("child.pid");
    let text = format!("sleep 60 & echo $! > {}; wait", pidfile.display());
    let command = BackgroundCommand::new(text, 60).unwrap();
    let (gateway, remote) = tokio::io::duplex(4096);
    let (input, output) = tokio::io::split(remote);
    let cwd = directory.path().to_str().unwrap().to_owned();
    let worker = tokio::spawn(async move { serve(&runner(), command, cwd, input, output).await });
    let pid: i32 = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(contents) = std::fs::read_to_string(&pidfile) {
                if let Ok(pid) = contents.trim().parse() {
                    break pid;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (reader, mut writer) = tokio::io::split(gateway);
    writer.shutdown().await.unwrap();
    let mut bytes = Vec::new();
    reader
        .take(MAX_RECORD_BYTES as u64)
        .read_to_end(&mut bytes)
        .await
        .unwrap();
    worker.await.unwrap().unwrap();
    let outcome = serde_json::from_slice::<Outcome>(&bytes)
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!(outcome.cause, TaskCause::Cancelled);
    assert!(outcome.cleanup_verified, "{outcome:?}");
    // A cleaned scope must leave no live descendant. A zombie may remain
    // until the container's PID 1 reaps it, so inspect its process state.
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
    assert!(stat.is_err() || stat.unwrap().split_whitespace().nth(2) == Some("Z"));
}

#[tokio::test]
async fn bounded_output_exit_and_timeout_cross_the_stream() {
    for (text, timeout, cause) in [
        (
            "printf hello; printf error >&2; exit 7",
            10,
            TaskCause::Exited,
        ),
        ("sleep 60", 1, TaskCause::TimedOut),
        ("yes x | head -c 100000", 10, TaskCause::Exited),
    ] {
        let (gateway, remote) = tokio::io::duplex(1024);
        let (input, output) = tokio::io::split(remote);
        let command = BackgroundCommand::new(text, timeout).unwrap();
        let worker =
            tokio::spawn(
                async move { serve(&runner(), command, "/tmp".into(), input, output).await },
            );
        let (reader, _kept_open) = tokio::io::split(gateway);
        let (_cancel, stop) = watch::channel(None);
        let outcome = receive(tokio::io::sink(), reader, stop).await;
        worker.await.unwrap().unwrap();
        assert_eq!(outcome.cause, cause, "{outcome:?}");
        assert!(outcome.cleanup_verified);
        assert!(outcome.stdout.len() + outcome.stderr.len() <= TaskOutcome::MAX_OUTPUT_BYTES * 3);
        if text.starts_with("printf") {
            assert_eq!(outcome.exit_code, Some(7));
            assert_eq!(outcome.stdout, "hello");
            assert_eq!(outcome.stderr, "error");
        }
        if text.starts_with("yes") {
            assert!(outcome.omitted_bytes > 0);
        }
    }
}

#[tokio::test]
async fn missing_malformed_oversized_and_inconsistent_outcomes_are_interrupted() {
    let mut invalid = TaskOutcome::interrupted();
    invalid.cleanup_verified = true;
    let mut oversized_error = TaskOutcome::not_started();
    oversized_error.error = Some("x".repeat(4097));
    for bytes in [
        Vec::new(),
        b"not json".to_vec(),
        vec![b'x'; MAX_RECORD_BYTES + 1],
        serde_json::to_vec(&Outcome::encode(&invalid)).unwrap(),
        serde_json::to_vec(&Outcome::encode(&oversized_error)).unwrap(),
    ] {
        let (_cancel, stop) = watch::channel(None);
        let outcome = receive(tokio::io::sink(), &bytes[..], stop).await;
        assert_eq!(outcome.cause, TaskCause::Interrupted);
        assert!(!outcome.cleanup_verified);
    }
}

struct CleanupGate {
    stopped: Arc<AtomicBool>,
    release: Arc<tokio::sync::Notify>,
}
impl BackgroundRunner for CleanupGate {
    fn run(
        &self,
        _: BackgroundCommand,
        _: BackgroundTarget,
        mut stop: watch::Receiver<Option<TaskStop>>,
    ) -> BackgroundRun<'_, TaskOutcome> {
        Box::pin(async move {
            stop.wait_for(Option::is_some).await.unwrap();
            self.stopped.store(true, Ordering::SeqCst);
            self.release.notified().await;
            TaskOutcome::interrupted()
        })
    }
    fn shutdown(&self) -> BackgroundRun<'_, Result<(), BackgroundTaskError>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn cancel_keeps_the_reader_pending_until_cleanup_answers() {
    let stopped = Arc::new(AtomicBool::new(false));
    let release = Arc::new(tokio::sync::Notify::new());
    let runner = CleanupGate {
        stopped: stopped.clone(),
        release: release.clone(),
    };
    let (gateway, remote) = tokio::io::duplex(4096);
    let (input, output) = tokio::io::split(remote);
    let worker = tokio::spawn(async move {
        serve(
            &runner,
            BackgroundCommand::new("true", 10).unwrap(),
            "/tmp".into(),
            input,
            output,
        )
        .await
    });
    let (reader, writer) = tokio::io::split(gateway);
    let (cancel, stop) = watch::channel(None);
    let receiver = tokio::spawn(receive(writer, reader, stop));
    cancel.send_replace(Some(TaskStop::GatewayStopped(
        TaskInitiator::new("test".into(), "test".into(), "stop".into()).unwrap(),
    )));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !stopped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!receiver.is_finished());
    release.notify_one();
    assert_eq!(receiver.await.unwrap().cause, TaskCause::Interrupted);
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_connection_already_closed_does_not_start_a_command() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("started");
    let command = BackgroundCommand::new(format!("touch {}", marker.display()), 10).unwrap();
    let mut bytes = Vec::new();
    serve(
        &runner(),
        command,
        "/tmp".into(),
        tokio::io::empty(),
        &mut bytes,
    )
    .await
    .unwrap();
    let outcome = serde_json::from_slice::<Outcome>(&bytes)
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!(outcome.cause, TaskCause::Cancelled);
    assert!(outcome.scope.is_none());
    assert!(outcome.cleanup_verified);
    assert!(!marker.exists());
}
