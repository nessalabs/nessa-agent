use super::*;
use crate::application::agent_execution::agents::AgentFuture;
use crate::infrastructure::acp::{
    executions::event_queue::EventQueueBudget,
    tests::profile_substitution::{profile_setup, TestAcpProfile},
};
use crate::infrastructure::clock::manual::ManualClock;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::timeout,
};

/// A clock on `config` that moves only when the test moves it.
fn manual_clock(config: &mut AcpConfig) -> Arc<ManualClock> {
    let clock = Arc::new(ManualClock::default());
    config.clock = clock.clone();
    clock
}
/// `operation`, which must finish without waiting for a deadline the test has
/// not reached. The real-time bound only turns such a wait into a failure.
async fn promptly<T>(operation: impl Future<Output = T>) -> T {
    timeout(Duration::from_secs(10), operation)
        .await
        .expect("it waited for a deadline the clock never reached")
}

/// `operation`, with the clock moved to `deadline` once something waits for
/// it: it must wait for exactly that moment, and end there.
async fn ending_at<T>(
    clock: &ManualClock,
    deadline: ClockInstant,
    operation: impl Future<Output = T>,
) -> T {
    let (answer, _) = promptly(async {
        tokio::join!(operation, clock.run_out(|wait| wait.deadline == deadline))
    })
    .await;
    answer
}

/// A prompt as a session hands one to the worker: its images already read,
/// and the deadline of the phase that began before that read.
fn dispatched(input: ExecutionRequest, deadline: Option<ClockInstant>) -> DispatchedPrompt {
    DispatchedPrompt {
        input,
        images: ImageBlocks::none(),
        deadline,
    }
}
/// The same for native steering, whose deadline is never absent.
fn steered(clock: &dyn Clock, input: ExecutionRequest) -> DispatchedPrompt {
    dispatched(input, Some(clock.now() + steering::RESPONSE_TIMEOUT))
}

struct UnexpectedAudit;
impl ExecutionAudit for UnexpectedAudit {
    fn record(&self, _: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        panic!("wire-only cancellation must not fabricate audit records")
    }
}

fn decline_publication(
    capacity: usize,
) -> (
    DeclineNoticePublication,
    crate::infrastructure::acp::executions::event_queue::EventReceiver,
) {
    let (events, receiver) = EventQueueBudget::new().channel(capacity);
    let observation = ReviewDeclineObservation::selected(
        ReviewDeclineId::new("1").unwrap(),
        ReviewDecline::new(Some("Read"), ReviewDeclineReason::ToolNotReviewable),
    );
    (
        DeclineNoticePublication::new(events, ExecutionId::new("execution").unwrap(), observation),
        receiver,
    )
}

async fn decline_stage(
    receiver: &mut crate::infrastructure::acp::executions::event_queue::EventReceiver,
) -> ReviewDeclineStage {
    let ExecutionUpdate::ReviewDeclined(observation) = receiver.recv().await.unwrap().into_update()
    else {
        panic!("expected declined-review observation")
    };
    observation.stage()
}

#[tokio::test]
async fn dropping_decline_publication_finalizes_each_owned_write_state() {
    let (mut before_write, mut before_events) = decline_publication(2);
    before_write.publish_selected().unwrap();
    drop(before_write);
    assert_eq!(
        decline_stage(&mut before_events).await,
        ReviewDeclineStage::Selected
    );
    assert_eq!(
        decline_stage(&mut before_events).await,
        ReviewDeclineStage::WriteNotAttempted
    );

    let (mut during_write, mut during_events) = decline_publication(2);
    during_write.publish_selected().unwrap();
    during_write.begin_write();
    drop(during_write);
    assert_eq!(
        decline_stage(&mut during_events).await,
        ReviewDeclineStage::Selected
    );
    assert_eq!(
        decline_stage(&mut during_events).await,
        ReviewDeclineStage::WriteUnconfirmed
    );
}

