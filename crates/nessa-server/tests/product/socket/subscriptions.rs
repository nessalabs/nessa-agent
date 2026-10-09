// Replay-to-live subscriptions over a real socket, a real record store and
// a real conversation service (`docs/design/record-subscriptions.md`). Each
// test is one row of that design's state table.

use super::*;
use crate::conversation::application::{
    ConversationDependencies, ConversationLimits, ConversationModeRequest,
    ConversationModeRequestState, ConversationRepository, ConversationService, SubmissionMode,
    SubmittedMessage,
};
use crate::conversation::infrastructure::NessaRecordWatches;
use crate::product::conversation::caller;
use conversation_support::{
    claude_erasers, mode_agents, AcceptingCreationAudit, AcceptingDeletionAudit,
    AcceptingModeAudit, ProviderFactory, RecordingFileLinkAudit, RecordingModeExecutionAudit,
    TestClock, DELETION_BUDGETS,
};
use nessa_protocol::product::generated::{
    MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS, SUBSCRIPTION_DELIVERY_TIMEOUT_MS,
};
use nessa_sdk::application::agent_execution::sessions::ChangeWatchError;
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
use std::task::Waker;
use tempfile::TempDir;
use tokio::task::JoinHandle;

/// How long a test waits for a frame it expects.
const EXPECTED: Duration = Duration::from_secs(10);
/// How long a test watches for a frame it expects not to come.
const QUIET: Duration = Duration::from_millis(750);

/// A socket whose writes the test can hold: while the gate is shut, a flush
/// does not finish, so the connection's one writer is stuck on that frame.
#[derive(Default)]
struct Gate {
    shut: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl Gate {
    fn shut(&self) {
        self.shut.store(true, Ordering::SeqCst);
    }

    fn open(&self) {
        self.shut.store(false, Ordering::SeqCst);
        if let Some(waker) = self.waker.lock().unwrap().take() {
            waker.wake();
        }
    }
}

struct GatedSocket {
    incoming: tokio::sync::mpsc::UnboundedReceiver<Result<Message, Error>>,
    outgoing: tokio::sync::mpsc::UnboundedSender<Message>,
    gate: Arc<Gate>,
    pending: Vec<Message>,
}

impl Stream for GatedSocket {
    type Item = Result<Message, Error>;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.incoming.poll_recv(cx)
    }
}

impl Sink<Message> for GatedSocket {
    type Error = std::io::Error;
    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn start_send(mut self: std::pin::Pin<&mut Self>, message: Message) -> Result<(), Self::Error> {
        self.pending.push(message);
        Ok(())
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        if self.gate.shut.load(Ordering::SeqCst) {
            *self.gate.waker.lock().unwrap() = Some(cx.waker().clone());
            if self.gate.shut.load(Ordering::SeqCst) {
                return std::task::Poll::Pending;
            }
        }
        for message in std::mem::take(&mut self.pending) {
            self.outgoing
                .send(message)
                .map_err(|_| std::io::Error::other("test peer closed"))?;
        }
        std::task::Poll::Ready(Ok(()))
    }
    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.poll_flush(cx)
    }
}

/// The client's end of one connection.
struct Client {
    input: tokio::sync::mpsc::UnboundedSender<Result<Message, Error>>,
    output: tokio::sync::mpsc::UnboundedReceiver<Message>,
    gate: Arc<Gate>,
    socket: JoinHandle<()>,
}

impl Client {
    fn send(&self, id: &str, method: &str, params: Value) {
        let frame = RequestFrame::new(id, method, &params).unwrap();
        self.input
            .send(Ok(Message::Text(
                serde_json::to_string(&frame).unwrap().into(),
            )))
            .unwrap();
    }

    async fn within(&mut self, wait: Duration) -> Option<Value> {
        match timeout(wait, self.output.recv()).await {
            Ok(Some(Message::Text(text))) => Some(serde_json::from_str(&text).unwrap()),
            Ok(Some(other)) => panic!("product text expected, got {other:?}"),
            Ok(None) => panic!("the socket closed"),
            Err(_) => None,
        }
    }

    async fn next(&mut self) -> Value {
        self.within(EXPECTED).await.expect("a frame in time")
    }

    /// The reply to request `id`; events before it are returned beside it.
    async fn reply(&mut self, id: &str) -> (Value, Vec<Value>) {
        let mut before = Vec::new();
        loop {
            let message = self.next().await;
            if message["type"] == "res" && message["id"] == id {
                return (message, before);
            }
            before.push(message);
        }
    }

    /// Subscribe to `conversation` and return the subscription's id.
    async fn subscribe(&mut self, request: &str, conversation: &ConversationId) -> String {
        self.send(
            request,
            "conversation.subscribe",
            json!({"conversationId": conversation.to_string()}),
        );
        let (reply, before) = self.reply(request).await;
        assert_eq!(reply["ok"], true, "{reply}");
        let id = reply["payload"]["subscriptionId"]
            .as_str()
            .unwrap()
            .to_owned();
        // Other subscriptions' frames may come first; none of this one's.
        assert!(
            before
                .iter()
                .all(|frame| frame["payload"]["subscriptionId"] != id.as_str()),
            "nothing of it before the reply: {before:?}"
        );
        id
    }

    /// Frames of subscription `id` until one satisfies `done`; every frame
    /// seen, the last satisfying it.
    async fn until(&mut self, id: &str, mut done: impl FnMut(&Value) -> bool) -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let message = self.next().await;
            if message["type"] != "event" || message["payload"]["subscriptionId"] != id {
                continue;
            }
            let finished = done(&message);
            seen.push(message);
            if finished {
                return seen;
            }
        }
    }

    /// Nothing of subscription `id` arrives for a while.
    async fn quiet(&mut self, id: &str) {
        let until = Instant::now() + QUIET;
        while let Ok(Some(message)) = timeout_at(until, self.output.recv()).await {
            let Message::Text(text) = message else {
                continue;
            };
            let message: Value = serde_json::from_str(&text).unwrap();
            assert_ne!(
                message["payload"]["subscriptionId"], id,
                "no frame expected: {message}"
            );
        }
    }

    async fn close(self) {
        drop(self.input);
        drop(self.output);
        self.gate.open();
        timeout(EXPECTED, self.socket).await.unwrap().unwrap();
    }
}

/// A connection to `state`'s gateway, authenticated as `session`.
fn connect(state: ProductRouteState, session: &AuthenticatedSession) -> Client {
    let (input, incoming) = tokio::sync::mpsc::unbounded_channel();
    let (outgoing, output) = tokio::sync::mpsc::unbounded_channel();
    let gate = Arc::new(Gate::default());
    let socket = tokio::spawn(run_authenticated(
        GatedSocket {
            incoming,
            outgoing,
            gate: gate.clone(),
            pending: Vec::new(),
        },
        state,
        session.clone(),
    ));
    Client {
        input,
        output,
        gate,
        socket,
    }
}

