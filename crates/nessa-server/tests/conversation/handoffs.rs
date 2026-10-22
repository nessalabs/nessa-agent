//! Switching a conversation to another agent and forking one, through the real
//! metadata store and record storage (issue #1080,
//! `docs/design/conversation-handoffs.md`).
use super::*;
use crate::conversation::application::{
    ConversationAgentSwitch, ConversationAgentSwitchState, FailureBackoff, ProviderSessionErasers,
    RequestedConversation, SubmittedMessage, UnfinishedTurns,
};
use crate::conversation::infrastructure::LocalConversationStore;
use crate::conversation_test_support::{
    AcceptingCreationAudit, AcceptingDeletionAudit, AcceptingModeAudit, Provider, ProviderFactory,
    RecordingFileLinkAudit, TestClock, DELETION_BUDGETS,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::view::{ConversationHandoffKind, ConversationMessageStatus};
use nessa_sdk::application::agent_execution::commands::CreationFailure;
use nessa_sdk::application::agent_execution::sessions::HandoffKind;
use nessa_sdk::infrastructure::session_storage::{RecordStorage, RuntimeMessageCommitClock};

const BOUND: Duration = Duration::from_secs(10);
const CODEX_MODEL: &str = "codex-model";

struct Fixture {
    service: ConversationService,
    claude: Arc<ProviderFactory>,
    codex: Arc<ProviderFactory>,
    storage: Arc<RecordStorage>,
    metadata: Arc<LocalConversationStore>,
    _directory: tempfile::TempDir,
}

fn caller(action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
        action_id: action.into(),
    }
}
fn new_id() -> ConversationId {
    ConversationId::new(&Uuid::new_v4().to_string()).unwrap()
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = &directory.path().join("gateway");
        nessa_local_storage::create_directory(root).unwrap();
        let storage = Arc::new(RecordStorage::new(root.join("records")).unwrap());
        let metadata = Arc::new(
            LocalConversationStore::open(&root.join("metadata.sqlite3"))
                .unwrap()
                .with_access_policy(crate::conversation_test_support::access_policy()),
        );
        let claude = Arc::new(ProviderFactory::default());
        let codex = Arc::new(ProviderFactory::default());
        let agent = |provider: Arc<
            dyn nessa_sdk::application::agent_execution::providers::AgentProvider,
        >| ConversationAgent {
            provider,
            execution_audit: Arc::new(crate::conversation_test_support::AcceptingAudit),
            reserved_output_tokens: 4096,
            readiness: None,
            sandbox: nessa_sdk::domain::agent_execution::leases::SandboxProfiles::HARNESS_DEFAULT,
        };
        let agents = ConversationAgents::new(
            HashMap::from([
                (
                    AgentId::Claude,
                    agent(Arc::new(Provider::new(claude.clone()))),
                ),
                (
                    AgentId::Codex,
                    agent(Arc::new(
                        Provider::new(codex.clone())
                            .with_model(CODEX_MODEL)
                            .with_name("codex-acp"),
                    )),
                ),
            ]),
            AgentId::Claude,
        )
        .unwrap();
        let service = ConversationService::new(
            ConversationDependencies {
                access_policy: crate::conversation_test_support::access_policy(),
                agents,
                storage: storage.clone(),
                metadata: metadata.clone(),
                mode_audit: Arc::new(AcceptingModeAudit),
                creation_audit: Arc::new(AcceptingCreationAudit),
                file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
                attachments: None,
                summaries: metadata.clone(),
                listing: metadata.clone(),
                deletion_audit: Arc::new(AcceptingDeletionAudit),
                provider_sessions: ProviderSessionErasers::default(),
                deletion_budgets: DELETION_BUDGETS,
                message_commit_clock: Arc::new(RuntimeMessageCommitClock::new()),
                clock: Arc::new(TestClock),
                environment: crate::conversation::infrastructure::in_process_environment().into(),
            },
            ConversationLimits::default(),
            None,
        )
        .unwrap();
        service.bind_commands(storage.clone());
        Self {
            service,
            claude,
            codex,
            storage,
            metadata,
            _directory: directory,
        }
    }

    async fn create(&self) -> ConversationId {
        let id = new_id();
        self.service
            .create(
                id.clone(),
                caller("create"),
                RequestedConversation::default(),
            )
            .await
            .unwrap();
        id
    }

    /// Send `action` to `id` and wait until its turn completed.
    async fn turn(&self, id: &ConversationId, action: &str) {
        tokio::time::timeout(
            BOUND,
            self.service.submit(
                id.clone(),
                caller(action),
                action.to_owned(),
                SubmittedMessage {
                    text: format!("said {action}"),
                    ..SubmittedMessage::default()
                },
                SubmissionMode::Queue,
            ),
        )
        .await
        .expect("the message is answered")
        .unwrap();
        tokio::time::timeout(BOUND, async {
            loop {
                let view = self.service.read(id.clone(), caller("read")).await.unwrap();
                if view.messages.iter().any(|message| {
                    message.execution_id == action
                        && message.status == ConversationMessageStatus::Completed
                }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let slot = self
                .service
                .inner
                .conversations
                .lock()
                .await
                .get(id)
                .cloned()
                .unwrap();
            let live = self.service.wait_for_slot(id, slot).await.unwrap();
            while !live.agent.is_idle().await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{action} completes"));
    }

    async fn switch(&self, id: &ConversationId, action: &str) -> Result<bool, ConversationError> {
        tokio::time::timeout(
            BOUND,
            self.service.switch_agent(
                id.clone(),
                caller(action),
                AgentId::Codex,
                ConversationModelId::new(CODEX_MODEL).unwrap(),
            ),
        )
        .await
        .expect("the switch is answered")
    }

    async fn record(&self, id: &ConversationId) -> crate::conversation::domain::Conversation {
        load_conversation(self.metadata.as_ref(), id)
            .await
            .unwrap()
            .unwrap()
    }

    async fn snapshot(&self, id: &ConversationId) -> SessionSnapshot {
        self.storage
            .read_committed(conversation_session(id))
            .await
            .unwrap()
            .unwrap()
            .snapshot()
            .cloned()
            .unwrap()
    }

    /// Leave a switch journaled and not settled, as a gateway that stopped
    /// in its middle does: the agent stopped, the journal pending.
    async fn interrupt_switch(&self, id: &ConversationId) {
        let slot = self
            .service
            .inner
            .conversations
            .lock()
            .await
            .get(id)
            .cloned()
            .unwrap();
        let actor = caller("stop").actor().unwrap();
        self.service
            .stop_and_release(
                id,
                slot,
                &actor,
                &McpAppInitiator::System,
                LeaseEndCause::Stopped,
                AgentEnd::Suspend,
            )
            .await
            .unwrap();
        self.metadata
            .begin_agent_switch(ConversationAgentSwitch {
                conversation_id: id.clone(),
                organization_id: OrganizationId::new("org").unwrap(),
                request_id: "interrupted".into(),
                initiator_principal_id: PrincipalId::new("person").unwrap(),
                initiator_surface_id: "panel".into(),
                prior_agent: AgentId::Claude,
                prior_model: ConversationModelId::new("test").unwrap(),
                requested_agent: AgentId::Codex,
                requested_model: ConversationModelId::new(CODEX_MODEL).unwrap(),
                state: ConversationAgentSwitchState::Pending,
                requested_at_ms: 5,
            })
            .await
            .unwrap();
    }

    async fn retire(self) {
        self.service.shutdown().await.unwrap();
        drop(self.service);
        self.storage.shutdown().await.unwrap();
    }
}

/// Rows W1, W4: a switch stops the agent it leaves, opens a fresh session
/// of the new one, commits its agent and model, and shows one switch line;
/// the next turn on the new agent is given the turns before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_carries_the_conversation_on_under_another_agent() {
    let fixture = Fixture::new();
    let id = fixture.create().await;
    fixture.turn(&id, "first").await;

    assert!(matches!(fixture.switch(&id, "switch").await, Ok(true)));
    let record = fixture.record(&id).await;
    assert_eq!(record.agent(), Some(AgentId::Codex));
    assert_eq!(record.model().as_str(), CODEX_MODEL);
    let opens = fixture.codex.provider_opens.lock().unwrap().clone();
    assert_eq!(opens.len(), 1);
    assert_eq!(opens[0].1, None, "a fresh native session");
    let snapshot = fixture.snapshot(&id).await;
    assert_eq!(snapshot.provider.name(), "codex-acp");
    let handoff = snapshot.handoff("person", "switch").expect("the switch");
    assert!(matches!(handoff.kind, HandoffKind::Switch { .. }));

    let view = fixture
        .service
        .read(id.clone(), caller("read"))
        .await
        .unwrap();
    assert_eq!(view.handoffs.len(), 1);
    assert_eq!(view.handoffs[0].kind, ConversationHandoffKind::Switch);
    assert_eq!(
        view.handoffs[0].after_execution_id.as_deref(),
        Some("first")
    );
    let to = view.handoffs[0].to.as_ref().unwrap();
    assert_eq!(
        (to.agent.as_deref(), to.model.as_str()),
        (Some("codex"), CODEX_MODEL)
    );

    fixture.turn(&id, "second").await;
    let given = fixture
        .codex
        .messages
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    let history = given.history().expect("the turns before the switch");
    assert_eq!(history.turns().len(), 1);
    assert_eq!(history.turns()[0].said(), Some("said first"));
    fixture.turn(&id, "third").await;
    let given = fixture
        .codex
        .messages
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert!(given.history().is_none(), "only the first turn after it");
    // The agent left was given nothing after the switch.
    assert_eq!(fixture.claude.messages.lock().unwrap().len(), 1);

    // The same request answers as it did, opening nothing more.
    assert!(matches!(fixture.switch(&id, "switch").await, Ok(true)));
    assert_eq!(fixture.codex.provider_opens.lock().unwrap().len(), 1);
    fixture.retire().await;
}

/// Row W2: the agent the conversation runs on is refused, and so is an
/// agent this gateway does not run; a turn at work keeps its agent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_is_refused_to_the_same_agent_and_while_a_turn_runs() {
    let fixture = Fixture::new();
    let id = fixture.create().await;
    fixture.turn(&id, "first").await;
    assert!(matches!(
        fixture
            .service
            .switch_agent(
                id.clone(),
                caller("same"),
                AgentId::Claude,
                ConversationModelId::new("test").unwrap(),
            )
            .await,
        Err(ConversationError::InvalidInput)
    ));
    assert!(matches!(
        fixture
            .service
            .switch_agent(
                id.clone(),
                caller("unknown model"),
                AgentId::Codex,
                ConversationModelId::new("missing").unwrap(),
            )
            .await,
        Err(ConversationError::ModelUnavailable)
    ));

    let (release, gate) = tokio::sync::oneshot::channel();
    *fixture.claude.execution_gate.lock().unwrap() = Some(gate);
    let service = fixture.service.clone();
    let running = id.clone();
    let sent = tokio::spawn(async move {
        service
            .submit(
                running,
                caller("running"),
                "running".into(),
                SubmittedMessage {
                    text: "hold on".into(),
                    ..SubmittedMessage::default()
                },
                SubmissionMode::Queue,
            )
            .await
    });
    // The first turn left a notification behind, so wait for this one's
    // prompt to reach the agent rather than for any notification.
    tokio::time::timeout(BOUND, async {
        while !fixture
            .claude
            .executions
            .lock()
            .unwrap()
            .iter()
            .any(|execution| execution == "running")
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let busy = fixture.switch(&id, "busy").await;
    assert!(
        matches!(busy, Err(ConversationError::TurnRunning)),
        "{busy:?}"
    );
    assert_eq!(fixture.record(&id).await.agent(), Some(AgentId::Claude));
    release.send(()).unwrap();
    sent.await.unwrap().unwrap();
    fixture.retire().await;
}

/// Row W3: a switch the gateway stopped in the middle of, journaled and not
/// settled, is carried on by the next opening, which settles it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pending_switch_is_carried_on_by_the_next_opening() {
    let fixture = Fixture::new();
    let id = fixture.create().await;
    fixture.turn(&id, "first").await;
    fixture.interrupt_switch(&id).await;

    fixture.turn(&id, "second").await;
    let settled = fixture
        .metadata
        .agent_switch(&id, "interrupted")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled.state, ConversationAgentSwitchState::Applied);
    assert_eq!(fixture.record(&id).await.agent(), Some(AgentId::Codex));
    let given = fixture
        .codex
        .messages
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert_eq!(given.history().unwrap().turns().len(), 1);
    fixture.retire().await;
}

/// Rows F1–F3: a fork is a new conversation on the source's agent and
/// model, listed as the source is, whose transcript starts with the copied
/// turns and whose first turn is given them; the source is unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_starts_a_new_conversation_from_the_sources_turns() {
    let fixture = Fixture::new();
    let source = fixture.create().await;
    fixture.turn(&source, "first").await;
    fixture.turn(&source, "second").await;

    let fork = new_id();
    let store = fixture.service.commands().unwrap();
    fixture
        .service
        .fork(store.clone(), fork.clone(), caller("fork"), source.clone())
        .await
        .unwrap();
    let record = fixture.record(&fork).await;
    assert_eq!(record.agent(), Some(AgentId::Claude));
    assert_eq!(record.model().as_str(), "test");
    let view = fixture
        .service
        .read(fork.clone(), caller("read"))
        .await
        .unwrap();
    assert_eq!(view.handoffs.len(), 1);
    assert_eq!(view.handoffs[0].kind, ConversationHandoffKind::Fork);
    let said: Vec<_> = view.handoffs[0]
        .turns
        .iter()
        .map(|turn| turn.said.clone().unwrap())
        .collect();
    assert_eq!(said, vec!["said first", "said second"]);
    assert!(view.messages.is_empty());
    assert_eq!(
        view.title.as_deref(),
        fixture.service.title(&source).await.as_deref()
    );

    fixture.turn(&fork, "third").await;
    let given = fixture
        .claude
        .messages
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert_eq!(given.history().unwrap().turns().len(), 2);
    assert!(fixture.snapshot(&source).await.handoffs.is_empty());

    // A retry of the same fork answers as the first did, copying nothing again.
    fixture
        .service
        .fork(store.clone(), fork.clone(), caller("fork"), source.clone())
        .await
        .unwrap();
    assert_eq!(fixture.snapshot(&fork).await.handoffs.len(), 1);
    // A conversation is not its own fork.
    assert!(matches!(
        fixture
            .service
            .fork(store, source.clone(), caller("self"), source.clone())
            .await,
        Err(CreationFailure::Target(ConversationError::InvalidInput))
    ));
    fixture.retire().await;
}

