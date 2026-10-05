//! Payload-free timing at accepted provider response boundaries.
use super::dispatch_policy::{request, worker_with_ready_frames};
use super::*;
use std::{
    fs::File,
    io::{self, ErrorKind, Write},
    os::fd::AsFd,
    sync::Mutex,
};
use tracing::instrument::WithSubscriber;

#[derive(Clone, Default)]
struct TimingLog(Arc<Mutex<Vec<u8>>>);
impl Write for TimingLog {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl TimingLog {
    async fn wait_for(&self, event: &str) {
        promptly(async {
            loop {
                if String::from_utf8_lossy(&self.0.lock().unwrap()).contains(event) {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
    }
}

#[tokio::test]
async fn first_response_timing_ignores_thoughts_empty_and_rejected_text_and_logs_once() {
    let (mut worker, _commands, _close, _events) = worker_with_ready_frames(&[], "").await;
    let clock = manual_clock(&mut worker.config);
    let mut execution = ExecutionController::new(ExecutionSessionId::new("context").unwrap());
    let (reply, _result) = oneshot::channel();
    worker
        .dispatch_execution(&mut execution, dispatched(request(), None), reply)
        .await
        .unwrap_or_else(|failure| panic!("{}", failure.error()));
    let captured = TimingLog::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let update = |session: &str, kind: &str, text: &str| {
            json!({
                "sessionId":session,"update":{"sessionUpdate":kind,"content":{"type":"text","text":text}}
            })
        };
        clock.advance(Duration::from_millis(20));
        worker
            .update(
                &mut execution,
                update("context", "agent_thought_chunk", "private thought"),
            )
            .unwrap();
        worker
            .update(&mut execution, update("context", "agent_message_chunk", ""))
            .unwrap();
        assert!(worker
            .update(
                &mut execution,
                update("wrong", "agent_message_chunk", "wrong text")
            )
            .is_err());
        assert!(worker
            .active
            .as_ref()
            .unwrap()
            .first_response_started
            .is_some());
        clock.advance(Duration::from_millis(30));
        worker
            .update(
                &mut execution,
                update("context", "agent_message_chunk", "private answer"),
            )
            .unwrap();
        clock.advance(Duration::from_millis(10));
        worker
            .update(
                &mut execution,
                update("context", "agent_message_chunk", "later answer"),
            )
            .unwrap();
    });
    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert_eq!(
        log.matches("agent_first_text_response_ms").count(),
        1,
        "{log}"
    );
    assert!(log.contains("elapsed_ms=50"), "{log}");
    assert!(log.contains("execution_id=\"next\""), "{log}");
    assert!(log.contains("session_id=\"context\""), "{log}");
    for private in [
        "private thought",
        "private answer",
        "wrong text",
        "later answer",
        "read file",
    ] {
        assert!(!log.contains(private), "{log}");
    }
    assert!(worker
        .active
        .as_ref()
        .unwrap()
        .first_response_started
        .is_none());
    worker
        .scope
        .cleanup(Duration::ZERO, Duration::from_secs(2))
        .await
        .unwrap();
}

#[tokio::test]
async fn startup_timing_logs_success_error_and_deadline_without_request_payloads() {
    for outcome in [
        "success",
        "error",
        "deadline",
        "write_error",
        "notifications",
        "preparation_error",
        "close",
    ] {
        let frames = match outcome {
            "success" => vec![json!({"jsonrpc":"2.0","id":1,"result":{}})],
            "notifications" => vec![
                json!({"jsonrpc":"2.0","method":"private notification","params":{"secret":"private provider payload"}}),
                json!({"jsonrpc":"2.0","method":"private notification"}),
                json!({"jsonrpc":"2.0","id":1,"result":{}}),
            ],
            "error" => vec![
                json!({"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":"fixture refusal"}}),
            ],
            _ => vec![],
        };
        let (mut worker, _commands, close, _events) = worker_with_ready_frames(&frames, "").await;
        if outcome == "deadline" {
            worker
                .scope
                .cleanup(Duration::ZERO, Duration::from_secs(2))
                .await
                .unwrap();
            let mut command = tokio::process::Command::new("/usr/bin/python3");
            command.args(["-c", "import sys,time;sys.stdin.readline();time.sleep(60)"]);
            let mut scope = ProcessScope::spawn(command).unwrap();
            worker.reader = Reader::new(
                scope.stdout.take().unwrap(),
                worker.config.max_incoming_frame_bytes,
            );
            worker.scope = scope;
        }
        if outcome == "write_error" {
            worker.scope.stdin.take();
        }
        if outcome == "preparation_error" {
            worker.config.max_frame_bytes = 1;
        }
        if outcome == "close" {
            close
                .send(Some(SessionCloseRequest::SessionHandlesDropped))
                .unwrap();
        }
        let clock = manual_clock(&mut worker.config);
        let deadline = clock.now() + Duration::from_millis(100);
        let captured = TimingLog::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let operation = worker
            .rpc(
                "initialize",
                json!({"secret":"private configuration"}),
                deadline,
                None,
            )
            .with_subscriber(subscriber);
        let result = if outcome == "deadline" {
            let (result, _) = promptly(async {
                tokio::join!(operation, async {
                    captured
                        .wait_for("agent startup request write finished")
                        .await;
                    clock.advance(Duration::from_millis(100));
                })
            })
            .await;
            result
        } else {
            promptly(operation).await
        };
        let success = matches!(outcome, "success" | "notifications");
        assert_eq!(result.is_ok(), success);
        let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        assert!(log.contains("agent startup phase started"), "{log}");
        assert!(log.contains("agent startup phase finished"), "{log}");
        assert!(log.contains("phase=\"initialize\""), "{log}");
        let expected = if success { "success" } else { "error" };
        assert!(log.contains(&format!("outcome=\"{expected}\"")), "{log}");
        if outcome == "deadline" {
            assert!(matches!(result, Err(AgentError::Deadline)));
            assert!(log.contains("elapsed_ms=100"), "{log}");
        }
        assert_eq!(
            log.matches("agent startup request write finished").count(),
            usize::from(outcome != "preparation_error"),
            "{log}"
        );
        assert_eq!(
            log.matches("agent startup first frame received").count(),
            usize::from(matches!(outcome, "success" | "error" | "notifications")),
            "{log}"
        );
        if outcome == "preparation_error" {
            assert!(matches!(result, Err(AgentError::InvalidInput(_))));
        } else {
            assert!(log.contains("request_id=1"), "{log}");
            let write = log
                .lines()
                .find(|line| line.contains("agent startup request write finished"))
                .unwrap();
            assert!(
                write.contains(if outcome == "write_error" {
                    "outcome=\"error\""
                } else {
                    "outcome=\"success\""
                }),
                "{log}"
            );
        }
        if matches!(outcome, "write_error" | "close") {
            assert!(matches!(result, Err(AgentError::Closed)));
        }
        // Provider refusal warnings are a separate existing target.
        let timing = log
            .lines()
            .filter(|line| line.contains("nessa_sdk::timing:"))
            .collect::<Vec<_>>()
            .join("\n");
        for private in [
            "private configuration",
            "private notification",
            "private provider payload",
            "fixture refusal",
        ] {
            assert!(!timing.contains(private), "{log}");
        }
        worker
            .scope
            .cleanup(Duration::ZERO, Duration::from_secs(2))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn startup_first_frame_duration_excludes_later_frames_and_uses_injected_clock() {
    let (mut worker, _commands, _close, _events) = worker_with_ready_frames(&[], "").await;
    worker
        .scope
        .cleanup(Duration::ZERO, Duration::from_secs(2))
        .await
        .unwrap();
    let mut command = tokio::process::Command::new("/usr/bin/python3");
    command.args([
        "-c",
        r#"import sys,json
print(json.dumps({'jsonrpc':'2.0','method':'ready'}),flush=True)
m=json.loads(sys.stdin.readline())
sys.stdin.readline()
print(json.dumps({'jsonrpc':'2.0','method':'notice'}),flush=True)
sys.stdin.readline()
print(json.dumps({'jsonrpc':'2.0','id':m['id'],'result':{}}),flush=True)
sys.stdin.read()
"#,
    ]);
    let mut scope = ProcessScope::spawn(command).unwrap();
    let mut gate = File::from(
        scope
            .stdin
            .as_ref()
            .unwrap()
            .as_fd()
            .try_clone_to_owned()
            .unwrap(),
    );
    worker.reader = Reader::new(
        scope.stdout.take().unwrap(),
        worker.config.max_incoming_frame_bytes,
    );
    assert_eq!(
        worker.reader.next().await.unwrap().method.as_deref(),
        Some("ready")
    );
    worker.scope = scope;
    let clock = manual_clock(&mut worker.config);
    let captured = TimingLog::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let operation = worker
        .rpc(
            "session/new",
            json!({}),
            clock.now() + Duration::from_secs(1),
            None,
        )
        .with_subscriber(subscriber);
    let (result, _) = promptly(async {
        tokio::join!(operation, async {
            captured
                .wait_for("agent startup request write finished")
                .await;
            clock.advance(Duration::from_millis(30));
            gate.write_all(b"\n").unwrap();
            captured
                .wait_for("agent startup first frame received")
                .await;
            clock.advance(Duration::from_millis(70));
            gate.write_all(b"\n").unwrap();
        })
    })
    .await;
    result.unwrap();
    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let first = log
        .lines()
        .find(|line| line.contains("agent startup first frame received"))
        .unwrap();
    assert!(first.contains("elapsed_ms=30"), "{log}");
    let finished = log
        .lines()
        .find(|line| line.contains("agent startup phase finished"))
        .unwrap();
    assert!(finished.contains("elapsed_ms=100"), "{log}");
    assert_eq!(
        log.matches("agent startup first frame received").count(),
        1,
        "{log}"
    );
    drop(gate);
    worker
        .scope
        .cleanup(Duration::ZERO, Duration::from_secs(2))
        .await
        .unwrap();
}

#[tokio::test]
async fn startup_write_duration_reports_blocked_pipe_until_deadline() {
    let (mut worker, _commands, _close, _events) = worker_with_ready_frames(&[], "").await;
    worker
        .scope
        .cleanup(Duration::ZERO, Duration::from_secs(2))
        .await
        .unwrap();
    let mut command = tokio::process::Command::new("/usr/bin/python3");
    command.args([
        "-c",
        "import sys,time;print('ready',flush=True);time.sleep(60)",
    ]);
    let mut scope = ProcessScope::spawn(command).unwrap();
    let mut ready = [0; 6];
    scope
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut ready)
        .await
        .unwrap();
    assert_eq!(&ready, b"ready\n");
    let mut filler = File::from(
        scope
            .stdin
            .as_ref()
            .unwrap()
            .as_fd()
            .try_clone_to_owned()
            .unwrap(),
    );
    loop {
        match filler.write(&[b'x'; 4096]) {
            Ok(size) => assert!(size > 0),
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) => panic!("pipe fill failed: {error}"),
        }
    }
    drop(filler);
    worker.scope = scope;
    let clock = manual_clock(&mut worker.config);
    let deadline = clock.now() + Duration::from_millis(100);
    let captured = TimingLog::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let result = ending_at(
        &clock,
        deadline,
        worker
            .rpc("initialize", json!({}), deadline, None)
            .with_subscriber(subscriber),
    )
    .await;
    assert!(matches!(result, Err(AgentError::Deadline)));
    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let write = log
        .lines()
        .find(|line| line.contains("agent startup request write finished"))
        .unwrap();
    assert!(write.contains("elapsed_ms=100"), "{log}");
    assert!(write.contains("outcome=\"error\""), "{log}");
    assert!(!log.contains("agent startup first frame received"), "{log}");
    worker
        .scope
        .cleanup(Duration::ZERO, Duration::from_secs(2))
        .await
        .unwrap();
}
