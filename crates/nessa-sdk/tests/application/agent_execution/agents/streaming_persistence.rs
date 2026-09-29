//! Live text is provisional until the cadence or a lifecycle boundary saves it.
use super::*;
use nessa_sdk::application::agent_execution::sessions::{MessageCommitClock, MessageCommitSleep};
use std::{
    future::{poll_fn, Future},
    sync::Arc,
    task::Poll,
    time::Duration,
};

struct ManualMessageClock(watch::Sender<Duration>);
impl ManualMessageClock {
    fn new() -> Self {
        Self(watch::channel(Duration::ZERO).0)
    }
    fn advance(&self, duration: Duration) {
        let next = *self.0.borrow() + duration;
        self.0.send_replace(next);
    }
}
impl MessageCommitClock for ManualMessageClock {
    fn now(&self) -> Duration {
        *self.0.borrow()
    }
    fn sleep_until(&self, deadline: Duration) -> MessageCommitSleep {
        let mut changes = self.0.subscribe();
        Box::pin(async move {
            loop {
                if *changes.borrow() >= deadline {
                    return;
                }
                changes.changed().await.expect("manual clock remains owned");
            }
        })
    }
}

type Chunk = (ExecutionEvent, oneshot::Sender<()>);
struct TextStream {
    chunks: mpsc::UnboundedReceiver<Chunk>,
    consumed: Option<oneshot::Sender<()>>,
}
impl ExecutionEventStream for TextStream {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async move {
            if let Some(consumed) = self.consumed.take() {
                let _ = consumed.send(());
            }
            Ok(self.chunks.recv().await.map(|(event, consumed)| {
                self.consumed = Some(consumed);
                event
            }))
        })
    }
}
struct TextProvider {
    backend: Arc<TestBackend>,
    chunks: Mutex<Option<mpsc::UnboundedReceiver<Chunk>>>,
}
impl AgentProvider for TextProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("batch", "batch", "test").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("batch").unwrap(),
                    self.backend.clone(),
                    capabilities(),
                ),
                events: Box::new(TextStream {
                    chunks: self.chunks.lock().unwrap().take().unwrap(),
                    consumed: None,
                }),
            })
        })
    }
}
struct StreamingTest {
    agent: Agent,
    storage: MemoryStorage,
    backend: Arc<TestBackend>,
    chunks: mpsc::UnboundedSender<Chunk>,
    started: watch::Receiver<bool>,
    settled: watch::Receiver<bool>,
}
impl StreamingTest {
    async fn new() -> Self {
        Self::with_clock(Arc::new(
            nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
        ))
        .await
    }
    async fn with_clock(clock: Arc<dyn MessageCommitClock>) -> Self {
        Self::with_clock_and_outcome(clock, None).await
    }
    async fn with_clock_and_outcome(
        clock: Arc<dyn MessageCommitClock>,
        outcome_after_close: Option<Result<ExecutionOutcome, AgentError>>,
    ) -> Self {
        let storage = MemoryStorage::default();
        let (chunks, receiver) = mpsc::unbounded_channel();
        // The existing backend remains active until close. Its ordinary output
        // goes to an unrelated channel so these tests control stream boundaries.
        let (sender, _ignored) = mpsc::unbounded_channel();
        let (closing, _) = watch::channel(false);
        let (settled_sender, settled) = watch::channel(false);
        let backend = Arc::new(TestBackend {
            calls: Arc::new(ProviderCalls::default()),
            sender: Mutex::new(Some(sender)),
            closing,
            settled: Some(settled_sender),
            wait_for_close: true,
            outcome: Ok(ExecutionOutcome::Completed),
            outcome_after_close,
        });
        let (started_sender, started) = watch::channel(false);
        // The first backend message proves execute has started. Keep its output
        // receiver alive while the test controls the separate observed stream.
        tokio::spawn(async move {
            let mut ignored = _ignored;
            while ignored.recv().await.is_some() {
                started_sender.send_replace(true);
            }
        });
        let agent = attached_agent(
            Arc::new(TextProvider {
                backend: backend.clone(),
                chunks: Mutex::new(Some(receiver)),
            }),
            SessionManager::open(
                Some(SessionId::new("conversation").unwrap()),
                Arc::new(storage.clone()),
                clock,
            )
            .await
            .unwrap(),
        )
        .await
        .unwrap();
        Self {
            agent,
            storage,
            backend,
            chunks,
            started,
            settled,
        }
    }
    async fn wait_for_dispatch(&self) {
        let mut started = self.started.clone();
        started.wait_for(|started| *started).await.unwrap();
    }
    async fn start(&self) -> tokio::task::JoinHandle<Result<ExecutionOutcome, AgentError>> {
        let agent = self.agent.clone();
        let running = tokio::spawn(async move { agent.invoke(request("batch"), actor()).await });
        self.wait_for_dispatch().await;
        running
    }
    fn send(&self, update: ExecutionUpdate) -> oneshot::Receiver<()> {
        let (consumed, receiver) = oneshot::channel();
        self.chunks
            .send((
                ExecutionEvent::new(ExecutionId::new("batch").unwrap(), update),
                consumed,
            ))
            .unwrap();
        receiver
    }
    async fn text(&self, text: &str) {
        self.wait_for_dispatch().await;
        self.send(ExecutionUpdate::Message(MessageChunk::text(text)))
            .await
            .unwrap();
    }
    async fn finish(&self) {
        self.send(ExecutionUpdate::Finished(ExecutionOutcome::Cancelled))
            .await
            .unwrap();
        self.backend.closing.send_replace(true);
    }
}
async fn assert_pending<T>(future: impl Future<Output = T>) {
    let mut future = Box::pin(future);
    poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn text_is_live_until_the_cadence_and_terminal_persists_the_sequence() {
    let test = StreamingTest::new().await;
    let mut events = test.agent.subscribe();
    let running = test.start().await;
    test.text("first").await;
    let writes = test.storage.0.lock().unwrap().writes;
    let mut received = vec![events.next().await.unwrap().unwrap()];
    assert!(test.storage.snapshot().invocations[0].events.is_empty());
    for index in 1..150 {
        test.text(&index.to_string()).await;
        received.push(events.next().await.unwrap().unwrap());
    }
    test.text(&"x".repeat(64 * 1024)).await;
    received.push(events.next().await.unwrap().unwrap());
    assert!(test.storage.0.lock().unwrap().writes > writes);
    assert!(!test.storage.snapshot().invocations[0].events.is_empty());
    let (saving, release) = test.storage.pause_next_save();
    let terminal = test.send(ExecutionUpdate::Finished(ExecutionOutcome::Cancelled));
    saving.await.unwrap();
    assert_pending(events.next()).await;
    release.send(()).unwrap();
    terminal.await.unwrap();
    received.push(events.next().await.unwrap().unwrap());
    assert_eq!(test.storage.snapshot().invocations[0].events, received);
    test.backend.closing.send_replace(true);
    running.await.unwrap().unwrap();
}

#[tokio::test]
async fn terminal_save_failure_stops_provider_and_retains_failed_queue_receipt() {
    let test = StreamingTest::new().await;
    let receipt = test.agent.enqueue(request("batch"), actor()).await.unwrap();
    test.text("retained despite failed save").await;
    test.storage.fail_next();
    let _terminal = test.send(ExecutionUpdate::Finished(ExecutionOutcome::Cancelled));
    let result = receipt.wait().await;
    assert!(result.is_err());
    assert_eq!(
        test.agent
            .enqueue(request("batch"), actor())
            .await
            .unwrap()
            .wait()
            .await,
        result
    );
    assert_eq!(
        test.backend.calls.closes.lock().unwrap().as_slice(),
        &[SessionCloseRequest::ExecutionFailed]
    );
    let saved = test.storage.snapshot();
    assert_eq!(saved.invocations[0].events.len(), 2);
    assert!(saved.invocations[0].result.as_ref().unwrap().is_err());
}

#[tokio::test]
async fn dropping_invocation_waiter_keeps_text_and_settlement_owned() {
    let test = StreamingTest::new().await;
    let running = test.start().await;
    test.text("before caller loss").await;
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    test.text("after caller loss").await;
    test.finish().await;
    test.agent.close(actor()).await.unwrap();
    let saved = test.storage.snapshot();
    assert_eq!(saved.invocations[0].events.len(), 3);
    assert_eq!(
        saved.invocations[0].result,
        Some(Ok(ExecutionOutcome::Cancelled))
    );
}

#[tokio::test]
async fn eof_settlement_saves_text_without_a_terminal_event() {
    let test = StreamingTest::new().await;
    let running = test.start().await;
    test.text("text before eof").await;
    assert!(test.storage.snapshot().invocations[0].events.is_empty());
    drop(test.chunks);
    test.backend.closing.send_replace(true);
    assert_eq!(running.await.unwrap(), Ok(ExecutionOutcome::Cancelled));
    let saved = test.storage.snapshot();
    assert_eq!(saved.invocations[0].events.len(), 1);
    assert_eq!(
        saved.invocations[0].result,
        Some(Ok(ExecutionOutcome::Cancelled))
    );
}

#[tokio::test]
async fn provider_report_save_panic_retains_observed_text_during_recovery() {
    let test = StreamingTest::new().await;
    let running = test.start().await;
    test.text("text before save panic").await;
    test.storage.0.lock().unwrap().panic_provider_settlement = Some(false);
    test.backend.closing.send_replace(true);
    assert!(running.await.unwrap().is_err());
    let saved = test.storage.snapshot();
    assert_eq!(saved.invocations[0].events.len(), 1);
    assert!(saved.invocations[0].result.as_ref().unwrap().is_err());
}

#[tokio::test]
async fn tool_and_review_boundaries_persist_preceding_text() {
    let test = StreamingTest::new().await;
    let mut events = test.agent.subscribe();
    let running = test.start().await;
    test.text("before tool").await;
    let text = events.next().await.unwrap().unwrap();
    let writes = test.storage.0.lock().unwrap().writes;
    let (saving, release) = test.storage.pause_next_save();
    let consumed = test.send(ExecutionUpdate::Tool(ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        None,
        None,
        None,
        None,
        None,
    )));
    saving.await.unwrap();
    assert_pending(events.next()).await;
    release.send(()).unwrap();
    consumed.await.unwrap();
    let tool = events.next().await.unwrap().unwrap();
    assert_eq!(test.storage.0.lock().unwrap().writes, writes + 1);
    assert_eq!(
        test.storage.snapshot().invocations[0].events,
        vec![text, tool]
    );
    test.text("before review").await;
    let review = ExecutionUpdate::PermissionRequested {
        id: PermissionId::new("review").unwrap(),
        tool_id: ToolCallId::new("tool").unwrap(),
        observation: ToolObservation::default().with_update(ToolCallUpdate::new(
            ToolCallId::new("tool").unwrap(),
            None,
            None,
            None,
            None,
            None,
        )),
        input: ToolReviewInput {
            name: "tool".into(),
            arguments_json: "{}".into(),
        },
        options: PermissionOptions::new(
            vec![PermissionOption::new(
                PermissionOptionId::new("allow").unwrap(),
                "Allow",
                PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
            )
            .unwrap()],
            &PermissionOfferPolicy::once_only(),
        )
        .unwrap(),
    };
    let review_text = events.next().await.unwrap().unwrap();
    test.send(review).await.unwrap();
    let review = events.next().await.unwrap().unwrap();
    let saved = test.storage.snapshot();
    assert_eq!(&saved.invocations[0].events[2..], &[review_text, review]);
    assert_eq!(test.storage.0.lock().unwrap().writes, writes + 2);
    test.finish().await;
    running.await.unwrap().unwrap();
}