/// Row W4: another request while a switch is pending carries the pending one
/// on first, and is itself refused, so it never starts from a conversation
/// half way between two agents.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_switch_while_one_is_pending_carries_that_one_on_and_is_refused() {
    let fixture = Fixture::new();
    let id = fixture.create().await;
    fixture.turn(&id, "first").await;
    fixture.interrupt_switch(&id).await;

    assert!(matches!(
        fixture.switch(&id, "another").await,
        Err(ConversationError::RequestConflict)
    ));
    let settled = fixture
        .metadata
        .agent_switch(&id, "interrupted")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled.state, ConversationAgentSwitchState::Applied);
    assert!(fixture
        .metadata
        .agent_switch(&id, "another")
        .await
        .unwrap()
        .is_none());
    assert_eq!(fixture.record(&id).await.agent(), Some(AgentId::Codex));
    fixture.retire().await;
}

/// Row W5: a new agent that cannot open leaves no handoff in the history, so
/// the switch is not made: the conversation stays on its agent, and its next
/// turn runs there, given nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_whose_agent_cannot_open_is_not_made() {
    let fixture = Fixture::new();
    let id = fixture.create().await;
    fixture.turn(&id, "first").await;
    *fixture.codex.open_failure.lock().unwrap() =
        Some(AgentError::Protocol("will not open".into()));

    assert!(matches!(fixture.switch(&id, "switch").await, Ok(false)));
    let settled = fixture
        .metadata
        .agent_switch(&id, "switch")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled.state, ConversationAgentSwitchState::NotApplied);
    let record = fixture.record(&id).await;
    assert_eq!(record.agent(), Some(AgentId::Claude));
    assert_eq!(record.model().as_str(), "test");
    assert!(fixture.snapshot(&id).await.handoffs.is_empty());
    // The same request answers as it settled, trying nothing again.
    assert!(matches!(fixture.switch(&id, "switch").await, Ok(false)));

    fixture.turn(&id, "second").await;
    let given = fixture
        .claude
        .messages
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert!(given.history().is_none());
    assert!(fixture.codex.messages.lock().unwrap().is_empty());
    fixture.retire().await;
}

