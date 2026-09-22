use super::*;
use crate::application::agent_execution::agents::AgentFuture;
use crate::infrastructure::acp::{
    executions::event_queue::EventQueueBudget,
    tests::profile_substitution::{profile_setup, TestAcpProfile},
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A prompt as a session hands one to the worker: its images already read,
/// and the deadline of the phase that began before that read.
fn dispatched(input: ExecutionRequest, deadline: Option<Instant>) -> DispatchedPrompt {
    DispatchedPrompt {
        input,
        images: ImageBlocks::none(),
        deadline,
    }
}
/// The same for native steering, whose deadline is never absent.
fn steered(input: ExecutionRequest) -> DispatchedPrompt {
    dispatched(input, Some(Instant::now() + steering::RESPONSE_TIMEOUT))
}

struct UnexpectedAudit;
impl ExecutionAudit for UnexpectedAudit {
    fn record(&self, _: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        panic!("wire-only cancellation must not fabricate audit records")
    }
}

#[tokio::test]
async fn worker_initial_and_fallback_cancellation_share_grace_with_a_full_pipe() {
    for (grace, pending_permission) in [
        (Duration::from_millis(20), false),
        (Duration::from_millis(20), true),
        (Duration::from_secs(5), false),
        (Duration::from_secs(5), true),
    ] {
        let (_root, mut config, capabilities) = profile_setup();
        config.shutdown_grace = grace;
        let mut command = tokio::process::Command::new("/usr/bin/python3");
        command.args([
            "-c",
            "import sys,time;sys.stdout.write('!');sys.stdout.flush();time.sleep(60)",
        ]);
        let mut scope = ProcessScope::spawn(command).unwrap();
        let mut stdout = scope.stdout.take().unwrap();
        stdout.read_exact(&mut [0]).await.unwrap();
        // Establish actual pipe backpressure on a real clock before pausing Tokio.
        // The child never reads stdin, so this write cannot finish.
        assert!(timeout(
            Duration::from_millis(100),
            scope
                .stdin
                .as_mut()
                .unwrap()
                .write_all(&vec![b'x'; 2 * 1024 * 1024])
        )
        .await
        .is_err());
        let (_commands, commands) = mpsc::channel(1);
        let (_close, close_requested) = watch::channel(None);
        let (operation_capabilities, _) = watch::channel(ProviderOperationCapabilities::default());
        let (events, _events) = EventQueueBudget::new().channel(1);
        let mut worker = Worker {
            profile: TestAcpProfile {
                reject_startup: false,
                reject_session: false,
            },
            audit: Arc::new(UnexpectedAudit),
            cancellation_cause: None,
            reader: Reader::new(stdout, config.max_incoming_frame_bytes),
            scope,
            config,
            capabilities,
            commands,
            close_requested,
            events,
            sequence: 0,
            permission_sequence: Arc::new(AtomicU64::new(0)),
            active: None,
            steering: None,
            steering_supported: false,
            agent_accepts_images: false,
            operation_capabilities,
            permissions: HashMap::new(),
            declined: None,
            shutdown_deadline: None,
            configured: true,
            closing: true,
            deferred_outcome: None,
            provider_result: None,
            audit_failure: None,
            failure_cause: ObservationFailureCause::ExecutionFailed,
        };
        if pending_permission {
            worker
                .permissions
                .insert(PermissionId::new("review").unwrap(), RpcId::Number(7));
        }
        tokio::time::pause();
        let began = Instant::now();
        let deadline = worker.begin_shutdown_grace();
        let initial = worker.send_cancellation("fixture-context").await;
        let elapsed = Instant::now() - began;
        let expected = grace.min(Duration::from_secs(1));
        assert!(
            elapsed >= expected && elapsed <= expected + Duration::from_millis(1),
            "{elapsed:?}"
        );
        if grace < Duration::from_secs(1) {
            assert_eq!(initial, Ok(()));
            let expired = Instant::now();
            assert_eq!(worker.send_cancellation("fixture-context").await, Ok(()));
            assert_eq!(Instant::now(), expired, "fallback restarted expired grace");
        } else {
            assert_eq!(
                initial,
                Err(AgentError::Deadline),
                "write timeout is not grace exhaustion"
            );
            tokio::time::advance(deadline - Instant::now()).await;
            assert_eq!(worker.send_cancellation("fixture-context").await, Ok(()));
        }
        assert_eq!(worker.shutdown_deadline, Some(deadline));
        tokio::time::resume();
        // OS cleanup must never run under a paused/auto-advancing Tokio clock.
        worker
            .scope
            .cleanup(Duration::ZERO, Duration::from_secs(2))
            .await
            .unwrap();
    }
}

#[path = "worker/panics.rs"]
mod panics;

#[path = "worker/dispatch_policy.rs"]
mod dispatch_policy;

#[path = "worker/startup_responses.rs"]
mod startup_responses;

#[path = "worker/response_deadlines.rs"]
mod response_deadlines;