fn position(frame: &Value) -> u64 {
    frame["payload"]["cursor"]["position"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

fn view(frame: &Value) -> &Value {
    assert_eq!(frame["event"], "conversation.view", "{frame}");
    &frame["payload"]["view"]
}

/// Every message in `frame`'s view has finished.
fn settled(frame: &Value, messages: usize) -> bool {
    frame["event"] == "conversation.view"
        && view(frame)["transcriptState"] == "complete"
        && view(frame)["messages"]
            .as_array()
            .is_some_and(|all| {
                all.len() == messages && all.iter().all(|message| message["status"] == "completed")
            })
}

struct SubscriptionFixture {
    state: ProductRouteState,
    session: AuthenticatedSession,
    authority: Arc<Authority>,
    service: Arc<ConversationService>,
    provider: Arc<ProviderFactory>,
    storage: Arc<RecordStorage>,
    metadata: Arc<LocalConversationStore>,
    id: ConversationId,
    _directory: TempDir,
}

fn conversation_service(
    storage: &Arc<RecordStorage>,
    metadata: &Arc<LocalConversationStore>,
    provider: &Arc<ProviderFactory>,
) -> Arc<ConversationService> {
    let service = ConversationService::new(
        ConversationDependencies {
            // Agents that can change approval mode (row S20).
            agents: mode_agents(
                provider.clone(),
                Arc::new(RecordingModeExecutionAudit::default()),
            ),
            storage: storage.clone(),
            metadata: metadata.clone(),
            mode_audit: Arc::new(AcceptingModeAudit),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            summaries: metadata.clone(),
            listing: metadata.clone(),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions: claude_erasers(),
            deletion_budgets: DELETION_BUDGETS,
            message_commit_clock: Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
            ),
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    service.bind_commands(storage.clone());
    Arc::new(service)
}

fn grants(authority: &Authority, actions: &[&str]) {
    let mut snapshot = authority.snapshot.lock().unwrap();
    let organization = OrganizationId::new("organization").unwrap();
    snapshot.credential = Credential::new(
        CredentialId::new("credential").unwrap(),
        PrincipalId::new("principal").unwrap(),
        organization.clone(),
        AudienceId::new("gateway").unwrap(),
        100,
        200,
        actions
            .iter()
            .map(|action| {
                Grant::new(
                    Action::new(*action).unwrap(),
                    Resource::new(
                        organization.clone(),
                        ResourceId::new("gateway-resource").unwrap(),
                    ),
                )
            })
            .collect(),
    )
    .unwrap();
}

impl SubscriptionFixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let metadata =
            Arc::new(LocalConversationStore::open(&private.join("metadata.sqlite3")).unwrap());
        let storage = Arc::new(RecordStorage::new(directory.path().join("records")).unwrap());
        storage.initialize().await.unwrap();
        let provider = Arc::new(ProviderFactory::default());
        let service = conversation_service(&storage, &metadata, &provider);
        let (state, authority) = fixture(MembershipRole::Member);
        grants(&authority, &["conversation.write", "server.read"]);
        // A writer that may be held past a delivery timeout without the
        // socket giving up on the write first (rows S8, S10, S12).
        let settings = SessionSettings::new(
            Duration::from_secs(10),
            Duration::from_millis(SUBSCRIPTION_DELIVERY_TIMEOUT_MS * 3),
            Duration::from_secs(1),
        )
        .unwrap();
        let state = state
            .with_settings(settings)
            .with_conversations(service.clone())
            .with_change_watches(
                Arc::new(NessaRecordWatches::new(storage.clone())),
                metadata.clone(),
            )
            .with_agents_catalog(
                serde_json::from_value(json!({
                    "agents": [{
                        "agent": "claude",
                        "defaultModel": "test",
                        "models": [{
                            "modelId": "test",
                            "displayName": "Test model",
                            "maxContextWindowTokens": 100000,
                            "reasoning": false,
                            "imageInput": false,
                            "approvalModes": [{
                                "id": "ask",
                                "name": "Provider asks",
                                "description": "The provider requests approval where required."
                            }, {
                                "id": "auto",
                                "name": "Automatic",
                                "description": "The provider decides."
                            }]
                        }]
                    }]
                }))
                .unwrap(),
            );
        let session = authenticate(&state).await;
        let mut fixture = Self {
            state,
            session,
            authority,
            service,
            provider,
            storage,
            metadata,
            id: ConversationId::new(&Uuid::new_v4().to_string()).unwrap(),
            _directory: directory,
        };
        fixture.id = fixture.create().await;
        fixture
    }

    async fn call(&self, method: &str, params: Value) -> Value {
        let frame = RequestFrame::new("call", method, &params).unwrap();
        let OutgoingMessage::Response(response) =
            dispatch(&self.state, &self.session, frame).await
        else {
            panic!("a response")
        };
        serde_json::to_value(response).unwrap()
    }

    async fn create(&self) -> ConversationId {
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        let created = self
            .call(
                "conversation.create",
                json!({"conversationId": id.to_string(), "requestId": format!("create-{id}")}),
            )
            .await;
        assert_eq!(created["ok"], true, "{created}");
        id
    }

    /// Send a turn on the fixture's conversation; the fake agent answers it.
    async fn turn(&self, execution: &str) {
        let sent = self
            .call(
                "conversation.send",
                json!({
                    "conversationId": self.id.to_string(),
                    "requestId": format!("send-{execution}"),
                    "executionId": execution,
                    "text": format!("question {execution}"),
                    "attachments": [],
                    "files": [],
                }),
            )
            .await;
        assert_eq!(sent["ok"], true, "{sent}");
    }

    /// A one-shot read of the fixture's conversation, until it has settled
    /// with `messages` messages.
    async fn settled_read(&self, messages: usize) -> Value {
        timeout(EXPECTED, async {
            loop {
                let view = self
                    .service
                    .read(self.id.clone(), caller(&self.session, "read".into()))
                    .await
                    .unwrap();
                let view = serde_json::to_value(view).unwrap();
                let frame = json!({"event": "conversation.view", "payload": {"view": view}});
                if settled(&frame, messages) {
                    return frame["payload"]["view"].clone();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }

    /// A one-shot read until the last message is `execution`, completed:
    /// for a history longer than one bounded view.
    async fn turn_settled(&self, execution: &str) {
        // Generous: a long history makes each read slower.
        timeout(EXPECTED * 6, async {
            loop {
                let view = self
                    .service
                    .read(self.id.clone(), caller(&self.session, "read".into()))
                    .await
                    .unwrap();
                if view.messages.last().is_some_and(|last| {
                    last.execution_id == execution
                        && serde_json::to_value(last.status).unwrap() == "completed"
                }) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap()
    }

    fn connect(&self) -> Client {
        connect(self.state.clone(), &self.session)
    }

    /// Hold `client`'s writer: a health reply's flush does not finish until
    /// the gate opens, so nothing else is taken (rows S8, S10, S12).
    async fn stall(&self, client: &mut Client) {
        client.gate.shut();
        client.send("stall", "server.health", json!({}));
        // The writer is on the health reply once the request has been read;
        // give the socket a moment to take it.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    /// A conversation whose history the test writes straight to the store,
    /// `saves` complete saves long, with no agent ever opened on it.
    async fn seeded(&self, saves: usize) -> ConversationId {
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        let session = SessionId::new(id.to_string()).unwrap();
        let writer = self.storage.open(session.clone()).await.unwrap();
        let provider = ProviderIdentity::new("gateway-test", "test", "test").unwrap();
        let mut binding = writer.load().await.unwrap().binding().clone();
        let mut context = ProviderContext::Absent;
        for save in 0..saves {
            let change = if save == 0 {
                SessionChange::Opened {
                    id: session.clone(),
                    provider: provider.clone(),
                    context: ProviderContext::Absent,
                }
            } else {
                let after = ProviderContext::Recorded(
                    ExecutionSessionId::new(Uuid::new_v4().to_string()).unwrap(),
                );
                let before = std::mem::replace(&mut context, after.clone());
                SessionChange::ProviderContext { before, after }
            };
            binding = writer
                .save_changes(
                    binding,
                    SessionSnapshot {
                        id: session.clone(),
                        provider: provider.clone(),
                        provider_context: context.clone(),
                        invocations: Vec::new(),
                        queue_history: Vec::new(),
                    },
                    vec![SessionSaveUnit::new(vec![change]).unwrap()],
                )
                .await
                .unwrap()
                .next()
                .clone();
        }
        drop(writer);
        self.metadata
            .create(
                Conversation::new(
                    id.clone(),
                    OrganizationId::new("organization").unwrap(),
                    PrincipalId::new("principal").unwrap(),
                    "panel".into(),
                    format!("create-{id}"),
                    1,
                    AgentId::Claude,
                    ConversationModelId::new("test").unwrap(),
                    ConversationApprovalMode::Ask,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        id
    }

    /// One complete durable save to `id`'s records: the opening save first,
    /// then a provider context change, so each is a real commit.
    async fn commit(&self, id: &ConversationId) {
        let id = SessionId::new(id.to_string()).unwrap();
        let writer = self.storage.open(id.clone()).await.unwrap();
        let provider = ProviderIdentity::new("gateway-test", "test", "test").unwrap();
        let loaded = writer.load().await.unwrap();
        let (context, changes) = match loaded.snapshot() {
            None => (
                ProviderContext::Absent,
                vec![SessionChange::Opened {
                    id: id.clone(),
                    provider: provider.clone(),
                    context: ProviderContext::Absent,
                }],
            ),
            Some(snapshot) => {
                let after = ProviderContext::Recorded(
                    ExecutionSessionId::new(Uuid::new_v4().to_string()).unwrap(),
                );
                (
                    after.clone(),
                    vec![SessionChange::ProviderContext {
                        before: snapshot.provider_context.clone(),
                        after,
                    }],
                )
            }
        };
        writer
            .save_changes(
                loaded.binding().clone(),
                SessionSnapshot {
                    id,
                    provider,
                    provider_context: context,
                    invocations: Vec::new(),
                    queue_history: Vec::new(),
                },
                vec![SessionSaveUnit::new(changes).unwrap()],
            )
            .await
            .unwrap();
    }

    /// The fixture's conversation replayed in full by a second record store
    /// opened on the same files, folded by a fresh projection: nothing of the
    /// gateway's own fold or the first store's cached reads is shared. The
    /// first store is shut down for it, so this is a test's last step.
    async fn replayed(&self) -> Value {
        self.storage.shutdown().await.unwrap();
        let cold = RecordStorage::new(self._directory.path().join("records")).unwrap();
        cold.initialize().await.unwrap();
        let session = crate::conversation::application::conversation_session(&self.id);
        let mut projection = nessa_protocol::conversation::projection::Projection::new(
            self.id.to_string(),
            nessa_protocol::conversation::view::ConversationCapabilities::read_only(),
            None,
        );
        let view = timeout(EXPECTED, async {
            loop {
                let committed = cold.read_committed(session.clone()).await.unwrap().unwrap();
                let order = committed
                    .snapshot()
                    .map(SessionSnapshot::pending_order)
                    .transpose()
                    .unwrap()
                    .unwrap_or_default();
                projection.replace_committed(&committed, &order, None);
                projection.transcript_state(committed.state().into());
                let view = serde_json::to_value(projection.read()).unwrap();
                if view["transcriptState"] == "complete" {
                    return view;
                }
            }
        })
        .await
        .unwrap();
        cold.shutdown().await.unwrap();
        view
    }
}

/// Row S1.
#[tokio::test]
async fn subscribe_replies_after_the_first_read_then_sends_the_view() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    let id = client.subscribe("subscribe", &fixture.id).await;
    let first = client.next().await;
    assert_eq!(first["event"], "conversation.view");
    assert_eq!(first["payload"]["subscriptionId"], id);
    assert_eq!(view(&first)["conversationId"], fixture.id.to_string());
    assert!(first["payload"]["cursor"]["incarnation"].is_string());
    // The view is the one a one-shot read returns.
    let read = fixture
        .call(
            "conversation.read",
            json!({"conversationId": fixture.id.to_string()}),
        )
        .await;
    assert_eq!(read["payload"]["messages"], view(&first)["messages"]);
    client.quiet(&id).await;
    client.close().await;
}

/// Row S2: a refused first read refuses the subscription and frees its place.
#[tokio::test]
async fn a_refused_first_read_refuses_the_subscription() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    for attempt in 0..=MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS {
        let request = format!("unknown-{attempt}");
        client.send(
            &request,
            "conversation.subscribe",
            json!({"conversationId": Uuid::new_v4().to_string()}),
        );
        let (reply, before) = client.reply(&request).await;
        assert!(before.is_empty());
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["code"], "conversation_not_found");
    }
    // None of them kept a place.
    client.subscribe("known", &fixture.id).await;
    client.close().await;
}

/// Row S3.
#[tokio::test]
async fn a_cursor_ahead_of_history_is_refused() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    let id = client.subscribe("first", &fixture.id).await;
    let first = client.next().await;
    client.send("stop", "conversation.unsubscribe", json!({"subscriptionId": id}));
    client.reply("stop").await;
    let mut ahead = first["payload"]["cursor"].clone();
    ahead["position"] = json!((position(&first) + 1000).to_string());
    client.send(
        "ahead",
        "conversation.subscribe",
        json!({"conversationId": fixture.id.to_string(), "after": ahead}),
    );
    let (reply, _) = client.reply("ahead").await;
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"]["code"], "cursor_ahead");
    client.close().await;
}

/// Row S4.
#[tokio::test]
async fn a_cursor_from_another_incarnation_gets_the_current_view() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    client.send(
        "foreign",
        "conversation.subscribe",
        json!({
            "conversationId": fixture.id.to_string(),
            "after": {"incarnation": "another-store", "position": "99999"},
        }),
    );
    let (reply, _) = client.reply("foreign").await;
    assert_eq!(reply["ok"], true, "{reply}");
    let frame = client.next().await;
    assert_eq!(view(&frame)["conversationId"], fixture.id.to_string());
    assert_ne!(frame["payload"]["cursor"]["incarnation"], "another-store");
    client.close().await;
}

/// Row S5: a history longer than one bounded read is replayed read by read,
/// and a client that already holds part of it gets nothing behind that.
#[tokio::test]
async fn a_long_history_replays_in_bounded_frames_and_never_behind_the_cursor() {
    let fixture = SubscriptionFixture::new().await;
    let long = fixture.seeded(300).await;
    let mut client = fixture.connect();
    let id = client.subscribe("replay", &long).await;
    let frames = client
        .until(&id, |frame| view(frame)["transcriptState"] == "complete")
        .await;
    assert!(frames.len() > 1, "more than one bounded read: {}", frames.len());
    for pair in frames.windows(2) {
        assert!(position(&pair[0]) < position(&pair[1]), "only forward");
        assert_eq!(view(&pair[0])["transcriptState"], "stale");
    }
    let last = frames.last().unwrap().clone();
    client.close().await;

    // A client that already has `last` is sent nothing behind it.
    let mut client = fixture.connect();
    client.send(
        "resume",
        "conversation.subscribe",
        json!({"conversationId": long.to_string(), "after": last["payload"]["cursor"]}),
    );
    let (reply, _) = client.reply("resume").await;
    assert_eq!(reply["ok"], true, "{reply}");
    let resumed = reply["payload"]["subscriptionId"].as_str().unwrap().to_owned();
    let frame = client.next().await;
    assert_eq!(frame["payload"]["subscriptionId"], resumed);
    assert_eq!(position(&frame), position(&last), "nothing behind it");
    assert_eq!(view(&frame)["transcriptState"], "complete");
    client.quiet(&resumed).await;
    client.close().await;
}

/// Row S6.
#[tokio::test]
async fn a_commit_after_subscribe_is_delivered() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    let id = client.subscribe("subscribe", &fixture.id).await;
    let first = client.next().await;
    fixture.turn("turn-1").await;
    let frames = client.until(&id, |frame| settled(frame, 1)).await;
    assert!(frames.iter().all(|frame| position(frame) >= position(&first)));
    client.close().await;
}

/// Row S7: subscriptions taken at every point of a run of turns each reach
/// the last commit, with nothing after it to wake them.
#[tokio::test]
async fn replay_then_live_misses_no_commit_under_concurrent_writes() {
    let fixture = Arc::new(SubscriptionFixture::new().await);
    const TURNS: usize = 6;
    let writer = {
        let fixture = fixture.clone();
        tokio::spawn(async move {
            for turn in 1..=TURNS {
                fixture.turn(&format!("turn-{turn}")).await;
                fixture.settled_read(turn).await;
            }
        })
    };
    let mut clients = Vec::new();
    for round in 0..TURNS {
        let mut client = fixture.connect();
        let id = client.subscribe(&format!("subscribe-{round}"), &fixture.id).await;
        clients.push((client, id));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    writer.await.unwrap();
    let expected = fixture.settled_read(TURNS).await;
    for (mut client, id) in clients {
        let frames = client.until(&id, |frame| settled(frame, TURNS)).await;
        for pair in frames.windows(2) {
            assert!(position(&pair[0]) <= position(&pair[1]), "only forward");
        }
        assert_eq!(
            view(frames.last().unwrap())["messages"],
            expected["messages"]
        );
        client.close().await;
    }
}

/// Row S8.
#[tokio::test]
async fn commits_while_a_frame_waits_collapse_into_one_frame() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    let id = client.subscribe("subscribe", &fixture.id).await;
    client.next().await;
    fixture.stall(&mut client).await;
    for turn in 1..=3 {
        fixture.turn(&format!("turn-{turn}")).await;
        fixture.settled_read(turn).await;
    }
    client.gate.open();
    let mut frames = Vec::new();
    while let Some(message) = client.within(QUIET).await {
        if message["payload"]["subscriptionId"] == id.as_str() {
            frames.push(message);
        }
    }
    // The frame offered at the first commit, then one with all of them.
    assert!(
        (1..=2).contains(&frames.len()),
        "many commits, at most two frames: {}",
        frames.len()
    );
    assert!(settled(frames.last().unwrap(), 3));
    client.close().await;
}

/// Row S9: a commit that changes no row of the list wakes the list
/// subscription, and it sends nothing.
#[tokio::test]
async fn a_wake_that_changes_nothing_sends_nothing() {
    let fixture = SubscriptionFixture::new().await;
    let quiet = fixture.seeded(1).await;
    let mut client = fixture.connect();
    client.send("list", "conversation.subscribeList", json!({}));
    let (reply, _) = client.reply("list").await;
    let id = reply["payload"]["subscriptionId"].as_str().unwrap().to_owned();
    client.next().await;
    fixture.commit(&quiet).await;
    client.quiet(&id).await;
    // Still live: a change the list shows is sent.
    fixture.turn("turn-1").await;
    client
        .until(&id, |frame| {
            frame["payload"]["list"]["conversations"]
                .as_array()
                .is_some_and(|rows| rows.len() == 1)
        })
        .await;
    client.close().await;
}

/// Row S10.
#[tokio::test]
async fn a_frame_not_taken_in_time_ends_the_subscription_as_lagging() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    let id = client.subscribe("subscribe", &fixture.id).await;
    let first = client.next().await;
    fixture.stall(&mut client).await;
    fixture.turn("turn-1").await;
    tokio::time::sleep(Duration::from_millis(SUBSCRIPTION_DELIVERY_TIMEOUT_MS + 1000)).await;
    client.gate.open();
    let (_, before) = client.reply("stall").await;
    assert!(before.is_empty());
    let ended = client.next().await;
    assert_eq!(ended["event"], "conversation.subscriptionEnded", "{ended}");
    assert_eq!(ended["payload"]["subscriptionId"], id);
    assert_eq!(ended["payload"]["reason"], "lagging");
    assert_eq!(ended["payload"]["lastDelivered"], first["payload"]["cursor"]);
    // The socket stays: it still answers.
    client.send("health", "server.health", json!({}));
    let (health, _) = client.reply("health").await;
    assert_eq!(health["ok"], true);
    client.quiet(&id).await;
    client.close().await;
}

/// Row S11.
#[tokio::test]
async fn a_lagging_subscriber_resumes_from_its_cursor() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    client.subscribe("subscribe", &fixture.id).await;
    client.next().await;
    fixture.stall(&mut client).await;
    fixture.turn("turn-1").await;
    tokio::time::sleep(Duration::from_millis(SUBSCRIPTION_DELIVERY_TIMEOUT_MS + 1000)).await;
    client.gate.open();
    client.reply("stall").await;
    let ended = client.next().await;
    assert_eq!(ended["payload"]["reason"], "lagging");
    let after = ended["payload"]["lastDelivered"].clone();
    client.send(
        "resume",
        "conversation.subscribe",
        json!({"conversationId": fixture.id.to_string(), "after": after}),
    );
    let (reply, _) = client.reply("resume").await;
    assert_eq!(reply["ok"], true, "{reply}");
    let id = reply["payload"]["subscriptionId"].as_str().unwrap().to_owned();
    fixture.turn("turn-2").await;
    let frames = client.until(&id, |frame| settled(frame, 2)).await;
    let floor: u64 = after["position"].as_str().unwrap().parse().unwrap();
    assert!(frames.iter().all(|frame| position(frame) >= floor));
    let replayed = fixture.replayed().await;
    assert_eq!(view(frames.last().unwrap())["messages"], replayed["messages"]);
    client.close().await;
}

/// Row S12.
#[tokio::test]
async fn one_stalled_socket_delays_no_commit_and_no_other_subscriber() {
    let fixture = SubscriptionFixture::new().await;
    let mut stalled = fixture.connect();
    stalled.subscribe("subscribe", &fixture.id).await;
    stalled.next().await;
    let mut other = fixture.connect();
    let id = other.subscribe("subscribe", &fixture.id).await;
    other.next().await;
    fixture.stall(&mut stalled).await;
    let started = Instant::now();
    fixture.turn("turn-1").await;
    fixture.settled_read(1).await;
    other.until(&id, |frame| settled(frame, 1)).await;
    assert!(
        started.elapsed() < Duration::from_millis(SUBSCRIPTION_DELIVERY_TIMEOUT_MS / 2),
        "the commit and the other socket's frame waited on nothing: {:?}",
        started.elapsed()
    );
    stalled.close().await;
    other.close().await;
}

/// Row S13: what the live frames build is what a cold full replay builds.
/// Committed fields only: the live overlay (lifecycle, capabilities) is the
/// running agent's, which a cold read does not have.
#[tokio::test]
async fn live_frames_and_a_cold_full_replay_build_the_same_view() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    let id = client.subscribe("subscribe", &fixture.id).await;
    for turn in 1..=4 {
        fixture.turn(&format!("turn-{turn}")).await;
        fixture.settled_read(turn).await;
    }
    let frames = client.until(&id, |frame| settled(frame, 4)).await;
    let live = view(frames.last().unwrap()).clone();
    let replayed = fixture.replayed().await;
    for field in ["messages", "pending", "tools", "transcriptState", "queueComplete", "truncated"] {
        assert_eq!(live[field], replayed[field], "{field}");
    }
    client.close().await;
}

/// Row S14.
#[tokio::test]
async fn a_revoked_grant_ends_the_subscription_before_the_next_batch() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    let id = client.subscribe("subscribe", &fixture.id).await;
    client.next().await;
    grants(&fixture.authority, &["server.read"]);
    fixture
        .service
        .submit(
            fixture.id.clone(),
            caller(&fixture.session, "after-revoke".into()),
            "turn-1".into(),
            SubmittedMessage {
                text: "question".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let frames = client
        .until(&id, |frame| frame["event"] == "conversation.subscriptionEnded")
        .await;
    assert_eq!(frames.len(), 1, "no frame read after the revocation");
    let ended = &frames[0];
    assert_eq!(ended["payload"]["reason"], "refused");
    assert_eq!(ended["payload"]["code"], "forbidden");
    client.close().await;
}

/// Row S30: a batch woken while every request permit is taken waits for
/// one, and is admitted only once it holds it, so a grant revoked during
/// that wait lets no read through.
#[tokio::test]
async fn a_grant_revoked_while_a_batch_waits_for_capacity_ends_it_before_the_read() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    let id = client.subscribe("subscribe", &fixture.id).await;
    client.next().await;
    let permits = u32::try_from(fixture.state.limits.requests()).unwrap();
    let held = fixture
        .state
        .requests
        .clone()
        .acquire_many_owned(permits)
        .await
        .unwrap();
    // A turn wakes the subscription; its batch waits for a permit. The sleep
    // only lets the wake land before the revocation: if it lands after, the
    // old order passes too, so a slow run can miss the regression but never
    // fails this test wrongly.
    fixture
        .service
        .submit(
            fixture.id.clone(),
            caller(&fixture.session, "while-full".into()),
            "turn-1".into(),
            SubmittedMessage {
                text: "question".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    grants(&fixture.authority, &["server.read"]);
    drop(held);
    let frames = client
        .until(&id, |frame| frame["event"] == "conversation.subscriptionEnded")
        .await;
    assert_eq!(frames.len(), 1, "nothing read under the revoked grant: {frames:?}");
    assert_eq!(frames[0]["payload"]["reason"], "refused");
    assert_eq!(frames[0]["payload"]["code"], "forbidden");
    client.close().await;
}

/// Row S31: the subscribe reply is owed by the request's reply deadline, as
/// a watch's is. A writer held past it closes the socket rather than write
/// the identity after the client stopped waiting for it, which would leave
/// a live subscription the client cannot name and every retry refused
/// `subscription_duplicate`.
#[tokio::test]
async fn a_subscribe_reply_not_written_by_its_deadline_closes_the_socket() {
    let (captured, _guard) = limit_log();
    let fixture = SubscriptionFixture::new().await;
    // A write timeout well past the reply deadline, so only that deadline
    // can close the socket in time.
    let settings = SessionSettings::new(
        Duration::from_secs(10),
        RECORD_SEND_TIMEOUT * 4,
        Duration::from_secs(1),
    )
    .unwrap();
    let mut client = connect(fixture.state.clone().with_settings(settings), &fixture.session);
    fixture.stall(&mut client).await;
    client.send(
        "subscribe",
        "conversation.subscribe",
        json!({"conversationId": fixture.id.to_string()}),
    );
    // The first read finishes and its reply waits behind the held writer;
    // from here only timers move, so the clock may run ahead.
    let received = Instant::now();
    tokio::time::sleep(Duration::from_millis(500)).await;
    tokio::time::pause();
    timeout(RECORD_SEND_TIMEOUT * 2, &mut client.socket)
        .await
        .expect("closed by the reply deadline, before the write timeout")
        .unwrap();
    // At the reply's deadline: not earlier, as a first read refused at its
    // own deadline would close it.
    let closed = received.elapsed();
    assert!(
        closed >= RECORD_SEND_TIMEOUT - Duration::from_secs(1)
            && closed <= RECORD_SEND_TIMEOUT + Duration::from_secs(1),
        "{closed:?}"
    );
    let logged = limit_text(&captured);
    assert!(logged.contains("socket.subscription_delivery_deadline"), "{logged}");
    assert!(!logged.contains("socket.write_timeout"), "{logged}");
}

/// Row S15.
#[tokio::test]
async fn deleting_the_conversation_ends_its_subscription() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    let id = client.subscribe("subscribe", &fixture.id).await;
    client.next().await;
    let deleted = fixture
        .call(
            "conversation.delete",
            json!({"conversationId": fixture.id.to_string(), "requestId": "delete"}),
        )
        .await;
    assert_eq!(deleted["ok"], true, "{deleted}");
    let frames = client
        .until(&id, |frame| frame["event"] == "conversation.subscriptionEnded")
        .await;
    let ended = frames.last().unwrap();
    assert_eq!(ended["payload"]["reason"], "refused");
    assert_eq!(ended["payload"]["code"], "conversation_deleted");
    client.close().await;
}

/// Row S16.
#[tokio::test]
async fn no_frame_follows_an_unsubscribe_reply() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    let id = client.subscribe("subscribe", &fixture.id).await;
    client.next().await;
    fixture.turn("turn-1").await;
    client.send("stop", "conversation.unsubscribe", json!({"subscriptionId": id}));
    let (reply, _) = client.reply("stop").await;
    assert_eq!(reply["ok"], true);
    assert_eq!(reply["payload"]["subscriptionId"], id);
    fixture.turn("turn-2").await;
    client.quiet(&id).await;
    // It is gone: a second unsubscribe does not know it.
    client.send("again", "conversation.unsubscribe", json!({"subscriptionId": id}));
    let (again, _) = client.reply("again").await;
    assert_eq!(again["error"]["code"], "unknown_subscription");
    client.close().await;
}

/// Row S17.
#[tokio::test]
async fn subscriptions_past_the_published_limit_are_refused() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    let mut conversations = vec![fixture.id.clone()];
    while conversations.len() <= MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS {
        conversations.push(fixture.create().await);
    }
    for (index, conversation) in conversations
        .iter()
        .take(MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS)
        .enumerate()
    {
        client.subscribe(&format!("view-{index}"), conversation).await;
    }
    client.send(
        "one-too-many",
        "conversation.subscribe",
        json!({"conversationId": conversations.last().unwrap().to_string()}),
    );
    let (reply, _) = client.reply("one-too-many").await;
    assert_eq!(reply["error"]["code"], "subscription_capacity");
    client.send("list", "conversation.subscribeList", json!({}));
    assert_eq!(client.reply("list").await.0["ok"], true);
    client.send("archived", "conversation.subscribeList", json!({"archived": true}));
    let (reply, _) = client.reply("archived").await;
    assert_eq!(reply["error"]["code"], "subscription_capacity");
    client.close().await;
}

/// Row S18.
#[tokio::test]
async fn a_duplicate_subscription_is_refused() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    client.subscribe("first", &fixture.id).await;
    client.send(
        "second",
        "conversation.subscribe",
        json!({"conversationId": fixture.id.to_string()}),
    );
    let (reply, _) = client.reply("second").await;
    assert_eq!(reply["error"]["code"], "subscription_duplicate");
    client.close().await;
}

