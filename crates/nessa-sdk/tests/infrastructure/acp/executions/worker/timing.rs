//! Payload-free timing at accepted provider response boundaries.
use super::dispatch_policy::{request, worker_with_ready_frames};
use super::*;
use std::{
    io::{self, Write},
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
    for outcome in ["success", "error", "deadline"] {
        let frames = match outcome {
            "success" => vec![json!({"jsonrpc":"2.0","id":1,"result":{}})],
            "error" => vec![
                json!({"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":"fixture refusal"}}),
            ],
            _ => vec![],
        };
        let (mut worker, _commands, _close, _events) = worker_with_ready_frames(&frames, "").await;
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
            ending_at(&clock, deadline, operation).await
        } else {
            promptly(operation).await
        };
        assert_eq!(result.is_ok(), outcome == "success");
        let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        assert!(log.contains("agent startup phase started"), "{log}");
        assert!(log.contains("agent startup phase finished"), "{log}");
        assert!(log.contains("phase=\"initialize\""), "{log}");
        let expected = if outcome == "success" {
            "success"
        } else {
            "error"
        };
        assert!(log.contains(&format!("outcome=\"{expected}\"")), "{log}");
        if outcome == "deadline" {
            assert!(matches!(result, Err(AgentError::Deadline)));
            assert!(log.contains("elapsed_ms=100"), "{log}");
        }
        assert!(!log.contains("private configuration"), "{log}");
        worker
            .scope
            .cleanup(Duration::ZERO, Duration::from_secs(2))
            .await
            .unwrap();
    }
}
