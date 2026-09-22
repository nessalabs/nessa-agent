//! Live text is ephemeral; lifecycle boundaries persist the observed sequence.
use super::*;
use std::{
    future::{poll_fn, Future},
    task::Poll,
};

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
    fn open(&self, _: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("batch").unwrap(),
                    self.backend.clone(),
                    capabilities(),
                    Arc::new(AcceptingAudit),
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
}
impl StreamingTest {
    async fn new() -> Self {
        let storage = MemoryStorage::default();
        let (chunks, receiver) = mpsc::unbounded_channel();
        // The existing backend remains active until close. Its ordinary output
        // goes to an unrelated channel so these tests control stream boundaries.
        let (sender, _ignored) = mpsc::unbounded_channel();
        let (closing, _) = watch::channel(false);
        let backend = Arc::new(TestBackend {
            calls: Arc::new(ProviderCalls::default()),
            sender: Mutex::new(Some(sender)),
            closing,
            wait_for_close: true,
            outcome: Ok(ExecutionOutcome::Completed),
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
            storage.manager().await,
        )
        .await
        .unwrap();
        Self {
            agent,
            storage,
            backend,
            chunks,
            started,
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
async fn text_is_live_without_a_save_and_terminal_persists_the_sequence() {
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
    assert_eq!(test.storage.0.lock().unwrap().writes, writes);
    assert!(test.storage.snapshot().invocations[0].events.is_empty());
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
