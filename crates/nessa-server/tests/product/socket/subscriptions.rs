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
    AgentsListResult, AgentsListed, MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS,
    SUBSCRIPTION_DELIVERY_TIMEOUT_MS,
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
    conversation_service_with_repository(storage, metadata, provider, metadata.clone())
}

fn conversation_service_with_repository(
    storage: &Arc<RecordStorage>,
    metadata: &Arc<LocalConversationStore>,
    provider: &Arc<ProviderFactory>,
    repository: Arc<dyn ConversationRepository>,
) -> Arc<ConversationService> {
    let service = ConversationService::new(
        ConversationDependencies {
            // Agents that can change approval mode (row S20).
            agents: mode_agents(
                provider.clone(),
                Arc::new(RecordingModeExecutionAudit::default()),
            ),
            storage: storage.clone(),
            metadata: repository,
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
            environment: crate::conversation::infrastructure::in_process_environment().into(),
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

/// The agents and SSH hosts a test gateway composes at startup: one agent,
/// one host.
fn composed_catalog() -> AgentsListResult {
    serde_json::from_value(json!({
        "environments": ["build.example"],
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
        }],
        "nativeHooks": {"claude": "off", "codex": "off"}
    }))
    .unwrap()
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
            .with_agents_catalog(composed_catalog());
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

    /// A turn the owner sends, while the fixture's session is a paired device
    /// or its receiver lookups are scripted: sent as a gateway that pairs
    /// nothing would send it, so it asks no receiver binding.
    async fn owner_turn(&self, execution: &str) {
        let mut owner = self.state.clone();
        owner.passive_read = None;
        let params = json!({
            "conversationId": self.id.to_string(),
            "requestId": format!("send-{execution}"),
            "executionId": execution,
            "text": format!("question {execution}"),
            "attachments": [],
            "files": [],
        });
        let frame = RequestFrame::new("call", "conversation.send", &params).unwrap();
        let OutgoingMessage::Response(sent) = dispatch(&owner, &self.session, frame).await else {
            panic!("a response")
        };
        let sent = serde_json::to_value(sent).unwrap();
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
                        lease: None,
                        artifacts: Vec::new(),
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
                    lease: None,
                    artifacts: Vec::new(),
                },
                vec![SessionSaveUnit::new(changes).unwrap()],
            )
            .await
            .unwrap();
    }

    /// Who the saved turn `execution` says started it, as its record holds.
    async fn initiation_of(
        &self,
        execution: &str,
    ) -> nessa_sdk::domain::agent_execution::executions::Initiation {
        let session = crate::conversation::application::conversation_session(&self.id);
        let committed = self.storage.read_committed(session).await.unwrap().unwrap();
        committed
            .snapshot()
            .unwrap()
            .invocations
            .iter()
            .find(|record| record.request.execution_id.as_str() == execution)
            .unwrap_or_else(|| panic!("a saved turn {execution}"))
            .initiation
            .clone()
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

/// A view says what time it is on the gateway, read from the view clock as
/// the view is made, on a one-shot read and on each subscription frame: a
/// client measures a deadline's `dueAtMs` against it, not against its own
/// clock (ADR 141). By default that clock is the process timeline the agents
/// promise `dueAtMs` on, not the gateway's credential clock, which a wall
/// clock correction moves on its own.
#[tokio::test]
async fn a_view_carries_the_gateways_time_from_its_clock() {
    struct ViewClock(AtomicU64);
    impl Clock for ViewClock {
        fn unix_milliseconds(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }
    let mut fixture = SubscriptionFixture::new().await;
    fixture.authority.now.store(123, Ordering::SeqCst);
    let before = nessa_sdk::infrastructure::clock::process_unix_ms();
    let read = fixture
        .call(
            "conversation.read",
            json!({"conversationId": fixture.id.to_string()}),
        )
        .await;
    let after = nessa_sdk::infrastructure::clock::process_unix_ms();
    let stamped = read["payload"]["serverNowMs"].as_u64().unwrap();
    assert!(
        (before..=after).contains(&stamped),
        "{stamped} is not on the process timeline [{before}, {after}]"
    );

    let clock = Arc::new(ViewClock(AtomicU64::new(7_000)));
    fixture.state = fixture.state.clone().with_view_clock(clock.clone());
    let read = fixture
        .call(
            "conversation.read",
            json!({"conversationId": fixture.id.to_string()}),
        )
        .await;
    assert_eq!(read["payload"]["serverNowMs"], 7_000, "{read}");
    let mut client = fixture.connect();
    let id = client.subscribe("subscribe", &fixture.id).await;
    let first = client.next().await;
    assert_eq!(view(&first)["serverNowMs"], 7_000);
    // A later frame is stamped when it is read, not when the follow began.
    clock.0.store(9_000, Ordering::SeqCst);
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
    assert_eq!(view(frames.last().unwrap())["serverNowMs"], 9_000);
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
        client
            .subscribe(&format!("view-{index}"), conversation)
            .await;
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
    client.send(
        "archived",
        "conversation.subscribeList",
        json!({"archived": true}),
    );
    let (reply, _) = client.reply("archived").await;
    assert_eq!(reply["ok"], true, "{reply}");
    let archived = reply["payload"]["subscriptionId"]
        .as_str()
        .unwrap()
        .to_owned();
    client.send(
        "duplicate-list",
        "conversation.subscribeList",
        json!({"archived": true}),
    );
    assert_eq!(
        client.reply("duplicate-list").await.0["error"]["code"],
        "subscription_duplicate"
    );
    client.send(
        "release-archived",
        "conversation.unsubscribe",
        json!({"subscriptionId": archived}),
    );
    assert_eq!(client.reply("release-archived").await.0["ok"], true);
    client.send(
        "archived-again",
        "conversation.subscribeList",
        json!({"archived": true}),
    );
    assert_eq!(client.reply("archived-again").await.0["ok"], true);
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

    async fn grant_to(&self, transition: crate::conversation::application::ShareGrantTransition) {
        use crate::conversation::application::{ShareGrantChange, ShareGrants};
        assert!(self
            .metadata
            .change(ShareGrantChange {
                transition,
                conversation_id: self.id.clone(),
                receiver_id: Some("receiver".into()),
                credential_id: CredentialId::new("credential").unwrap(),
                initiator: caller(&self.session, uuid::Uuid::new_v4().to_string()),
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

    /// `conversation.list`, `.observe` and `.subscribeList` all answer
    /// `forbidden`, so no list row or cursor reaches a device.
    async fn assert_lists_refused(&self) {
        for method in ["conversation.list", "conversation.observe"] {
            let reply = self.call(method, json!({})).await;
            assert_eq!(reply["error"]["code"], "forbidden", "{method}: {reply}");
        }
        // With the watch pool full, a refused subscribe answers `forbidden`,
        // not `subscription_capacity`: it took no watch first (row S29).
        let mut held = Vec::new();
        loop {
            match self.storage.watch_any_committed() {
                Ok(watch) => held.push(watch),
                Err(ChangeWatchError::Capacity) => break,
                Err(error) => panic!("{error:?}"),
            }
        }
        let mut client = self.connect();
        client.send("list", "conversation.subscribeList", json!({}));
        let (reply, _) = client.reply("list").await;
        assert_eq!(reply["error"]["code"], "forbidden", "{reply}");
        drop(held);
        client.close().await;
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
    // The socket's lists are the owner's: a device lists through its
    // catalogue, so it is refused them, whatever it was granted.
    fixture.assert_lists_refused().await;
    let shared = fixture
        .call(
            "conversation.shares",
            json!({"conversationId": fixture.id.to_string()}),
        )
        .await;
    assert_eq!(shared["error"]["code"], "forbidden", "{shared}");
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
        .grant_to(crate::conversation::application::ShareGrantTransition::Grant(crate::conversation::domain::ShareAccess::read()))
        .await;
    let read = fixture
        .call(
            "conversation.read",
            json!({"conversationId": fixture.id.to_string()}),
        )
        .await;
    assert_eq!(read["ok"], true, "{read}");
    fixture.assert_lists_refused().await;
}

/// Row H8 (`docs/design/auth/peer-gateways.md`): a session whose receiver
/// binding's owner is not the session's principal, the shape a paired peer
/// gateway has, is a paired reader on the socket like a device: `reader_of`
/// matches its binding by credential and organization, not by owner, and its
/// read and view subscription then ask its receiver's grant. Ungranted, both
/// answer `conversation_not_found`; granted, the read answers. Its lists are
/// refused, as a device's are. (A real peer never gets this far: these
/// methods need `conversation.write`, which a gateway principal can never
/// hold, so the policy refuses them first.)
#[tokio::test]
async fn a_binding_owned_by_another_principal_reads_only_what_it_was_granted() {
    let fixture = SubscriptionFixture::new().await;
    fixture.turn("first").await;
    let fixture = fixture.with_devices(Devices::new(&[("credential", "receiver", "grantor", true)]));
    let conversation = json!({"conversationId": fixture.id.to_string()});
    let read = fixture.call("conversation.read", conversation.clone()).await;
    assert_eq!(read["error"]["code"], "conversation_not_found", "{read}");
    let mut client = fixture.connect();
    client.send("view", "conversation.subscribe", conversation.clone());
    let (reply, _) = client.reply("view").await;
    assert_eq!(reply["error"]["code"], "conversation_not_found", "{reply}");
    client.close().await;
    fixture
        .grant_to(crate::conversation::application::ShareGrantTransition::Grant(crate::conversation::domain::ShareAccess::read()))
        .await;
    let read = fixture.call("conversation.read", conversation).await;
    assert_eq!(read["ok"], true, "{read}");
    fixture.assert_lists_refused().await;
}

/// Slice D2: `artifact.read` admits a paired reader as `conversation.read`
/// does. Ungranted, a held file in the conversation answers
/// `conversation_not_found`; granted, its bytes; an inactive binding is
/// refused; and a binding owned by another principal, the shape a peer
/// gateway has, is granted the conversation but not the caller's to read
/// files in, as its own principal owns none.
#[tokio::test]
async fn a_paired_reader_reads_back_a_held_file_only_where_it_was_granted() {
    use crate::attachments::application::AttachmentLimits;
    use crate::attachments_test_support::{digest_of, Fixture};
    let held = Fixture::new(AttachmentLimits::default());
    let fixture = SubscriptionFixture::new().await;
    grants(
        &fixture.authority,
        &["conversation.read", "conversation.write", "server.read"],
    );
    held.held.hold("organization", &fixture.id.to_string(), b"a held file", "text/plain");
    held.ownership
        .give(&fixture.id.to_string(), "organization", "principal");
    let read = json!({
        "conversationId": fixture.id.to_string(),
        "digest": digest_of(b"a held file").to_string(),
        "mimeType": "text/plain",
        "size": 11,
        "offset": 0,
        "maxBytes": 5,
    });
    let with = |fixture: SubscriptionFixture, devices| {
        let mut fixture = fixture.with_devices(devices);
        fixture.state = fixture
            .state
            .clone()
            .with_attachments(held.service.clone(), held.reads());
        fixture
    };
    let fixture = with(fixture, this_device(true));
    let reply = fixture.call("artifact.read", read.clone()).await;
    assert_eq!(reply["error"]["code"], "conversation_not_found", "{reply}");
    fixture
        .grant_to(crate::conversation::application::ShareGrantTransition::Grant(crate::conversation::domain::ShareAccess::read()))
        .await;
    let reply = fixture.call("artifact.read", read.clone()).await;
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["payload"]["data"], "YSBoZWw=");
    let fixture = with(fixture, this_device(false));
    let reply = fixture.call("artifact.read", read.clone()).await;
    assert_eq!(reply["error"]["code"], "unauthorized", "{reply}");
    let peer = with(
        fixture,
        Devices::new(&[("credential", "receiver", "grantor", true)]),
    );
    // The fixture's session is the conversation owner's principal, so a
    // binding owned by another principal is the peer's shape only for the
    // grant; the ownership check is what refuses a peer's own principal.
    held.ownership.give(&peer.id.to_string(), "organization", "grantor");
    let reply = peer.call("artifact.read", read).await;
    assert_eq!(reply["error"]["code"], "conversation_not_found", "{reply}");
}

/// Row G13: a revoke ends a device's subscription before its next batch.
#[tokio::test]
async fn a_revoke_ends_a_device_subscription_before_its_next_batch() {
    let fixture = SubscriptionFixture::new()
        .await
        .with_devices(this_device(true));
    fixture
        .grant_to(crate::conversation::application::ShareGrantTransition::Grant(crate::conversation::domain::ShareAccess::read()))
        .await;
    let mut client = fixture.connect();
    let id = client.subscribe("subscribe", &fixture.id).await;
    client.next().await;
    fixture
        .grant_to(crate::conversation::application::ShareGrantTransition::Revoke)
        .await;
    fixture.owner_turn("after-revoke").await;
    let frames = client
        .until(&id, |frame| frame["event"] == "conversation.subscriptionEnded")
        .await;
    assert_eq!(frames.len(), 1, "no frame read after the revoke");
    assert_eq!(frames[0]["payload"]["reason"], "refused");
    assert_eq!(frames[0]["payload"]["code"], "conversation_not_found");
    client.close().await;
}

/// Receiver bindings that cannot be read for the next `failures` lookups,
/// then show no device: an owner's surface on a gateway whose receiver
/// store missed a beat.
struct FlakyReceivers(std::sync::Mutex<u32>);
impl ReceiverAuthority for FlakyReceivers {
    fn resolve<'a>(
        &'a self,
        _credential: &'a CredentialId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>> + Send + 'a>>
    {
        let failed = {
            let mut left = self.0.lock().unwrap();
            let failed = *left > 0;
            *left = left.saturating_sub(1);
            failed
        };
        Box::pin(async move {
            if failed {
                Err(ReadRefusal::Unverifiable)
            } else {
                Ok(None)
            }
        })
    }
}

/// Row G11: one receiver lookup that cannot be verified is retried as a
/// read slot is, so it does not end an owner's subscription.
#[tokio::test]
async fn an_owner_subscription_outlasts_one_failed_receiver_lookup() {
    let receivers = Arc::new(FlakyReceivers(std::sync::Mutex::new(0)));
    let mut fixture = SubscriptionFixture::new().await;
    let metadata = fixture.metadata.clone();
    fixture.state = fixture
        .state
        .clone()
        .with_passive_read(receivers.clone(), metadata.clone(), metadata);
    let mut client = fixture.connect();
    let id = client.subscribe("subscribe", &fixture.id).await;
    client.next().await;
    *receivers.0.lock().unwrap() = 1;
    fixture.owner_turn("after-a-miss").await;
    let frames = client
        .until(&id, |frame| {
            frame["event"] == "conversation.view"
                || frame["event"] == "conversation.subscriptionEnded"
        })
        .await;
    let last = frames.last().unwrap();
    assert_eq!(last["event"], "conversation.view", "{last}");
    assert_eq!(*receivers.0.lock().unwrap(), 0);
    client.close().await;
}

/// Row G14: an unpaired device reads nothing, whatever it was granted.
#[tokio::test]
async fn an_unpaired_device_reads_nothing_whatever_it_was_granted() {
    let fixture = SubscriptionFixture::new()
        .await
        .with_devices(this_device(false));
    fixture
        .grant_to(crate::conversation::application::ShareGrantTransition::Grant(crate::conversation::domain::ShareAccess::read()))
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
    // The fake answers every share target with this snapshot, and a target
    // must hold the read it is being shared (row I1).
    grants(
        &fixture.authority,
        &[
            "conversation.write",
            "conversation.read",
            "server.read",
            "credential.manage",
        ],
    );
    let share = |id: String, credential: &str| {
        json!({"conversationId": id, "requestId": format!("share-{credential}"), "credentialId": credential, "role": "read"})
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
    // A new request: the share's own request id names the other change, so
    // reusing it for the unshare would be refused invalid_request (row G16).
    // An unshare names no role: it ends a grant of any role (row J8).
    let unshare = json!({
        "conversationId": fixture.id.to_string(),
        "requestId": "unshare-phone",
        "credentialId": "phone",
    });
    let answer = fixture.call("conversation.unshare", unshare).await;
    assert_eq!(answer["payload"]["applied"], true, "{answer}");
}

// Roles and tool policies on the socket (issue 707): rows J1, J3, J7 and
// J11 to J14 of docs/design/read-grants.md.

/// Row J1: a device holding a Comment or a Drive grant reads on the
/// socket exactly as one holding Read (G12): `conversation.read` and a view
/// subscription are admitted and the owner's lists are refused.
#[tokio::test]
async fn a_comment_or_drive_grant_reads_on_the_socket_as_read_does() {
    use crate::conversation::application::ShareGrantTransition;
    for access in crate::conversation_test_support::roles_beyond_read() {
        let fixture = SubscriptionFixture::new().await;
        fixture.turn("first").await;
        let fixture = fixture.with_devices(this_device(true));
        fixture
            .grant_to(ShareGrantTransition::Grant(access.clone()))
            .await;
        let read = fixture
            .call(
                "conversation.read",
                json!({"conversationId": fixture.id.to_string()}),
            )
            .await;
        assert_eq!(read["ok"], true, "{access:?}: {read}");
        let mut client = fixture.connect();
        client.subscribe("subscribe", &fixture.id).await;
        client.close().await;
        fixture.assert_lists_refused().await;
        fixture.grant_to(ShareGrantTransition::Revoke).await;
        let read = fixture
            .call(
                "conversation.read",
                json!({"conversationId": fixture.id.to_string()}),
            )
            .await;
        assert_eq!(read["error"]["code"], "conversation_not_found", "{access:?}: {read}");
    }
}

/// The commands a Drive grant admits (row J11), in the order a test sends
/// them.
const DRIVE_COMMANDS: [&str; 7] = [
    "conversation.send",
    "conversation.steer",
    "conversation.stop",
    "conversation.answer",
    "conversation.answerQuestion",
    "conversation.cancel",
    "conversation.receipt",
];

/// A Drive grant with the widest policy, which nothing enforces yet.
fn drive_access() -> crate::conversation::domain::ShareAccess {
    use crate::conversation::domain::{ShareAccess, ShareRole, ToolPolicy, ToolPreset};
    ShareAccess::new(ShareRole::Drive, Some(ToolPolicy::Preset(ToolPreset::All))).unwrap()
}

/// Params any Drive command's handler reads, naming conversation `id`: each
/// handler takes the fields it knows, so the test's question is admission
/// alone.
fn drive_params(method: &str, id: &ConversationId, execution: &str) -> Value {
    let mut params = json!({
        "conversationId": id.to_string(),
        "requestId": format!("{method}-{execution}"),
        "executionId": execution,
    });
    match method {
        "conversation.send" | "conversation.steer" => {
            params["text"] = json!(format!("question {execution}"));
            params["attachments"] = json!([]);
            params["files"] = json!([]);
        }
        "conversation.answer" => {
            params["permissionId"] = json!("permission");
            params["optionId"] = json!("allow");
        }
        "conversation.cancel" => {
            params["permissionId"] = json!("permission");
            params["reason"] = json!("no");
        }
        "conversation.answerQuestion" => params["questionId"] = json!("question"),
        // The receipt of the message `SubscriptionFixture::turn` sent.
        "conversation.receipt" => {
            params["requestId"] = json!(format!("send-{execution}"));
            params["operation"] = json!("submit");
            params["text"] = json!(format!("question {execution}"));
            params["attachments"] = json!([]);
            params["files"] = json!([]);
        }
        _ => {}
    }
    params
}

/// Whether `answer` got past admission: the handler answered it, success or
/// its own refusal, and admission refused nothing.
fn admitted(answer: &Value) -> bool {
    let code = &answer["error"]["code"];
    !["forbidden", "conversation_not_found", "unauthorized", "unknown_method"]
        .iter()
        .any(|refused| code == refused)
}

/// Row J11: a device holding pairing's `conversation.read` and a Drive grant
/// drives the conversation the grant names. Its message starts a turn there
/// as the owner's would, and each of the other Drive commands reaches its
/// handler. The same commands naming another of the owner's conversations
/// are refused `conversation_not_found`, as reading it is.
#[tokio::test]
async fn a_drive_grant_drives_only_the_conversation_it_names() {
    let fixture = SubscriptionFixture::new().await;
    fixture.turn("first").await;
    let other = fixture.create().await;
    let fixture = fixture.with_devices(this_device(true));
    fixture
        .grant_to(crate::conversation::application::ShareGrantTransition::Grant(drive_access()))
        .await;
    grants(&fixture.authority, &["conversation.read", "server.read"]);
    fixture.turn("driven").await;
    let view = fixture.settled_read(2).await;
    assert!(view.to_string().contains("question driven"), "{view}");
    for method in DRIVE_COMMANDS {
        let answer = fixture
            .call(method, drive_params(method, &fixture.id, "driven"))
            .await;
        assert!(admitted(&answer), "{method}: {answer}");
        let answer = fixture
            .call(method, drive_params(method, &other, "elsewhere"))
            .await;
        assert_eq!(
            answer["error"]["code"], "conversation_not_found",
            "{method} elsewhere: {answer}"
        );
    }
    let receipt = fixture
        .call(
            "conversation.receipt",
            drive_params("conversation.receipt", &fixture.id, "driven"),
        )
        .await;
    assert_eq!(receipt["ok"], true, "{receipt}");
    assert_eq!(receipt["payload"]["found"], true, "{receipt}");
}

/// Row J12: a paired session is refused `forbidden` every other method whose
/// grant is `conversation.write`, even holding that grant, so a device can
/// never act as its owner beyond what its share grants admit. The methods
/// are every published one, not a list kept here. A Read or Comment grant
/// drives nothing.
#[tokio::test]
async fn a_paired_session_writes_only_what_a_drive_grant_admits() {
    use crate::conversation::application::ShareGrantTransition;
    use nessa_protocol::product::generated::{action_for_method, PRODUCT_READY_METHODS};
    let own_checks = [
        "conversation.read",
        "conversation.list",
        "conversation.observe",
        "conversation.subscribe",
        "conversation.subscribeList",
        "conversation.unsubscribe",
    ];
    let fixture = SubscriptionFixture::new().await;
    fixture.turn("first").await;
    let fixture = fixture.with_devices(this_device(true));
    fixture
        .grant_to(ShareGrantTransition::Grant(drive_access()))
        .await;
    grants(
        &fixture.authority,
        &["conversation.write", "conversation.read", "server.read"],
    );
    let others: Vec<&str> = PRODUCT_READY_METHODS
        .iter()
        .copied()
        .filter(|method| action_for_method(method) == Some("conversation.write"))
        .filter(|method| !DRIVE_COMMANDS.contains(method) && !own_checks.contains(method))
        .collect();
    for method in ["conversation.create", "conversation.delete", "conversation.setApprovalMode"] {
        assert!(others.contains(&method), "{method} is refused to a driver");
    }
    for method in others {
        let answer = fixture
            .call(method, drive_params(method, &fixture.id, "other"))
            .await;
        assert_eq!(answer["error"]["code"], "forbidden", "{method}: {answer}");
    }
    use crate::conversation::domain::{ShareAccess, ShareRole};
    for role in [ShareRole::Read, ShareRole::Comment] {
        let access = ShareAccess::new(role, None).unwrap();
        fixture.grant_to(ShareGrantTransition::Grant(access.clone())).await;
        for method in DRIVE_COMMANDS {
            let answer = fixture
                .call(method, drive_params(method, &fixture.id, "below"))
                .await;
            assert_eq!(answer["error"]["code"], "forbidden", "{access:?} {method}: {answer}");
        }
    }
    // A party signed in as a principal other than its grantor, as a peer
    // gateway is, drives nothing here even holding Drive.
    let fixture = fixture.with_devices(Devices::new(&[("credential", "receiver", "grantor", true)]));
    fixture
        .grant_to(ShareGrantTransition::Grant(drive_access()))
        .await;
    for method in DRIVE_COMMANDS {
        let answer = fixture
            .call(method, drive_params(method, &fixture.id, "peer"))
            .await;
        assert_eq!(answer["error"]["code"], "forbidden", "peer {method}: {answer}");
    }
}

/// Row J13: the role is read at each command. Lowered to Read, the next
/// command is refused `forbidden`; revoked, `conversation_not_found`; and a
/// device unpaired is refused `unauthorized` whatever it was granted.
/// Without pairing's `conversation.read`, Cedar refuses it `forbidden`
/// before its grant is read.
#[tokio::test]
async fn a_drive_grant_lowered_revoked_or_unpaired_drives_no_more() {
    use crate::conversation::application::ShareGrantTransition;
    use crate::conversation::domain::{ShareAccess, ShareRole};
    let fixture = SubscriptionFixture::new().await;
    fixture.turn("first").await;
    let fixture = fixture.with_devices(this_device(true));
    fixture
        .grant_to(ShareGrantTransition::Grant(drive_access()))
        .await;
    grants(&fixture.authority, &["server.read"]);
    let id = fixture.id.clone();
    let send = |execution: &str| drive_params("conversation.send", &id, execution);
    let answer = fixture.call("conversation.send", send("no-pairing")).await;
    assert_eq!(answer["error"]["code"], "forbidden", "{answer}");
    grants(&fixture.authority, &["conversation.read", "server.read"]);
    fixture.turn("driven").await;
    fixture
        .grant_to(ShareGrantTransition::Grant(
            ShareAccess::new(ShareRole::Read, None).unwrap(),
        ))
        .await;
    let answer = fixture.call("conversation.send", send("lowered")).await;
    assert_eq!(answer["error"]["code"], "forbidden", "{answer}");
    fixture.grant_to(ShareGrantTransition::Revoke).await;
    let answer = fixture.call("conversation.send", send("revoked")).await;
    assert_eq!(answer["error"]["code"], "conversation_not_found", "{answer}");
    fixture
        .grant_to(ShareGrantTransition::Grant(drive_access()))
        .await;
    let fixture = fixture.with_devices(this_device(false));
    let answer = fixture.call("conversation.send", send("unpaired")).await;
    assert_eq!(answer["error"]["code"], "unauthorized", "{answer}");
}

/// Row J14: a driver sends text. A message naming an image or a linked
/// file is refused `forbidden`, since either names something of the owner's
/// that was never shared, and so is the receipt of a create, which Drive
/// never makes.
#[tokio::test]
async fn a_driver_sends_text_and_names_nothing_of_the_owners() {
    let fixture = SubscriptionFixture::new().await;
    fixture.turn("first").await;
    let fixture = fixture.with_devices(this_device(true));
    fixture
        .grant_to(crate::conversation::application::ShareGrantTransition::Grant(drive_access()))
        .await;
    grants(&fixture.authority, &["conversation.read", "server.read"]);
    for method in ["conversation.send", "conversation.steer"] {
        for (field, value) in [
            ("attachments", json!([{"digest": "a".repeat(64), "mimeType": "image/png", "size": 1}])),
            ("files", json!([{"path": "/home/owner/.ssh/id_ed25519"}])),
        ] {
            let mut params = drive_params(method, &fixture.id, "named");
            params[field] = value;
            let answer = fixture.call(method, params).await;
            assert_eq!(answer["error"]["code"], "forbidden", "{method} {field}: {answer}");
        }
    }
    let mut params = drive_params("conversation.receipt", &fixture.id, "named");
    params["files"] = json!([{"path": "/home/owner/notes.txt"}]);
    let answer = fixture.call("conversation.receipt", params).await;
    assert_eq!(answer["error"]["code"], "forbidden", "{answer}");
    let mut params = drive_params("conversation.receipt", &fixture.id, "named");
    params["operation"] = json!("create");
    let answer = fixture.call("conversation.receipt", params).await;
    assert_eq!(answer["error"]["code"], "forbidden", "{answer}");
}

/// Row J15: a turn is saved with who started it. The owner's turn says the
/// owner, from its own surface and while devices are paired. A driver's,
/// sent or steered, says the Drive grant that admitted it, with the role and
/// the policy revision read for that command, and each says its tools ran
/// unchecked. The stamp is per turn: a policy changed later stamps the next
/// turn, and a retry of the same request keeps the revision it was admitted
/// with.
#[tokio::test]
async fn a_turn_is_saved_with_who_started_it() {
    use crate::conversation::application::{ShareGrantTransition, ShareGrants};
    use crate::conversation::domain::{ShareAccess, ShareRole, ToolPolicy, ToolPreset};
    use nessa_sdk::domain::agent_execution::executions::{
        GrantInitiator, GrantRole, Initiation, ToolChecking,
    };
    let fixture = SubscriptionFixture::new().await;
    fixture.turn("first").await;
    let fixture = fixture.with_devices(this_device(true));
    fixture
        .grant_to(ShareGrantTransition::Grant(drive_access()))
        .await;
    let revision = || async {
        let held = fixture.metadata.grants(&fixture.id).await.unwrap();
        held[0].policy_revision
    };
    let granted = revision().await;
    grants(&fixture.authority, &["conversation.read", "server.read"]);
    fixture.turn("driven").await;
    let steered = fixture
        .call(
            "conversation.steer",
            drive_params("conversation.steer", &fixture.id, "steered"),
        )
        .await;
    assert_eq!(steered["ok"], true, "{steered}");
    fixture
        .grant_to(ShareGrantTransition::Grant(
            ShareAccess::new(
                ShareRole::Drive,
                Some(ToolPolicy::Preset(ToolPreset::ReadOnlyTools)),
            )
            .unwrap(),
        ))
        .await;
    let changed = revision().await;
    assert!(changed > granted);
    // The same request again, answered from its receipt.
    fixture.turn("driven").await;
    fixture.turn("again").await;
    grants(
        &fixture.authority,
        &["conversation.write", "conversation.read", "server.read"],
    );
    fixture.owner_turn("owner").await;
    let driver = |revision| {
        Initiation::grant(GrantInitiator::new("receiver", GrantRole::Drive, revision).unwrap())
    };
    for (execution, expected) in [
        ("first", Initiation::owner()),
        ("driven", driver(granted)),
        ("steered", driver(granted)),
        ("again", driver(changed)),
        ("owner", Initiation::owner()),
    ] {
        let saved = fixture.initiation_of(execution).await;
        assert_eq!(saved, expected, "{execution}");
        assert_eq!(saved.tools(), ToolChecking::NotChecked, "{execution}");
    }
    // The owner's own surface, unbound, on a gateway that pairs a device.
    let owner = SubscriptionFixture::new()
        .await
        .with_devices(Devices::new(&[("phone", "phone", "principal", true)]));
    owner.turn("paired").await;
    assert_eq!(owner.initiation_of("paired").await, Initiation::owner());
}

/// The owner's share fixture: a paired device `phone` and `tablet`, and the
/// owner's session able to share with them.
async fn sharing_fixture() -> SubscriptionFixture {
    let fixture = SubscriptionFixture::new().await.with_devices(Devices::new(&[
        ("phone", "phone", "principal", true),
        ("tablet", "tablet", "principal", true),
    ]));
    *fixture.authority.snapshot.lock().unwrap() =
        snapshot(MembershipRole::Admin, MembershipStatus::Active);
    grants(
        &fixture.authority,
        &[
            "conversation.write",
            "conversation.read",
            "server.read",
            "credential.manage",
        ],
    );
    fixture
}

/// Row J3: a role and policy that do not fit together, or a policy past its
/// bounds or of no known shape, are refused `invalid_request` and write
/// nothing.
#[tokio::test]
async fn a_share_whose_role_and_policy_do_not_fit_is_refused_and_writes_nothing() {
    use crate::conversation::domain::ToolRules;
    let fixture = sharing_fixture().await;
    let longest = "x".repeat(ToolRules::MAX_RULE_BYTES);
    let too_long = format!("{longest}x");
    let too_many: Vec<String> = (0..=ToolRules::MAX_RULES).map(|n| format!("t{n}")).collect();
    let refused = [
        json!({"role": "drive"}),
        json!({"role": "read", "policy": {"preset": "no_shell"}}),
        json!({"role": "comment", "policy": {"preset": "all"}}),
        json!({"role": "drive", "policy": {"allow": [], "deny": too_many}}),
        json!({"role": "drive", "policy": {"allow": [], "deny": []}}),
        json!({"role": "drive", "policy": {"allow": [too_long], "deny": []}}),
        json!({"role": "drive", "policy": {"allow": [" Read"], "deny": []}}),
        json!({"role": "drive", "policy": {"allow": ["Read"]}}),
        json!({"role": "drive", "policy": {"preset": "all", "deny": []}}),
        json!({"role": "drive", "policy": {}}),
        json!({"role": "owner"}),
        json!({}),
    ];
    for (n, terms) in refused.into_iter().enumerate() {
        let mut params = json!({
            "conversationId": fixture.id.to_string(),
            "requestId": format!("share-{n}"),
            "credentialId": "phone",
        });
        for (key, value) in terms.as_object().unwrap() {
            params[key] = value.clone();
        }
        let answer = fixture.call("conversation.share", params).await;
        assert_eq!(answer["error"]["code"], "invalid_request", "{terms}: {answer}");
    }
    let shares = fixture
        .call(
            "conversation.shares",
            json!({"conversationId": fixture.id.to_string()}),
        )
        .await;
    assert_eq!(shares["payload"]["items"], json!([]), "{shares}");
    let journaled: i64 = nessa_local_database::rusqlite::Connection::open(
        fixture
            ._directory
            .path()
            .join("conversations")
            .join("metadata.sqlite3"),
    )
    .unwrap()
    .query_row("SELECT COUNT(*) FROM share_grant_changes", [], |row| {
        row.get(0)
    })
    .unwrap();
    assert_eq!(journaled, 0, "nothing was journaled");
    // The longest pattern is accepted.
    let answer = fixture
        .call(
            "conversation.share",
            json!({
                "conversationId": fixture.id.to_string(),
                "requestId": "share-longest",
                "credentialId": "phone",
                "role": "drive",
                "policy": {"allow": [longest], "deny": []},
            }),
        )
        .await;
    assert_eq!(answer["payload"]["applied"], true, "{answer}");
}

/// Row J7: `conversation.shares` reports each grant's role, its policy when
/// it drives, and its policy revision, which moves when the owner changes
/// the role or the policy and not otherwise.
#[tokio::test]
async fn shares_reports_each_grants_role_policy_and_policy_revision() {
    let fixture = sharing_fixture().await;
    let share = |request: &str, credential: &str, terms: Value| {
        let mut params = json!({
            "conversationId": fixture.id.to_string(),
            "requestId": request,
            "credentialId": credential,
        });
        for (key, value) in terms.as_object().unwrap() {
            params[key] = value.clone();
        }
        params
    };
    let rules = json!({"allow": ["Read", "Grep"], "deny": ["Bash(rm -rf:*)"]});
    for params in [
        share("phone", "phone", json!({"role": "read"})),
        share("tablet", "tablet", json!({"role": "drive", "policy": rules})),
    ] {
        let answer = fixture.call("conversation.share", params).await;
        assert_eq!(answer["payload"]["applied"], true, "{answer}");
    }
    let listed = || async {
        let shares = fixture
            .call(
                "conversation.shares",
                json!({"conversationId": fixture.id.to_string()}),
            )
            .await;
        let items = shares["payload"]["items"].as_array().unwrap().clone();
        let by = |credential: &str| {
            items
                .iter()
                .find(|item| item["credentialId"] == credential)
                .unwrap()
                .clone()
        };
        (by("phone"), by("tablet"))
    };
    let (phone, tablet) = listed().await;
    assert_eq!(phone["role"], "read");
    assert!(phone.get("policy").is_none(), "{phone}");
    assert_eq!(tablet["role"], "drive");
    assert_eq!(tablet["policy"], rules);
    let first = tablet["policyRevision"].as_u64().unwrap();
    assert!(first > phone["policyRevision"].as_u64().unwrap());
    // The same terms again move nothing; another policy moves the revision.
    let again = fixture
        .call(
            "conversation.share",
            share("tablet-again", "tablet", json!({"role": "drive", "policy": rules})),
        )
        .await;
    assert_eq!(again["payload"]["applied"], false, "{again}");
    assert_eq!(listed().await.1["policyRevision"].as_u64().unwrap(), first);
    let preset = json!({"preset": "read_only_tools"});
    let changed = fixture
        .call(
            "conversation.share",
            share("tablet-preset", "tablet", json!({"role": "drive", "policy": preset})),
        )
        .await;
    assert_eq!(changed["payload"]["applied"], true, "{changed}");
    let (phone_after, tablet) = listed().await;
    assert_eq!(tablet["policy"], preset);
    assert!(tablet["policyRevision"].as_u64().unwrap() > first);
    assert_eq!(phone_after, phone, "another grant's change moves nothing here");
}

/// A retained retirement owner refuses both the wire read and its live follow.
#[tokio::test]
async fn retained_stopping_publication_refuses_wire_and_subscription_until_cleanup() {
    use conversation_support::{MemoryRepository, ModeReadbackFault};
    use nessa_protocol::conversation::domain::ConversationApprovalMode;
    use nessa_sdk::application::agent_execution::agents::AgentError;
    for terminal_mode in [false, true] {
        let expected_mode = if terminal_mode { "auto" } else { "ask" };
        let mut fixture = SubscriptionFixture::new().await;
        grants(
            &fixture.authority,
            &["conversation.write", "conversation.read", "server.read"],
        );
        fixture.session = authenticate(&fixture.state).await;
        let repository = Arc::new(MemoryRepository::default());
        repository
            .create(fixture.metadata.load(&fixture.id).await.unwrap().unwrap())
            .await
            .unwrap();
        fixture.service = conversation_service_with_repository(
            &fixture.storage,
            &fixture.metadata,
            &fixture.provider,
            repository.clone(),
        );
        fixture.state = fixture.state.with_conversations(fixture.service.clone());
        fixture
            .service
            .read(
                fixture.id.clone(),
                caller(&fixture.session, "initial".into()),
            )
            .await
            .unwrap();
        let opens = fixture.provider.open_calls.load(Ordering::SeqCst);
        let mut client = fixture.connect();
        let subscription = client.subscribe("before-retirement", &fixture.id).await;
        let initial = client.next().await;
        assert_eq!(view(&initial)["approvalMode"], "ask");
        *fixture.provider.close_failure.lock().unwrap() = Some(AgentError::CleanupUncertain);
        if terminal_mode {
            repository
                .lose_mode_commit_ack
                .store(true, Ordering::SeqCst);
            *repository.mode_readback_fault.lock().unwrap() = Some(ModeReadbackFault::Refuse);
            assert!(fixture
                .service
                .set_approval_mode(
                    fixture.id.clone(),
                    caller(&fixture.session, "uncertain-mode".into()),
                    ConversationApprovalMode::Auto
                )
                .await
                .is_err());
        } else {
            assert!(fixture
                .service
                .close(
                    fixture.id.clone(),
                    caller(&fixture.session, "uncertain-close".into())
                )
                .await
                .is_err());
        }
        for method in ["conversation.read"] {
            let response = fixture
                .call(method, json!({"conversationId":fixture.id.to_string()}))
                .await;
            assert_eq!(response["ok"], false, "{response}");
            assert_eq!(response["error"]["code"], "conversation_closed");
            assert!(
                response.get("payload").is_none(),
                "no clean replacement on refusal"
            );
        }
        let ended = client
            .until(&subscription, |frame| {
                frame["event"] == "conversation.subscriptionEnded"
            })
            .await;
        assert!(
            ended
                .iter()
                .all(|frame| frame["event"] != "conversation.view"),
            "{ended:?}"
        );
        let end = ended.last().unwrap();
        assert_eq!(end["payload"]["reason"], "refused");
        assert_eq!(end["payload"]["code"], "conversation_closed");
        client.send(
            "blocked-new-follow",
            "conversation.subscribe",
            json!({"conversationId":fixture.id.to_string()}),
        );
        let (reply, _) = client.reply("blocked-new-follow").await;
        assert_eq!(reply["error"]["code"], "conversation_closed");
        assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), opens);
        *fixture.provider.close_failure.lock().unwrap() = None;
        fixture
            .service
            .close(
                fixture.id.clone(),
                caller(&fixture.session, "confirmed-close".into()),
            )
            .await
            .unwrap();
        let after = fixture
            .call(
                "conversation.read",
                json!({"conversationId":fixture.id.to_string()}),
            )
            .await;
        assert_eq!(after["ok"], true, "{after}");
        assert_eq!(after["payload"]["approvalMode"], expected_mode);
        client.subscribe("after-cleanup", &fixture.id).await;
        assert_eq!(view(&client.next().await)["approvalMode"], expected_mode);
        client.close().await;
        fixture.service.shutdown().await.unwrap();
    }
}

/// A gateway whose paired environments the test connects and disconnects,
/// standing in for `PeerEnvironments`: the names it says are connected, and
/// its count of changes, moved after each change as a connection admitted,
/// ended or revoked moves it. Composed beside a conversation service, so
/// the conversation catalogue's head and list can be watched not to move.
struct CatalogFixture {
    inner: SubscriptionFixture,
    connected: Arc<Mutex<Vec<String>>>,
    changes: tokio::sync::watch::Sender<u64>,
}

impl CatalogFixture {
    async fn new() -> Self {
        let mut inner = SubscriptionFixture::new().await;
        let connected = Arc::new(Mutex::new(Vec::new()));
        let changes = tokio::sync::watch::Sender::new(0);
        let names = connected.clone();
        inner.state = inner
            .state
            .with_connected_environments(crate::product::ConnectedEnvironments {
                names: Arc::new(move || names.lock().unwrap().clone()),
                changes: changes.subscribe(),
            });
        Self {
            inner,
            connected,
            changes,
        }
    }

    /// `peer` connects: the list says so, then the count moves.
    fn connects(&self, peer: &str) {
        self.connected.lock().unwrap().push(peer.to_owned());
        self.moved();
    }

    /// `peer`'s connection ends, or is revoked: the list no longer names
    /// it, then the count moves.
    fn goes(&self, peer: &str) {
        self.connected.lock().unwrap().retain(|name| name != peer);
        self.moved();
    }

    fn moved(&self) {
        self.changes.send_modify(|count| *count += 1);
    }

    fn connect(&self) -> Client {
        self.inner.connect()
    }

    /// Subscribe to the agent catalog; its id and its first frame.
    async fn subscribe(&self, client: &mut Client, request: &str) -> (String, Value) {
        client.send(request, "agents.subscribe", json!({}));
        let (reply, before) = client.reply(request).await;
        assert_eq!(reply["ok"], true, "{reply}");
        let id = reply["payload"]["subscriptionId"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(
            before
                .iter()
                .all(|frame| frame["payload"]["subscriptionId"] != id.as_str()),
            "nothing of it before the reply: {before:?}"
        );
        let first = client.until(&id, |_| true).await.remove(0);
        (id, first)
    }
}

/// The places a catalog frame names.
fn places(frame: &Value) -> Vec<String> {
    assert_eq!(frame["event"], "agents.listed", "{frame}");
    serde_json::from_value(frame["payload"]["catalog"]["environments"].clone()).unwrap()
}

fn peer(byte: &str) -> String {
    format!("peer:{}", byte.repeat(32))
}

/// Rows AC1 and AC2 (`docs/design/record-subscriptions.md`), I10
/// (`docs/design/auth/peer-gateways.md`): the catalog is sent after the
/// reply, then again each time a paired environment connects, disconnects
/// or is revoked, the same catalog `agents.list` answers then; a change that
/// leaves it as it was sends nothing.
#[tokio::test]
async fn the_agent_catalog_is_sent_again_as_paired_environments_come_and_go() {
    let fixture = CatalogFixture::new().await;
    let mut client = fixture.connect();
    let (id, first) = fixture.subscribe(&mut client, "agents").await;
    assert_eq!(places(&first), ["build.example"]);
    let read = fixture.inner.call("agents.list", json!({})).await;
    assert_eq!(read["payload"], first["payload"]["catalog"]);

    let (e, f) = (peer("ab"), peer("cd"));
    fixture.connects(&e);
    let connected = client.until(&id, |_| true).await.remove(0);
    assert_eq!(places(&connected), ["build.example", e.as_str()]);
    fixture.connects(&f);
    let both = client.until(&id, |_| true).await.remove(0);
    assert_eq!(places(&both), ["build.example", e.as_str(), f.as_str()]);
    let read = fixture.inner.call("agents.list", json!({})).await;
    assert_eq!(read["payload"], both["payload"]["catalog"]);

    // A connection that takes the place of another moves the count with
    // the list as it was.
    fixture.moved();
    client.quiet(&id).await;

    fixture.goes(&e);
    let disconnected = client.until(&id, |_| true).await.remove(0);
    assert_eq!(places(&disconnected), ["build.example", f.as_str()]);
    fixture.goes(&f);
    let revoked = client.until(&id, |_| true).await.remove(0);
    assert_eq!(places(&revoked), ["build.example"]);
    client.close().await;
}

/// Row AC1 on a gateway that pairs no environments: nothing changes its
/// catalog after startup, so the first frame is the only one, and the
/// subscription stays open until stopped.
#[tokio::test]
async fn a_gateway_pairing_no_environments_sends_its_catalog_once() {
    let fixture = SubscriptionFixture::new().await;
    let mut client = fixture.connect();
    client.send("agents", "agents.subscribe", json!({}));
    let (reply, _) = client.reply("agents").await;
    let id = reply["payload"]["subscriptionId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(places(&client.until(&id, |_| true).await[0]), ["build.example"]);
    client.quiet(&id).await;
    client.send(
        "stop",
        "agents.unsubscribe",
        json!({"subscriptionId": id}),
    );
    assert_eq!(client.reply("stop").await.0["ok"], true);
    client.close().await;
}

/// Row AC3: changes while a frame waits for the writer are read once, after
/// it is written, as the catalog stands then: at most one more frame, never
/// one per change.
#[tokio::test]
async fn changes_while_a_catalog_frame_waits_collapse_into_one_frame() {
    let fixture = CatalogFixture::new().await;
    let mut client = fixture.connect();
    let (id, _) = fixture.subscribe(&mut client, "agents").await;
    fixture.inner.stall(&mut client).await;
    let names: Vec<String> = ["01", "02", "03", "04", "05", "06"]
        .into_iter()
        .map(peer)
        .collect();
    for name in &names {
        fixture.connects(name);
    }
    client.gate.open();
    let mut expected = vec!["build.example".to_owned()];
    expected.extend(names.iter().cloned());
    let frames = client.until(&id, |frame| places(frame) == expected).await;
    assert!(frames.len() <= 2, "{frames:?}");
    client.quiet(&id).await;
    client.close().await;
}

/// Row AC4 (row H9's fact kept apart): an environment connecting and going
/// moves neither the conversation catalogue's head nor its list.
#[tokio::test]
async fn paired_environments_coming_and_going_never_move_the_conversation_catalogue() {
    let fixture = CatalogFixture::new().await;
    let owner = caller(&fixture.inner.session, String::new());
    let head = || async {
        use crate::conversation::application::ConversationCatalogue;
        fixture
            .inner
            .metadata
            .head(
                &owner.organization_id,
                &owner.principal_id,
                &crate::conversation::application::Reader::Owner,
            )
            .await
            .unwrap()
            .revision
    };
    let before = head().await;
    let mut client = fixture.connect();
    client.send("list", "conversation.subscribeList", json!({}));
    let (reply, _) = client.reply("list").await;
    let list = reply["payload"]["subscriptionId"]
        .as_str()
        .unwrap()
        .to_owned();
    client.until(&list, |_| true).await;
    let (agents, _) = fixture.subscribe(&mut client, "agents").await;
    let e = peer("ab");
    fixture.connects(&e);
    client.until(&agents, |frame| places(frame).contains(&e)).await;
    fixture.goes(&e);
    client.until(&agents, |frame| !places(frame).contains(&e)).await;
    client.quiet(&list).await;
    assert_eq!(head().await, before);
    client.close().await;
}

/// Row AC5: the agent catalog is one target, so a connection follows it
/// once; what the subscription held goes with its socket.
#[tokio::test]
async fn a_second_agent_catalog_subscription_is_a_duplicate() {
    let fixture = CatalogFixture::new().await;
    // What the gateway's own state holds; the socket's copies of that state
    // and the subscription's wake source are counted on top while it is open.
    let held = fixture.changes.receiver_count();
    let mut client = fixture.connect();
    fixture.subscribe(&mut client, "agents").await;
    client.send("again", "agents.subscribe", json!({}));
    let (reply, _) = client.reply("again").await;
    assert_eq!(reply["error"]["code"], "subscription_duplicate", "{reply}");
    client.close().await;
    timeout(EXPECTED, async {
        while fixture.changes.receiver_count() != held {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the subscription's wake source goes with its socket");
}

/// Row AC6: following the agent catalog asks what `agents.list` asks, the
/// gateway's read grant, and nothing a paired reader's credential holds; a
/// grant lost while it is followed ends it before its next read.
#[tokio::test]
async fn following_the_agent_catalog_is_admitted_as_reading_it_is() {
    let fixture = CatalogFixture::new().await;
    for (actions, allowed) in [
        (&["server.read"][..], true),
        (&["conversation.write"][..], false),
        // What a paired reader's credential holds.
        (&["conversation.read"][..], false),
    ] {
        grants(&fixture.inner.authority, actions);
        let read = fixture.inner.call("agents.list", json!({})).await;
        let mut client = fixture.connect();
        client.send("agents", "agents.subscribe", json!({}));
        let (reply, _) = client.reply("agents").await;
        assert_eq!(read["ok"], allowed, "{actions:?} {read}");
        assert_eq!(reply["ok"], allowed, "{actions:?} {reply}");
        if !allowed {
            assert_eq!(read["error"]["code"], "forbidden");
            assert_eq!(reply["error"]["code"], "forbidden");
        }
        client.close().await;
    }
    grants(&fixture.inner.authority, &["server.read"]);
    let mut client = fixture.connect();
    let (id, _) = fixture.subscribe(&mut client, "agents").await;
    grants(&fixture.inner.authority, &["conversation.write"]);
    fixture.connects(&peer("ab"));
    let ended = client.until(&id, |_| true).await.remove(0);
    assert_eq!(ended["event"], "conversation.subscriptionEnded", "{ended}");
    assert_eq!(ended["payload"]["reason"], "refused");
    assert_eq!(ended["payload"]["code"], "forbidden");
    client.close().await;
}

/// Row AC7: `agents.unsubscribe` stops the catalog's subscription, asking
/// the same grant, and nothing of it follows the reply; each unsubscribe
/// stops only what the subscribe beside it made.
#[tokio::test]
async fn each_unsubscribe_stops_only_its_own_kind() {
    let fixture = CatalogFixture::new().await;
    let mut client = fixture.connect();
    let (agents, _) = fixture.subscribe(&mut client, "agents").await;
    client.send("list", "conversation.subscribeList", json!({}));
    let (reply, _) = client.reply("list").await;
    let list = reply["payload"]["subscriptionId"]
        .as_str()
        .unwrap()
        .to_owned();
    for (method, id) in [
        ("conversation.unsubscribe", &agents),
        ("agents.unsubscribe", &list),
    ] {
        client.send("wrong", method, json!({"subscriptionId": id}));
        let (reply, _) = client.reply("wrong").await;
        assert_eq!(reply["error"]["code"], "unknown_subscription", "{method}");
    }
    // A credential that may read the catalog and write nothing stops it.
    grants(&fixture.inner.authority, &["server.read"]);
    client.send(
        "stop",
        "agents.unsubscribe",
        json!({"subscriptionId": agents}),
    );
    let (reply, _) = client.reply("stop").await;
    assert_eq!(reply["payload"]["subscriptionId"], agents.as_str(), "{reply}");
    fixture.connects(&peer("ab"));
    client.quiet(&agents).await;
    client.close().await;
}

/// Row AC8: the paired environments' change source closing ends the
/// subscription `source_closed`.
#[tokio::test]
async fn the_environments_source_closing_ends_the_catalog_subscription() {
    let fixture = CatalogFixture::new().await;
    let mut client = fixture.connect();
    let (id, _) = fixture.subscribe(&mut client, "agents").await;
    let CatalogFixture { inner, changes, .. } = fixture;
    drop(changes);
    let ended = client.until(&id, |_| true).await.remove(0);
    assert_eq!(ended["event"], "conversation.subscriptionEnded", "{ended}");
    assert_eq!(ended["payload"]["reason"], "source_closed");
    client.close().await;
    drop(inner);
}

/// Row AC1: a catalog frame is the generated `AgentsListed`.
#[tokio::test]
async fn an_agent_catalog_frame_is_the_generated_shape() {
    let fixture = CatalogFixture::new().await;
    let mut client = fixture.connect();
    let (id, first) = fixture.subscribe(&mut client, "agents").await;
    let frame: AgentsListed = serde_json::from_value(first["payload"].clone()).unwrap();
    assert_eq!(frame.subscription_id, id);
    assert_eq!(frame.catalog.environments, ["build.example"]);
    client.close().await;
}

/// Row AC9: a native hooks command sends the catalog again, with the choice
/// `agents.list` answers then.
#[tokio::test]
async fn the_agent_catalog_is_sent_again_when_the_hooks_choice_changes() {
    let mut fixture = CatalogFixture::new().await;
    let hooks_directory = tempfile::tempdir().unwrap();
    let hooks = crate::agents::infrastructure::NativeHooksPreferences::open(
        hooks_directory.path().join("hooks"),
        Arc::new(TestClock),
    )
    .unwrap();
    fixture.inner.state = fixture.inner.state.clone().with_native_hooks(hooks.clone());
    let mut client = fixture.connect();
    let (id, first) = fixture.subscribe(&mut client, "agents").await;
    assert_eq!(first["payload"]["catalog"]["nativeHooks"]["claude"], "off");
    hooks
        .set(crate::agents::application::NativeHooksChange {
            agent: crate::agents::application::NativeHooksAgent::Claude,
            enabled: true,
            request_id: "request-on".into(),
            organization_id: "organization".into(),
            principal_id: "principal".into(),
            surface_id: "surface".into(),
            caller_kind: "user".into(),
        })
        .await
        .unwrap();
    let changed = client.until(&id, |_| true).await.remove(0);
    assert_eq!(changed["payload"]["catalog"]["nativeHooks"]["claude"], "on");
    let read = fixture.inner.call("agents.list", json!({})).await;
    assert_eq!(read["payload"], changed["payload"]["catalog"]);
    client.close().await;
}