/// Row F3, and a target that already has turns: neither is created as a
/// fork, and nothing is copied into a conversation that said something.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_of_a_source_not_its_own_or_into_a_conversation_with_turns_is_refused() {
    let fixture = Fixture::new();
    let source = fixture.create().await;
    fixture.turn(&source, "first").await;
    let store = fixture.service.commands().unwrap();

    let stranger = ConversationCaller {
        principal_id: PrincipalId::new("stranger").unwrap(),
        ..caller("fork")
    };
    let fork = new_id();
    assert!(fixture
        .service
        .fork(store.clone(), fork.clone(), stranger, source.clone())
        .await
        .is_err());
    assert!(load_conversation(fixture.metadata.as_ref(), &fork)
        .await
        .unwrap()
        .is_none());

    let target = fixture.create().await;
    fixture.turn(&target, "own").await;
    let refused = fixture
        .service
        .fork(store, target.clone(), caller("fork"), source.clone())
        .await;
    assert!(
        matches!(refused, Err(CreationFailure::Conflict)),
        "{refused:?}"
    );
    assert!(fixture.snapshot(&target).await.handoffs.is_empty());
    assert!(fixture.service.inner.forks.lock().unwrap().is_empty());
    fixture.retire().await;
}

/// Row W2, owed turns: a turn a restart left owed is carried on by the agent
/// it was given to, so a switch waits for it. The durable mark says so, read
/// before resumption has caught up with it after a start.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_is_refused_while_a_restart_left_a_turn_owed() {
    let fixture = Fixture::new();
    let id = fixture.create().await;
    fixture.turn(&id, "first").await;
    let marks: Arc<dyn UnfinishedTurns> = fixture.metadata.clone();
    fixture.service.bind_unfinished_turns(
        marks.clone(),
        FailureBackoff {
            first: Duration::ZERO,
            most: Duration::ZERO,
            attempts: 8,
        },
    );
    marks.mark(&id).await.unwrap();

    let owed = fixture.switch(&id, "owed").await;
    assert!(
        matches!(owed, Err(ConversationError::TurnRunning)),
        "{owed:?}"
    );
    assert!(fixture
        .metadata
        .agent_switch(&id, "owed")
        .await
        .unwrap()
        .is_none());
    assert_eq!(fixture.record(&id).await.agent(), Some(AgentId::Claude));
    marks.clear(&id).await.unwrap();
    assert!(matches!(fixture.switch(&id, "cleared").await, Ok(true)));
    fixture.retire().await;
}

