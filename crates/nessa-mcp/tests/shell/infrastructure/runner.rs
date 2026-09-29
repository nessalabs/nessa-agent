use super::ShepherdRunner;
use crate::shell::application::*;
use crate::shell::domain::*;
use shepherd::ProcessId;
use shepherd::SupervisorBuilder;
use std::sync::Mutex;
use tokio::sync::watch;
struct RejectStarted {
    records: Mutex<Vec<Evidence>>,
}
impl Audit for RejectStarted {
    fn record<'a>(&'a self, _: &'a RunRequest, evidence: Evidence) -> Task<'a, Result<(), ()>> {
        self.records.lock().unwrap().push(evidence);
        Box::pin(async { Err(()) })
    }
}
#[tokio::test]
async fn started_audit_failure_still_reaps_and_preserves_the_process_identity() {
    let root = tempfile::tempdir().unwrap();
    let supervisor = SupervisorBuilder::new().build();
    let runner = ShepherdRunner::new(
        supervisor.clone(),
        vec![("PATH".into(), "/usr/bin:/bin".into())],
    );
    let audit = RejectStarted {
        records: Mutex::new(vec![]),
    };
    let request = RunRequest {
        id: "command".into(),
        invocation: ToolInvocation::provider("provider-connection", ToolRequestId::Unsigned(1))
            .unwrap(),
        command: ShellCommand::new("sleep 30".into(), 60).unwrap(),
        cwd: root.path().into(),
    };
    let (_send, receive) = watch::channel(None);
    let result = runner.run(&request, receive, &audit).await;
    assert_eq!(result.cause, RunCause::AuditFailed);
    assert!(result.cleanup_verified);
    assert!(result.audit_error.is_some());
    let Evidence::Started {
        scope_id,
        process_id,
        os_pid,
    } = audit.records.lock().unwrap()[0].clone()
    else {
        panic!("missing started identity");
    };
    assert_eq!(result.scope_id, Some(scope_id));
    assert_eq!(result.process_id, Some(process_id));
    assert_eq!(result.os_pid, os_pid);
    assert!(supervisor
        .wait(ProcessId::new(process_id))
        .await
        .unwrap()
        .outcome
        .is_verified());
    supervisor.shutdown().await.unwrap();
}
#[tokio::test]
async fn stopped_before_admission_never_creates_a_process_or_start_record() {
    let root = tempfile::tempdir().unwrap();
    let supervisor = SupervisorBuilder::new().build();
    let runner = ShepherdRunner::new(supervisor.clone(), vec![]);
    let audit = RejectStarted {
        records: Mutex::new(vec![]),
    };
    let request = RunRequest {
        id: "cancelled".into(),
        invocation: ToolInvocation::provider("provider-connection", ToolRequestId::Unsigned(2))
            .unwrap(),
        command: ShellCommand::new("false".into(), 1).unwrap(),
        cwd: root.path().into(),
    };
    let (_send, receive) = watch::channel(Some(StopCause::ClientCancelled));
    let result = runner.run(&request, receive, &audit).await;
    assert_eq!(result.cause, RunCause::Stopped(StopCause::ClientCancelled));
    assert!(result.cleanup_verified);
    assert!(result.process_id.is_none());
    assert!(audit.records.lock().unwrap().is_empty());
    supervisor.shutdown().await.unwrap();
}

fn snapshot(
    chunks: Vec<(shepherd::OutputStream, &str)>,
    closed: (bool, bool),
    dropped_bytes: u64,
    errors: &[&str],
) -> shepherd::OutputSnapshot {
    shepherd::OutputSnapshot {
        chunks: chunks
            .into_iter()
            .map(|(stream, text)| shepherd::OutputChunk {
                stream,
                bytes: text.as_bytes().to_vec(),
            })
            .collect(),
        tail: vec![],
        dropped_bytes,
        stdout_closed: closed.0,
        stderr_closed: closed.1,
        errors: errors.iter().map(|error| error.to_string()).collect(),
    }
}

#[tokio::test]
async fn output_arriving_after_exit_is_kept_until_both_streams_close() {
    use shepherd::OutputStream::{Stderr, Stdout};
    // The stderr reader lags the process exit: its bytes land two reads later.
    let mut reads = vec![
        snapshot(vec![(Stdout, "hello")], (true, false), 0, &[]),
        snapshot(vec![], (true, false), 0, &[]),
        snapshot(
            vec![(Stderr, "prob"), (Stderr, "lem")],
            (true, true),
            3,
            &["late"],
        ),
    ]
    .into_iter();
    let collected =
        super::collect(|| reads.next().unwrap(), std::time::Duration::from_secs(1)).await;
    assert_eq!(collected.stdout, b"hello");
    assert_eq!(collected.stderr, b"problem");
    assert_eq!(collected.dropped_bytes, 3);
    assert_eq!(collected.errors, vec!["late".to_string()]);
    assert!(reads.next().is_none(), "stopped reading once both closed");
}

#[tokio::test]
async fn a_stream_still_open_at_the_deadline_is_an_output_error() {
    use shepherd::OutputStream::Stdout;
    let mut first = true;
    let collected = super::collect(
        || {
            let chunks = if std::mem::take(&mut first) {
                vec![(Stdout, "partial")]
            } else {
                vec![]
            };
            snapshot(chunks, (true, false), 0, &[])
        },
        std::time::Duration::from_millis(20),
    )
    .await;
    assert_eq!(collected.stdout, b"partial");
    assert_eq!(
        collected.errors,
        vec!["stderr still open 20ms after the command ended".to_string()]
    );
}