#[tokio::test]
async fn direct_invocation_retains_storage_failure_separately_from_provider_outcome() {
    let test = StreamingTest::new().await;
    let running = test.start().await;
    test.text("text before failed terminal save").await;
    test.storage.fail_next();
    let _terminal = test.send(ExecutionUpdate::Finished(ExecutionOutcome::Cancelled));
    let result = running.await.unwrap();
    assert!(result.is_err());
    let saved = test.storage.snapshot();
    assert_eq!(saved.invocations[0].result, Some(result));
    assert_eq!(
        saved.invocations[0]
            .provider_report
            .as_ref()
            .unwrap()
            .provider_result(),
        Some(&Ok(ExecutionOutcome::Cancelled))
    );
}

async fn wait_for_saved_text(storage: &MemoryStorage, count: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if storage.snapshot().invocations[0].events.len() >= count {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("deadline save did not finish");
}

#[tokio::test]
async fn lone_message_saves_at_fixed_manual_deadline_without_another_event() {
    let clock = Arc::new(ManualMessageClock::new());
    let test = StreamingTest::with_clock(clock.clone()).await;
    let running = test.start().await;
    test.text("only chunk").await;
    assert!(test.storage.snapshot().invocations[0].events.is_empty());
    clock.advance(Duration::from_millis(99));
    tokio::task::yield_now().await;
    assert!(test.storage.snapshot().invocations[0].events.is_empty());
    clock.advance(Duration::from_millis(1));
    wait_for_saved_text(&test.storage, 1).await;
    assert_eq!(
        test.storage.snapshot().invocations[0].events[0].update(),
        &ExecutionUpdate::Message(MessageChunk::text("only chunk"))
    );
    test.finish().await;
    running.await.unwrap().unwrap();
}

#[tokio::test]
async fn sustained_messages_do_not_slide_first_deadline() {
    let clock = Arc::new(ManualMessageClock::new());
    let test = StreamingTest::with_clock(clock.clone()).await;
    let running = test.start().await;
    test.text("first").await;
    clock.advance(Duration::from_millis(50));
    for _ in 0..8 {
        test.text("next").await;
    }
    clock.advance(Duration::from_millis(49));
    tokio::task::yield_now().await;
    assert!(test.storage.snapshot().invocations[0].events.is_empty());
    clock.advance(Duration::from_millis(1));
    wait_for_saved_text(&test.storage, 9).await;
    test.finish().await;
    running.await.unwrap().unwrap();
}

#[tokio::test]
async fn consequential_save_makes_old_timer_wake_a_noop() {
    let clock = Arc::new(ManualMessageClock::new());
    let test = StreamingTest::with_clock(clock.clone()).await;
    let running = test.start().await;
    test.text("before terminal").await;
    test.finish().await;
    running.await.unwrap().unwrap();
    let writes = test.storage.0.lock().unwrap().writes;
    clock.advance(Duration::from_millis(100));
    tokio::task::yield_now().await;
    assert_eq!(test.storage.0.lock().unwrap().writes, writes);
}

#[tokio::test]
async fn failed_deadline_save_stops_provider_with_typed_storage_failure() {
    let clock = Arc::new(ManualMessageClock::new());
    let test = StreamingTest::with_clock(clock.clone()).await;
    let running = test.start().await;
    test.text("pending").await;
    test.storage.fail_next();
    clock.advance(Duration::from_millis(100));
    let result = tokio::time::timeout(Duration::from_secs(2), running)
        .await
        .expect("failure supervised")
        .unwrap();
    assert!(matches!(
        result,
        Err(AgentError::StorageAfterExecution { .. })
    ));
    assert_eq!(
        test.backend.calls.closes.lock().unwrap().as_slice(),
        &[SessionCloseRequest::ExecutionFailed]
    );
}

#[tokio::test]
async fn message_count_or_bytes_flushes_before_deadline_once() {
    for large in [false, true] {
        let clock = Arc::new(ManualMessageClock::new());
        let test = StreamingTest::with_clock(clock.clone()).await;
        let running = test.start().await;
        let baseline = test.storage.0.lock().unwrap().writes;
        if large {
            test.text(&"x".repeat(16 * 1024)).await;
            wait_for_saved_text(&test.storage, 1).await;
        } else {
            for _ in 0..64 {
                test.text("x").await;
            }
            wait_for_saved_text(&test.storage, 64).await;
        }
        let writes = test.storage.0.lock().unwrap().writes;
        assert_eq!(writes, baseline + 1);
        clock.advance(Duration::from_millis(100));
        tokio::task::yield_now().await;
        assert_eq!(test.storage.0.lock().unwrap().writes, writes);
        test.finish().await;
        running.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn stalled_threshold_save_stops_provider_ingress_until_acknowledged() {
    let clock = Arc::new(ManualMessageClock::new());
    let test = StreamingTest::with_clock(clock).await;
    let running = test.start().await;
    for _ in 0..63 {
        test.text("x").await;
    }
    let (saving, release) = test.storage.pause_next_save();
    let mut threshold = test.send(ExecutionUpdate::Message(MessageChunk::text("x")));
    saving.await.unwrap();
    let mut following = test.send(ExecutionUpdate::Message(MessageChunk::text("next")));
    assert_pending(&mut threshold).await;
    assert_pending(&mut following).await;
    release.send(()).unwrap();
    threshold.await.unwrap();
    following.await.unwrap();
    assert_eq!(test.storage.snapshot().invocations[0].events.len(), 64);
    test.finish().await;
    running.await.unwrap().unwrap();
}

#[tokio::test]
async fn simultaneous_deadline_and_count_threshold_save_one_generation_once() {
    let clock = Arc::new(ManualMessageClock::new());
    let test = StreamingTest::with_clock(clock.clone()).await;
    let running = test.start().await;
    for _ in 0..63 {
        test.text("x").await;
    }
    let baseline = test.storage.0.lock().unwrap().writes;
    let threshold = test.send(ExecutionUpdate::Message(MessageChunk::text("x")));
    clock.advance(Duration::from_millis(100));
    threshold.await.unwrap();
    wait_for_saved_text(&test.storage, 63).await;
    assert_eq!(test.storage.0.lock().unwrap().writes, baseline + 1);
    test.finish().await;
    running.await.unwrap().unwrap();
    assert_eq!(test.storage.snapshot().invocations[0].events.len(), 65);
}

#[tokio::test]
async fn stopped_provider_cleanup_is_not_held_by_stalled_deadline_save() {
    let clock = Arc::new(ManualMessageClock::new());
    let test = StreamingTest::with_clock(clock.clone()).await;
    let running = test.start().await;
    test.text("pending").await;
    let (saving, release) = test.storage.pause_next_save();
    clock.advance(Duration::from_millis(100));
    saving.await.unwrap();
    let mut closing = test.backend.closing.subscribe();
    let agent = test.agent.clone();
    let close = tokio::spawn(async move { agent.close(actor()).await });
    let cleanup_started = tokio::time::timeout(Duration::from_secs(2), closing.changed()).await;
    let mut settled = test.settled.clone();
    let provider_settled = tokio::time::timeout(Duration::from_secs(2), settled.changed()).await;
    release.send(()).unwrap();
    test.finish().await;
    assert!(
        cleanup_started.is_ok(),
        "provider cleanup did not start while save was stalled"
    );
    assert_eq!(
        test.backend.calls.closes.lock().unwrap().as_slice(),
        &[SessionCloseRequest::Explicit(actor())]
    );
    assert!(
        provider_settled.is_ok(),
        "supervisor did not poll provider settlement while timer save stalled"
    );
    close.await.unwrap().unwrap();
    let _ = running.await.unwrap();
}

#[tokio::test]
async fn failed_timer_save_preserves_provider_reply_ready_during_the_write() {
    for provider_failed in [false, true] {
        let clock = Arc::new(ManualMessageClock::new());
        let provider_outcome =
            provider_failed.then(|| Err(AgentError::InvalidInput("provider failed".into())));
        let test = StreamingTest::with_clock_and_outcome(clock.clone(), provider_outcome).await;
        let running = test.start().await;
        test.text("pending").await;
        let (saving, release) = test.storage.pause_next_save();
        test.storage.fail_next();
        clock.advance(Duration::from_millis(100));
        saving.await.unwrap();
        test.backend.closing.send_replace(true);
        let mut settled = test.settled.clone();
        let provider_settled =
            tokio::time::timeout(Duration::from_secs(2), settled.changed()).await;
        release.send(()).unwrap();
        assert!(
            provider_settled.is_ok(),
            "provider reply was not polled during save"
        );
        drop(test.chunks);
        let result = tokio::time::timeout(Duration::from_secs(2), running)
            .await
            .unwrap()
            .unwrap();
        let Err(AgentError::StorageAfterExecution {
            execution_result, ..
        }) = result
        else {
            panic!("timer failure did not retain the provider outcome");
        };
        assert_eq!(execution_result.is_err(), provider_failed);
        assert_eq!(
            test.storage.snapshot().invocations[0]
                .provider_report
                .as_ref()
                .unwrap()
                .provider_result()
                .unwrap()
                .is_err(),
            provider_failed
        );
    }
}

#[tokio::test]
async fn failed_trailing_timer_save_keeps_an_already_recorded_provider_outcome() {
    for provider_failed in [false, true] {
        let clock = Arc::new(ManualMessageClock::new());
        let provider_outcome =
            provider_failed.then(|| Err(AgentError::InvalidInput("provider failed".into())));
        let test = StreamingTest::with_clock_and_outcome(clock.clone(), provider_outcome).await;
        let running = test.start().await;
        test.backend.closing.send_replace(true);
        let mut settled = test.settled.clone();
        settled.wait_for(|done| *done).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if test.storage.snapshot().invocations[0]
                    .provider_report
                    .is_some()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        test.text("trailing").await;
        let (saving, release) = test.storage.pause_next_save();
        test.storage.fail_next();
        clock.advance(Duration::from_millis(100));
        saving.await.unwrap();
        release.send(()).unwrap();
        drop(test.chunks);
        let result = tokio::time::timeout(Duration::from_secs(2), running)
            .await
            .unwrap()
            .unwrap();
        let Err(AgentError::StorageAfterExecution {
            execution_result, ..
        }) = result
        else {
            panic!("timer failure did not retain the independent execution result");
        };
        assert_eq!(execution_result.is_err(), provider_failed);
        if !provider_failed {
            assert_eq!(*execution_result, Ok(ExecutionOutcome::Cancelled));
        }
        assert_eq!(
            test.backend.calls.closes.lock().unwrap().as_slice(),
            &[SessionCloseRequest::ExecutionFailed]
        );
        assert_eq!(
            test.storage.snapshot().invocations[0]
                .provider_report
                .as_ref()
                .unwrap()
                .provider_result()
                .unwrap()
                .is_err(),
            provider_failed
        );
    }
}