#[tokio::test]
async fn a_full_notice_queue_retains_truthful_selection_and_reports_final_loss() {
    let (mut publication, mut events) = decline_publication(1);
    publication.publish_selected().unwrap();
    publication.begin_write();
    assert_eq!(
        publication.settle(ReviewDeclineStage::WriteConfirmed),
        Err(QueueError::Full)
    );
    drop(publication);
    assert_eq!(
        decline_stage(&mut events).await,
        ReviewDeclineStage::Selected
    );
    assert!(events.recv().await.is_none());
    assert_eq!(
        combine_decline_result(Err(AgentError::AuditFailure), Err(AgentError::Backpressure)),
        Err(AgentError::MultipleOperationFailures {
            first_error: Box::new(AgentError::AuditFailure),
            subsequent_error: Box::new(AgentError::Backpressure),
        })
    );
}

#[test]
fn event_publication_distinguishes_closed_from_full_and_preserves_an_earlier_cause() {
    let mut full_cause = None;
    assert_eq!(
        event_publication_result(&mut full_cause, Err(QueueError::Full)),
        Err(AgentError::Backpressure)
    );
    assert_eq!(full_cause, None, "an open full queue is not consumer loss");

    let mut closed_cause = None;
    assert_eq!(
        event_publication_result(&mut closed_cause, Err(QueueError::Closed)),
        Err(AgentError::Backpressure)
    );
    assert_eq!(
        closed_cause,
        Some((
            PermissionCancellationReason::event_consumer_dropped(),
            CancellationOrigin::Runtime,
        ))
    );

    let established = (
        PermissionCancellationReason::deadline_exceeded(),
        CancellationOrigin::Runtime,
    );
    let mut earlier_cause = Some(established.clone());
    assert_eq!(
        event_publication_result(&mut earlier_cause, Err(QueueError::Closed)),
        Err(AgentError::Backpressure)
    );
    assert_eq!(earlier_cause, Some(established));
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
        let clock = manual_clock(&mut config);
        let mut command = tokio::process::Command::new("/usr/bin/python3");
        command.args([
            "-c",
            "import sys,time;sys.stdout.write('!');sys.stdout.flush();time.sleep(60)",
        ]);
        let mut scope = ProcessScope::spawn(command).unwrap();
        let mut stdout = scope.stdout.take().unwrap();
        stdout.read_exact(&mut [0]).await.unwrap();
        // Establish actual pipe backpressure before the worker writes.
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
            question_sequence: Arc::new(AtomicU64::new(0)),
            active: None,
            steering: None,
            steering_supported: false,
            agent_accepts_images: false,
            operation_capabilities,
            permissions: HashMap::new(),
            startup_advisory_session: None,
            questions: HashMap::new(),
            declined: None,
            shutdown_deadline: None,
            configured: true,
            closing: true,
            deferred_outcome: None,
            provider_result: None,
            settlement_facts: SettlementFacts::new(),
            correlation_sequence: 0,
            failure_cause: ObservationFailureCause::ExecutionFailed,
        };
        if pending_permission {
            worker
                .permissions
                .insert(PermissionId::new("review").unwrap(), RpcId::Number(7));
        }
        let deadline = worker.begin_shutdown_grace();
        // The write into the full pipe is given the earlier of the grace and
        // the write allowance, and ends only when the clock reaches it.
        let expected = grace.min(Duration::from_secs(1));
        let (initial, _) = promptly(async {
            tokio::join!(
                worker.send_cancellation("fixture-context"),
                clock.run_out(|wait| wait.limit() == expected)
            )
        })
        .await;
        if grace < Duration::from_secs(1) {
            assert_eq!(initial, Ok(()));
            // Answered at once: the fallback does not start another grace.
            assert_eq!(
                promptly(worker.send_cancellation("fixture-context")).await,
                Ok(())
            );
        } else {
            assert_eq!(
                initial,
                Err(AgentError::Deadline),
                "write timeout is not grace exhaustion"
            );
            clock.advance_to(deadline);
            assert_eq!(
                promptly(worker.send_cancellation("fixture-context")).await,
                Ok(())
            );
        }
        assert_eq!(worker.shutdown_deadline, Some(deadline));
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

#[test]
fn questions_are_advertised_in_the_shape_acp_reads_and_only_where_answerable() {
    // ACP's schema reads `elicitation.form` as an object and discards anything
    // else as absent, so `true` told the agent nothing and it never asked.
    let advertised = initialize_params(true);
    assert_eq!(
        advertised["clientCapabilities"]["elicitation"],
        json!({"form":{}})
    );
    let silent = initialize_params(false);
    assert!(silent["clientCapabilities"].get("elicitation").is_none());
}