/// Row F2, a retry while the first still runs: it is answered busy, as a
/// request still running is, never as a conflict and never by a creation of
/// its own; the first keeps its seed, so the fork is saved once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_retried_while_the_first_runs_is_saved_once() {
    let fixture = Fixture::new();
    let source = fixture.create().await;
    fixture.turn(&source, "first").await;
    let store = fixture.service.commands().unwrap();
    let fork = new_id();

    let (release, gate) = tokio::sync::oneshot::channel();
    *fixture.claude.open_gate.lock().unwrap() = Some(gate);
    let opened = fixture.claude.open_calls.load(Ordering::SeqCst);
    let first = tokio::spawn({
        let service = fixture.service.clone();
        let (store, fork, source) = (store.clone(), fork.clone(), source.clone());
        async move { service.fork(store, fork, caller("fork"), source).await }
    });
    // The fork's own opening, not a notification an earlier one left.
    tokio::time::timeout(BOUND, async {
        while fixture.claude.open_calls.load(Ordering::SeqCst) == opened {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let retried = fixture
        .service
        .fork(store.clone(), fork.clone(), caller("fork"), source.clone())
        .await;
    assert!(
        matches!(
            retried,
            Err(CreationFailure::Storage(
                nessa_sdk::application::agent_execution::commands::CreationStorageError::Storage(
                    nessa_sdk::application::agent_execution::sessions::StorageError::Busy
                )
            ))
        ),
        "{retried:?}"
    );
    release.send(()).unwrap();
    tokio::time::timeout(BOUND, first)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let handoffs = fixture.snapshot(&fork).await.handoffs;
    assert_eq!(handoffs.len(), 1);
    assert!(matches!(handoffs[0].kind, HandoffKind::Fork { .. }));
    fixture.retire().await;
}