/// Row S20: an approval-mode change is a live fact of the view, not a record.
#[tokio::test]
async fn an_overlay_change_without_a_commit_is_delivered() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    let id = client.subscribe("subscribe", &fixture.id).await;
    let first = client.next().await;
    assert_eq!(view(&first)["approvalMode"], "ask");
    let changed = fixture
        .call(
            "conversation.setApprovalMode",
            json!({"conversationId": fixture.id.to_string(), "requestId": "mode", "mode": "auto"}),
        )
        .await;
    assert_eq!(changed["ok"], true, "{changed}");
    let frames = client
        .until(&id, |frame| view(frame)["approvalMode"] == "auto")
        .await;
    assert_eq!(
        frames.last().unwrap()["payload"]["cursor"],
        first["payload"]["cursor"],
        "no record was written"
    );
    client.close().await;
}

/// Row S26: following a conversation never opens its agent. A close lets
/// it go; the view says so once, with the same history, and the agent stays
/// closed until a send opens it.
#[tokio::test]
async fn a_closed_conversation_is_shown_let_go_and_not_opened_again() {
    let fixture = SubscriptionFixture::new().await;
    fixture.turn("e1").await;
    let mut client = fixture.connect();
    let id = client.subscribe("subscribe", &fixture.id).await;
    let open = client
        .until(&id, |frame| {
            settled(frame, 1) && view(frame)["lifecycle"]["phase"] == "attached"
        })
        .await
        .pop()
        .unwrap();
    let closed = fixture
        .call(
            "conversation.close",
            json!({"conversationId": fixture.id.to_string(), "requestId": "close"}),
        )
        .await;
    assert_eq!(closed["ok"], true, "{closed}");
    let let_go = client
        .until(&id, |frame| view(frame)["lifecycle"]["phase"] == "absent")
        .await
        .pop()
        .unwrap();
    assert!(position(&let_go) >= position(&open));
    assert_eq!(view(&let_go)["messages"], view(&open)["messages"]);
    assert_eq!(view(&let_go)["capabilities"]["queue"], false);
    // Not opened again: nothing more of it, and the list says it is not running.
    client.quiet(&id).await;
    let listed = fixture.call("conversation.list", json!({})).await;
    let row = listed["payload"]["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["conversationId"] == fixture.id.to_string())
        .cloned()
        .unwrap();
    assert_eq!(row["running"], false, "{row}");
    // A send opens it, and the view follows it again.
    fixture.turn("e2").await;
    client
        .until(&id, |frame| {
            settled(frame, 2) && view(frame)["lifecycle"]["phase"] == "attached"
        })
        .await;
    client.close().await;
}

/// Row S27: a follower shows a pending approval-mode change and leaves its
/// recovery, which stops and starts the agent, to a read or a send.
#[tokio::test]
async fn a_pending_mode_change_is_shown_not_recovered_by_a_follower() {
    let fixture = SubscriptionFixture::new().await;
    let conversation = fixture.seeded(1).await;
    fixture
        .metadata
        .begin_mode_change(ConversationModeRequest {
            conversation_id: conversation.clone(),
            organization_id: OrganizationId::new("organization").unwrap(),
            request_id: "pending-mode".into(),
            initiator_principal_id: PrincipalId::new("principal").unwrap(),
            initiator_surface_id: "panel".into(),
            prior: ConversationApprovalMode::Ask,
            requested: ConversationApprovalMode::Auto,
            state: ConversationModeRequestState::Pending,
            application: None,
            requested_at_ms: 1_700_000_000_000,
        })
        .await
        .unwrap();
    let opens = fixture.provider.open_calls.load(Ordering::SeqCst);
    let mut client = fixture.connect();
    let id = client.subscribe("subscribe", &conversation).await;
    let first = client.next().await;
    assert_eq!(
        view(&first)["approvalModeChange"]["requestId"],
        "pending-mode",
        "{first}"
    );
    assert_eq!(view(&first)["lifecycle"]["phase"], "absent", "{first}");
    client.quiet(&id).await;
    assert_eq!(
        fixture.provider.open_calls.load(Ordering::SeqCst),
        opens,
        "no agent opened"
    );
    client.close().await;
}

/// Row S29: a subscribe the session may not make is refused before it takes
/// a watch: with the SDK's watch pool full, it is refused `forbidden`, not
/// `subscription_capacity`.
#[tokio::test]
async fn a_forbidden_subscribe_is_refused_before_it_takes_a_watch() {
    let fixture = SubscriptionFixture::new().await;
    grants(&fixture.authority, &["server.read"]);
    let mut held = Vec::new();
    loop {
        match fixture.storage.watch_any_committed() {
            Ok(watch) => held.push(watch),
            Err(ChangeWatchError::Capacity) => break,
            Err(error) => panic!("{error:?}"),
        }
    }
    let mut client = fixture.connect();
    client.send(
        "view",
        "conversation.subscribe",
        json!({"conversationId": fixture.id.to_string()}),
    );
    let (reply, _) = client.reply("view").await;
    assert_eq!(reply["error"]["code"], "forbidden", "{reply}");
    client.send("list", "conversation.subscribeList", json!({}));
    let (reply, _) = client.reply("list").await;
    assert_eq!(reply["error"]["code"], "forbidden", "{reply}");
    drop(held);
    client.close().await;
}

/// Row S29: an unsubscribe is admitted like any request; one the session may
/// no longer make is refused and changes nothing.
#[tokio::test]
async fn a_forbidden_unsubscribe_is_refused() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    let id = client.subscribe("subscribe", &fixture.id).await;
    client.next().await;
    grants(&fixture.authority, &["server.read"]);
    client.send(
        "unsubscribe",
        "conversation.unsubscribe",
        json!({"subscriptionId": id}),
    );
    let (reply, _) = client.reply("unsubscribe").await;
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(reply["error"]["code"], "forbidden", "{reply}");
    client.close().await;
}

/// Row S21: every source a subscription registered goes with its socket.
#[tokio::test]
async fn closing_the_socket_drops_every_subscription() {
    let fixture = SubscriptionFixture::new().await;
    let other = fixture.create().await;
    let mut client = fixture.connect();
    client.subscribe("one", &fixture.id).await;
    client.subscribe("two", &other).await;
    client.send("list", "conversation.subscribeList", json!({}));
    assert_eq!(client.reply("list").await.0["ok"], true);
    client.close().await;
    // The SDK's watch registry is full again only if all three let go.
    let held = timeout(EXPECTED, async {
        loop {
            let mut held = Vec::new();
            loop {
                match fixture.storage.watch_any_committed() {
                    Ok(watch) => held.push(watch),
                    Err(ChangeWatchError::Capacity) => break,
                    Err(error) => panic!("{error:?}"),
                }
            }
            if held.len() == 64 {
                return held.len();
            }
            drop(held);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(held, 64);
    assert_eq!(
        fixture.state.requests.available_permits(),
        fixture.state.limits.requests()
    );
}

/// Row L1.
#[tokio::test]
async fn a_list_subscription_follows_the_catalogue() {
    let fixture = SubscriptionFixture::new().await;
    fixture.turn("turn-1").await;
    fixture.settled_read(1).await;
    let mut client = fixture.connect();
    client.send("list", "conversation.subscribeList", json!({}));
    let (reply, _) = client.reply("list").await;
    let id = reply["payload"]["subscriptionId"].as_str().unwrap().to_owned();
    let first = client.next().await;
    assert_eq!(first["event"], "conversation.listed");
    let rows = first["payload"]["list"]["conversations"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["conversationId"], fixture.id.to_string());
    let archived = fixture
        .call(
            "conversation.archive",
            json!({"conversationId": fixture.id.to_string(), "requestId": "archive"}),
        )
        .await;
    assert_eq!(archived["ok"], true, "{archived}");
    client
        .until(&id, |frame| {
            frame["payload"]["list"]["conversations"]
                .as_array()
                .is_some_and(Vec::is_empty)
        })
        .await;
    client.close().await;
}

/// Row L2: a turn running is a list fact that follows its commits.
#[tokio::test]
async fn a_turn_without_a_summary_change_updates_the_list() {
    let fixture = SubscriptionFixture::new().await;
    fixture.turn("turn-1").await;
    fixture.settled_read(1).await;
    let mut client = fixture.connect();
    client.send("list", "conversation.subscribeList", json!({}));
    let (reply, _) = client.reply("list").await;
    let id = reply["payload"]["subscriptionId"].as_str().unwrap().to_owned();
    let first = client.next().await;
    assert_eq!(first["payload"]["list"]["conversations"][0]["running"], false);
    let (release, gate) = tokio::sync::oneshot::channel();
    *fixture.provider.execution_gate.lock().unwrap() = Some(gate);
    fixture.turn("turn-2").await;
    client
        .until(&id, |frame| {
            frame["payload"]["list"]["conversations"][0]["running"] == true
        })
        .await;
    release.send(()).unwrap();
    client
        .until(&id, |frame| {
            frame["payload"]["list"]["conversations"][0]["running"] == false
        })
        .await;
    client.close().await;
}

/// Row L4: a list reads again no sooner than its floor after a wake, so a
/// turn's stream of commits costs a few list reads, not one per commit.
#[tokio::test]
async fn list_frames_come_no_closer_than_the_reread_floor() {
    use crate::product::subscription::LIST_REREAD_FLOOR;
    let fixture = SubscriptionFixture::new().await;
    fixture.turn("turn-1").await;
    fixture.settled_read(1).await;
    let mut client = fixture.connect();
    client.send("list", "conversation.subscribeList", json!({}));
    let (reply, _) = client.reply("list").await;
    let id = reply["payload"]["subscriptionId"].as_str().unwrap().to_owned();
    client.next().await;
    let (release, gate) = tokio::sync::oneshot::channel();
    *fixture.provider.execution_gate.lock().unwrap() = Some(gate);
    // Read while the turn starts, so each frame is timed as it arrives.
    let mut arrivals = Vec::new();
    let started = client.until(&id, |frame| {
        arrivals.push(Instant::now());
        frame["payload"]["list"]["conversations"][0]["running"] == true
    });
    tokio::join!(fixture.turn("turn-2"), started);
    release.send(()).unwrap();
    client
        .until(&id, |frame| {
            arrivals.push(Instant::now());
            frame["payload"]["list"]["conversations"][0]["running"] == false
        })
        .await;
    assert!(arrivals.len() >= 2, "{arrivals:?}");
    for pair in arrivals.windows(2) {
        assert!(pair[1] - pair[0] >= LIST_REREAD_FLOOR, "{arrivals:?}");
    }
    client.close().await;
}

/// Row L5: a catalogue larger than one list leaves rows out of the frame.
/// A change to one of those rows changes nothing the frame carries, and is
/// sent all the same, so the client walks the catalogue for it.
#[tokio::test]
async fn a_change_the_incomplete_list_leaves_out_is_still_sent() {
    use crate::conversation::application::{ConversationSummaries, MAX_LISTED_CONVERSATIONS};
    use nessa_protocol::conversation::domain::ConversationSummary;
    let fixture = SubscriptionFixture::new().await;
    let mut oldest = None;
    // One past the bound: the oldest is left out of the list.
    for created in 0..=MAX_LISTED_CONVERSATIONS {
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        fixture
            .metadata
            .create(
                Conversation::new(
                    id.clone(),
                    OrganizationId::new("organization").unwrap(),
                    PrincipalId::new("principal").unwrap(),
                    "panel".into(),
                    format!("create-{id}"),
                    created as u64,
                    AgentId::Claude,
                    ConversationModelId::new("test").unwrap(),
                    ConversationApprovalMode::Ask,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let said = ConversationSummary::after_message(None, "said", None, created as u64 + 1);
        fixture.metadata.record(&id, said).await.unwrap();
        oldest.get_or_insert(id);
    }
    let oldest = oldest.unwrap();
    let mut client = fixture.connect();
    client.send("list", "conversation.subscribeList", json!({}));
    let (reply, _) = client.reply("list").await;
    let id = reply["payload"]["subscriptionId"].as_str().unwrap().to_owned();
    let first = client.next().await;
    assert_eq!(first["payload"]["list"]["complete"], false);
    let rows = first["payload"]["list"]["conversations"].as_array().unwrap();
    assert!(
        rows.iter().all(|row| row["conversationId"] != oldest.to_string()),
        "the oldest is left out"
    );
    let archived = fixture
        .call(
            "conversation.archive",
            json!({"conversationId": oldest.to_string(), "requestId": "archive"}),
        )
        .await;
    assert_eq!(archived["ok"], true, "{archived}");
    let next = client.until(&id, |_| true).await;
    assert_eq!(next[0]["payload"]["list"], first["payload"]["list"], "the same rows");
    client.close().await;
}

/// Not a row: the gate's measurement (ADR 0009, "Expected-workload
/// performance"). The largest realistic fixture: a conversation as long as a
/// session may grow, of the two-kilobyte messages
/// `docs/design/bounded-terminal-discovery.md` calls realistic. Then a
/// subscriber follows it while `LIVE_TURNS` more turns run, and each turn's
/// latency is the time from its last commit (seen on a record watch of the
/// test's own) to the frame that shows it settled. Run with
/// `cargo test -p nessa-server --lib subscriptions::measure -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a measurement, not a regression: several minutes"]
async fn measure_commit_to_frame_latency_on_the_largest_realistic_fixture() {
    use nessa_sdk::application::agent_execution::executions::ExecutionUpdate;
    use nessa_sdk::domain::agent_execution::executions::MessageChunk;
    // A session holds at most 1,024 turns (`SessionSnapshot::MAX_INVOCATIONS`):
    // this history and the live turns after it come to 1,020 of them.
    const HISTORY_TURNS: usize = 990;
    const LIVE_TURNS: usize = 30;
    let answer = "an answer of about two kilobytes ".repeat(62);
    let fixture = SubscriptionFixture::new().await;
    let reply = |text: &str| {
        fixture
            .provider
            .execution_updates
            .lock()
            .unwrap()
            .push(ExecutionUpdate::Message(MessageChunk::text(text)));
    };
    let built = Instant::now();
    for turn in 1..=HISTORY_TURNS {
        reply(&answer);
        fixture.turn(&format!("history-{turn}")).await;
        // One at a time, as a person would: each turn settles first.
        fixture.turn_settled(&format!("history-{turn}")).await;
        if turn % 100 == 0 {
            eprintln!("history: {turn} turns after {:?}", built.elapsed());
        }
    }
    let built = built.elapsed();
    let commits = Arc::new(Mutex::new(Vec::<Instant>::new()));
    let mut watch = fixture
        .storage
        .watch_committed(&crate::conversation::application::conversation_session(&fixture.id)).unwrap();
    let seen = commits.clone();
    let watching = tokio::spawn(async move {
        while matches!(
            watch.changed().await,
            nessa_sdk::application::agent_execution::sessions::ChangeWatchState::Dirty
        ) {
            seen.lock().unwrap().push(Instant::now());
        }
    });
    let mut client = fixture.connect();
    let subscribed = Instant::now();
    let id = client.subscribe("subscribe", &fixture.id).await;
    let first = client.next().await;
    let first_frame = subscribed.elapsed();
    let frame_bytes = serde_json::to_string(&first).unwrap().len();
    let mut latencies = Vec::new();
    for turn in 1..=LIVE_TURNS {
        reply(&answer);
        fixture.turn(&format!("live-{turn}")).await;
        let frames = client
            .until(&id, |frame| {
                frame["event"] == "conversation.view"
                    && view(frame)["messages"]
                        .as_array()
                        .and_then(|all| all.last())
                        .is_some_and(|last| {
                            last["executionId"] == format!("live-{turn}")
                                && last["status"] == "completed"
                        })
            })
            .await;
        let arrived = Instant::now();
        assert!(!frames.is_empty());
        let last_commit = commits
            .lock()
            .unwrap()
            .iter()
            .copied()
            .filter(|at| *at <= arrived)
            .max()
            .unwrap();
        latencies.push(arrived - last_commit);
    }
    latencies.sort();
    let at = |q: f64| latencies[((latencies.len() as f64 - 1.0) * q).round() as usize];
    eprintln!(
        "fixture: {HISTORY_TURNS} turns ({} messages, ~2 KiB answers), built in {built:?}; \
         first frame {first_frame:?} ({frame_bytes} bytes); \
         commit-to-frame over {LIVE_TURNS} live turns: p50 {:?}, p95 {:?}, max {:?}",
        HISTORY_TURNS * 2,
        at(0.5),
        at(0.95),
        latencies.last().unwrap()
    );
    client.close().await;
    watching.abort();
}

// Read grants on the socket (issue 704): rows G8, G11–G14 of
// docs/design/read-grants.md.

/// Receiver bindings by credential: what pairing would have written.
struct Devices(std::collections::HashMap<&'static str, ReceiverBinding>);
impl Devices {
    fn new(entries: &[(&'static str, &str, &str, bool)]) -> Arc<Self> {
        Arc::new(Self(
            entries
                .iter()
                .map(|(credential, receiver, owner, active)| {
                    (
                        *credential,
                        ReceiverBinding {
                            receiver_id: (*receiver).into(),
                            credential_id: CredentialId::new(*credential).unwrap(),
                            organization_id: OrganizationId::new("organization").unwrap(),
                            owner_id: PrincipalId::new(*owner).unwrap(),
                            access_epoch: 1,
                            active: *active,
                        },
                    )
                })
                .collect(),
        ))
    }
}
impl ReceiverAuthority for Devices {
    fn resolve<'a>(
        &'a self,
        credential: &'a CredentialId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>> + Send + 'a>>
    {
        let found = self.0.get(credential.as_str()).cloned();
        Box::pin(async move { Ok(found) })
    }
}

impl SubscriptionFixture {
    /// Compose `devices` as the receiver authority and the fixture's store
    /// as the grant table, as `composition::local_auth` does.
    fn with_devices(mut self, devices: Arc<Devices>) -> Self {
        let metadata = self.metadata.clone();
        self.state = self
            .state
            .clone()
            .with_passive_read(devices, metadata.clone(), metadata);
        self
    }

    async fn grant_to(&self, transition: crate::conversation::application::ReadGrantTransition) {
        use crate::conversation::application::{ReadGrantChange, ReadGrants};
        assert!(self
            .metadata
            .change(ReadGrantChange {
                transition,
                conversation_id: self.id.clone(),
                receiver_id: Some("receiver".into()),
                credential_id: CredentialId::new("credential").unwrap(),
                initiator: caller(&self.session, "share".into()),
                at_ms: 1,
            })
            .await
            .unwrap());
    }

    async fn listed(&self) -> Vec<Value> {
        let listed = self.call("conversation.list", json!({})).await;
        assert_eq!(listed["ok"], true, "{listed}");
        listed["payload"]["conversations"].as_array().unwrap().clone()
    }
}

/// This session's credential is a paired device bound to receiver `receiver`.
fn this_device(active: bool) -> Arc<Devices> {
    Devices::new(&[("credential", "receiver", "principal", active)])
}

/// Row G11: the owner's own surface reads, lists and subscribes as before,
/// on a gateway that pairs nothing and on one that has paired devices.
#[tokio::test]
async fn an_owner_surface_reads_everything_it_owns() {
    for paired in [false, true] {
        let mut fixture = SubscriptionFixture::new().await;
        // A list shows a conversation once it has a summary.
        fixture.turn("first").await;
        if paired {
            fixture = fixture.with_devices(Devices::new(&[("phone", "phone", "principal", true)]));
        }
        let read = fixture
            .call(
                "conversation.read",
                json!({"conversationId": fixture.id.to_string()}),
            )
            .await;
        assert_eq!(read["ok"], true, "{read}");
        assert_eq!(fixture.listed().await.len(), 1);
        let mut client = fixture.connect();
        client.subscribe("subscribe", &fixture.id).await;
        client.close().await;
    }
}

/// Row G12: a paired device on the socket sees only what it was granted.
#[tokio::test]
async fn a_paired_device_on_the_socket_sees_only_what_it_was_granted() {
    let fixture = SubscriptionFixture::new().await;
    // A list shows a conversation once it has a summary.
    fixture.turn("first").await;
    let fixture = fixture.with_devices(this_device(true));
    let read = fixture
        .call(
            "conversation.read",
            json!({"conversationId": fixture.id.to_string()}),
        )
        .await;
    assert_eq!(read["error"]["code"], "conversation_not_found", "{read}");
    assert!(fixture.listed().await.is_empty());
    let mut client = fixture.connect();
    client.send(
        "view",
        "conversation.subscribe",
        json!({"conversationId": fixture.id.to_string()}),
    );
    let (reply, _) = client.reply("view").await;
    assert_eq!(reply["error"]["code"], "conversation_not_found", "{reply}");
    client.close().await;
    fixture
        .grant_to(crate::conversation::application::ReadGrantTransition::Grant)
        .await;
    let read = fixture
        .call(
            "conversation.read",
            json!({"conversationId": fixture.id.to_string()}),
        )
        .await;
    assert_eq!(read["ok"], true, "{read}");
    let listed = fixture.listed().await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["conversationId"], fixture.id.to_string());
}

/// Row G13: a revoke ends a device's subscription before its next batch.
#[tokio::test]
async fn a_revoke_ends_a_device_subscription_before_its_next_batch() {
    let fixture = SubscriptionFixture::new()
        .await
        .with_devices(this_device(true));
    fixture
        .grant_to(crate::conversation::application::ReadGrantTransition::Grant)
        .await;
    let mut client = fixture.connect();
    let id = client.subscribe("subscribe", &fixture.id).await;
    client.next().await;
    fixture
        .grant_to(crate::conversation::application::ReadGrantTransition::Revoke)
        .await;
    fixture.turn("after-revoke").await;
    let frames = client
        .until(&id, |frame| frame["event"] == "conversation.subscriptionEnded")
        .await;
    assert_eq!(frames.len(), 1, "no frame read after the revoke");
    assert_eq!(frames[0]["payload"]["reason"], "refused");
    assert_eq!(frames[0]["payload"]["code"], "conversation_not_found");
    client.close().await;
}

/// Row G14: an unpaired device reads nothing, whatever it was granted.
#[tokio::test]
async fn an_unpaired_device_reads_nothing_whatever_it_was_granted() {
    let fixture = SubscriptionFixture::new()
        .await
        .with_devices(this_device(false));
    fixture
        .grant_to(crate::conversation::application::ReadGrantTransition::Grant)
        .await;
    for (method, params) in [
        (
            "conversation.read",
            json!({"conversationId": fixture.id.to_string()}),
        ),
        ("conversation.list", json!({})),
    ] {
        let answer = fixture.call(method, params).await;
        assert_eq!(answer["error"]["code"], "unauthorized", "{method}: {answer}");
    }
}

/// Row G8: share refuses what the owner cannot grant, and writes nothing.
#[tokio::test]
async fn share_refuses_what_the_owner_cannot_grant() {
    let fixture = SubscriptionFixture::new().await.with_devices(Devices::new(&[
        ("phone", "phone", "principal", true),
        ("stranger", "stranger", "someone-else", true),
        ("gone", "gone", "principal", false),
    ]));
    *fixture.authority.snapshot.lock().unwrap() =
        snapshot(MembershipRole::Admin, MembershipStatus::Active);
    grants(
        &fixture.authority,
        &["conversation.write", "server.read", "credential.manage"],
    );
    let share = |id: String, credential: &str| {
        json!({"conversationId": id, "requestId": format!("share-{credential}"), "credentialId": credential})
    };
    for credential in ["stranger", "gone", "nobody"] {
        let answer = fixture
            .call(
                "conversation.share",
                share(fixture.id.to_string(), credential),
            )
            .await;
        assert_eq!(
            answer["error"]["code"], "share_target_not_paired",
            "{credential}: {answer}"
        );
    }
    let missing = Uuid::new_v4().to_string();
    let answer = fixture
        .call("conversation.share", share(missing, "phone"))
        .await;
    assert_eq!(answer["error"]["code"], "conversation_not_found", "{answer}");
    let shares = fixture
        .call(
            "conversation.shares",
            json!({"conversationId": fixture.id.to_string()}),
        )
        .await;
    assert_eq!(shares["payload"]["items"], json!([]), "{shares}");
    let answer = fixture
        .call("conversation.share", share(fixture.id.to_string(), "phone"))
        .await;
    assert_eq!(answer["payload"]["applied"], true, "{answer}");
    let shares = fixture
        .call(
            "conversation.shares",
            json!({"conversationId": fixture.id.to_string()}),
        )
        .await;
    assert_eq!(shares["payload"]["items"][0]["credentialId"], "phone");
    assert_eq!(shares["payload"]["items"][0]["role"], "read");
    let deleted = fixture
        .call(
            "conversation.delete",
            json!({"conversationId": fixture.id.to_string(), "requestId": "delete"}),
        )
        .await;
    assert_eq!(deleted["ok"], true, "{deleted}");
    let answer = fixture
        .call("conversation.share", share(fixture.id.to_string(), "phone"))
        .await;
    assert_eq!(answer["error"]["code"], "conversation_deleted", "{answer}");
    let answer = fixture
        .call(
            "conversation.unshare",
            share(fixture.id.to_string(), "phone"),
        )
        .await;
    assert_eq!(answer["payload"]["applied"], true, "{answer}");
}
