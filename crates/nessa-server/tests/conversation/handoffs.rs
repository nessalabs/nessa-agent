//! Switching a conversation to another agent and forking one, through the real
//! metadata store and record storage (issue #1080,
//! `docs/design/conversation-handoffs.md`).
use super::*;
use crate::conversation::application::{
    ConversationAgentSwitch, ConversationAgentSwitchState, FailureBackoff, HistoryGeneration,
    PlanToken, ProviderSessionErasers, RequestedConversation, ScheduleActor, ScheduleSettlement,
    ScheduledMessageStore, SelectionChange, SelectionRequest, SubmittedMessage, UnfinishedTurns,
};
use crate::conversation::infrastructure::{LocalConversationStore, LocalScheduledMessages};
use crate::conversation_test_support::{
    AcceptingCreationAudit, AcceptingDeletionAudit, AcceptingModeAudit, Provider, ProviderFactory,
    RecordingFileLinkAudit, TestClock, DELETION_BUDGETS,
};
use nessa_auth::application::ports::Clock;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_local_database::rusqlite::Connection;
use nessa_protocol::conversation::view::{ConversationHandoffKind, ConversationMessageStatus};
use nessa_sdk::application::agent_execution::agents::{
    UsageLimit, UsageLimitKind, UsageLimitProvider,
};
use nessa_sdk::application::agent_execution::commands::{
    CreationFailure, CreationStorage, MutationOutcome,
};
use nessa_sdk::application::agent_execution::providers::{
    ExecutionReport, ObservationFailure, ObservationFailureCause, ProviderExecutionReply,
    ProviderSessionState,
};
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
        Self::in_directory(tempfile::tempdir().unwrap())
    }

    fn in_directory(directory: tempfile::TempDir) -> Self {
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
            workspace: None,
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
                input_files: None,
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
                    uploads: Vec::new(),
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

    async fn schedules(&self, enabled: bool) -> Arc<LocalScheduledMessages> {
        let schedule = Arc::new(
            LocalScheduledMessages::open(&self._directory.path().join("gateway/scheduled.sqlite3"))
                .unwrap(),
        );
        schedule
            .set_preference(enabled, ScheduleActor::system(), 1)
            .await
            .unwrap();
        self.service.bind_scheduled_messages(schedule.clone());
        schedule
    }

    async fn blocked(&self, id: &ConversationId, action: &str, agent: AgentId) {
        let provider = match agent {
            AgentId::Claude => &self.claude,
            AgentId::Codex => &self.codex,
            _ => unreachable!(),
        };
        let kind = match agent {
            AgentId::Claude => UsageLimitProvider::Claude,
            AgentId::Codex => UsageLimitProvider::Codex,
            _ => unreachable!(),
        };
        let error = AgentError::UsageLimitReached(UsageLimit::timed_reset(
            kind,
            UsageLimitKind::FiveHour,
            TestClock.unix_milliseconds() + 60_000,
            None,
        ));
        *provider.execution_reply.lock().unwrap() = Some(ProviderExecutionReply::Finished(
            ExecutionReport::new(Some(Err(error.clone())), None, ProviderSessionState::Usable),
        ));
        *provider.execution_observation_failure.lock().unwrap() = Some(ObservationFailure::new(
            error,
            ObservationFailureCause::ExecutionFailed,
        ));
        self.service
            .submit(
                id.clone(),
                caller(action),
                action.into(),
                SubmittedMessage {
                    uploads: Vec::new(),
                    text: format!("blocked {action}"),
                    ..Default::default()
                },
                SubmissionMode::Queue,
            )
            .await
            .unwrap();
        tokio::time::timeout(BOUND, async {
            loop {
                let snapshot = self.snapshot(id).await;
                if snapshot.invocations.iter().any(|invocation| {
                    invocation.request.execution_id.as_str() == action
                        && invocation.result.as_ref().is_some_and(|result| {
                            result
                                .as_ref()
                                .err()
                                .and_then(AgentError::usage_limit)
                                .is_some()
                        })
                }) {
                    break;
                }
                tokio::task::yield_now().await;
            }
            let live = self.service.resolve(id, &caller("read")).await.unwrap();
            while !live.agent.is_idle().await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("usage-limit turn settles");
    }

    async fn restart(self) -> Self {
        let Self {
            service,
            storage,
            metadata,
            claude,
            codex,
            _directory,
        } = self;
        service.shutdown().await.unwrap();
        drop(service);
        storage.shutdown().await.unwrap();
        drop((storage, metadata, claude, codex));
        Self::in_directory(_directory)
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
                    uploads: Vec::new(),
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
            .fork(
                store.clone(),
                source.clone(),
                caller("self"),
                source.clone()
            )
            .await,
        Err(CreationFailure::Target(ConversationError::InvalidInput))
    ));
    // A retry after the source is gone is still answered by the creation the
    // fork made: the source is not read again once the fork is saved.
    let deleted = fixture
        .service
        .delete(source.clone(), caller("delete"))
        .await;
    assert!(
        matches!(
            deleted,
            Ok(_) | Err(ConversationError::DeletionIncomplete(_))
        ),
        "{deleted:?}"
    );
    fixture
        .service
        .fork(store, fork.clone(), caller("fork"), source.clone())
        .await
        .unwrap();
    assert_eq!(fixture.snapshot(&fork).await.handoffs.len(), 1);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_switch_invalidates_the_original_limit_before_applying() {
    for (enabled, schedule_before_limit) in [(true, true), (false, true), (true, false)] {
        let fixture = Fixture::new();
        let id = fixture.create().await;
        let before = if schedule_before_limit {
            Some(fixture.schedules(enabled).await)
        } else {
            None
        };
        fixture
            .blocked(&id, "original-limit", AgentId::Claude)
            .await;
        let schedule = match before {
            Some(store) => store,
            None => fixture.schedules(enabled).await,
        };
        if schedule_before_limit {
            fixture
                .service
                .reconcile_usage_limit_schedule(&id)
                .await
                .unwrap();
        }
        assert_eq!(
            schedule.open(&id.to_string()).await.unwrap().is_some(),
            enabled && schedule_before_limit
        );
        assert!(fixture.switch(&id, "agent-switch").await.unwrap());
        assert_eq!(fixture.record(&id).await.agent(), Some(AgentId::Codex));
        schedule
            .set_preference(true, ScheduleActor::system(), 2)
            .await
            .unwrap();
        fixture
            .service
            .reconcile_usage_limit_schedule(&id)
            .await
            .unwrap();
        assert!(schedule.open(&id.to_string()).await.unwrap().is_none());
        let connection =
            Connection::open(fixture._directory.path().join("gateway/scheduled.sqlite3")).unwrap();
        let receipt: String = connection.query_row("SELECT causal_execution FROM usage_limit_invalidation WHERE selection_kind = 'agent' AND request_id = 'agent-switch'", [], |row| row.get(0)).unwrap();
        assert_eq!(receipt, "original-limit");
        let audit: (String, String, String, String) = connection.query_row("SELECT cause, principal_id, surface_id, action_id FROM audit WHERE after_status = 'invalidated' ORDER BY id DESC LIMIT 1", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).unwrap();
        assert_eq!(
            audit,
            (
                "agent_changed".into(),
                "person".into(),
                "panel".into(),
                "agent-switch".into()
            )
        );
        fixture
            .service
            .switch_agent(
                id.clone(),
                caller("back"),
                AgentId::Claude,
                ConversationModelId::new("test").unwrap(),
            )
            .await
            .unwrap();
        fixture
            .service
            .reconcile_usage_limit_schedule(&id)
            .await
            .unwrap();
        assert!(schedule.open(&id.to_string()).await.unwrap().is_none());
        fixture.retire().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pending_agent_switch_retries_invalidation_as_its_original_request() {
    let fixture = Fixture::new();
    let id = fixture.create().await;
    let schedule = fixture.schedules(false).await;
    fixture
        .blocked(&id, "original-limit", AgentId::Claude)
        .await;
    let path = fixture._directory.path().join("gateway/scheduled.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_agent_invalidation BEFORE INSERT ON usage_limit_invalidation WHEN NEW.selection_kind = 'agent' BEGIN SELECT RAISE(ABORT, 'injected invalidation failure'); END;").unwrap();
    assert!(matches!(
        fixture.switch(&id, "original-switch").await,
        Err(ConversationError::Metadata)
    ));
    assert_eq!(
        fixture
            .metadata
            .agent_switch(&id, "original-switch")
            .await
            .unwrap()
            .unwrap()
            .state,
        ConversationAgentSwitchState::Pending
    );
    assert_eq!(fixture.record(&id).await.agent(), Some(AgentId::Claude));
    assert!(fixture
        .snapshot(&id)
        .await
        .handoff("person", "original-switch")
        .is_some());
    drop(schedule);
    drop(connection);
    let fixture = fixture.restart().await;
    let schedule = fixture.schedules(false).await;
    // A read can open the requested native agent but cannot authorize a new
    // submission before the original selection invalidation commits.
    let _ = fixture
        .service
        .read(id.clone(), caller("read-after-restart"))
        .await;
    let refused = fixture
        .service
        .submit(
            id.clone(),
            caller("new-work"),
            "new-work".into(),
            SubmittedMessage {
                uploads: Vec::new(),
                text: "must wait".into(),
                ..Default::default()
            },
            SubmissionMode::Queue,
        )
        .await;
    assert!(matches!(refused, Err(ConversationError::Metadata)));
    assert!(!fixture
        .snapshot(&id)
        .await
        .invocations
        .iter()
        .any(|invocation| invocation.request.execution_id.as_str() == "new-work"));
    assert!(!fixture
        .codex
        .executions
        .lock()
        .unwrap()
        .iter()
        .any(|execution| execution == "new-work"));
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("DROP TRIGGER fail_agent_invalidation;")
        .unwrap();
    // The invalidation may commit while finalizing the switch metadata fails.
    // A retry must reuse that receipt rather than sampling a newer turn.
    let metadata =
        Connection::open(fixture._directory.path().join("gateway/metadata.sqlite3")).unwrap();
    metadata.execute_batch("CREATE TRIGGER fail_agent_applied BEFORE UPDATE ON agent_switches WHEN NEW.state = 'applied' BEGIN SELECT RAISE(ABORT, 'injected Applied failure'); END;").unwrap();
    let unfinished = fixture
        .service
        .submit(
            id.clone(),
            caller("new-work"),
            "new-work".into(),
            SubmittedMessage {
                uploads: Vec::new(),
                text: "still must wait".into(),
                ..Default::default()
            },
            SubmissionMode::Queue,
        )
        .await;
    assert!(matches!(unfinished, Err(ConversationError::Metadata)));
    assert_eq!(
        fixture
            .metadata
            .agent_switch(&id, "original-switch")
            .await
            .unwrap()
            .unwrap()
            .state,
        ConversationAgentSwitchState::Pending
    );
    let committed: (i64, String) = connection.query_row(
        "SELECT count(*), causal_execution FROM usage_limit_invalidation WHERE selection_kind = 'agent' AND request_id = 'original-switch'",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(committed, (1, "original-limit".into()));
    let audits: i64 = connection.query_row(
        "SELECT count(*) FROM audit WHERE after_status = 'invalidated' AND cause = 'agent_changed' AND action_id = 'original-switch'",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(audits, 1);
    assert!(!fixture
        .snapshot(&id)
        .await
        .invocations
        .iter()
        .any(|invocation| invocation.request.execution_id.as_str() == "new-work"));
    metadata
        .execute_batch("DROP TRIGGER fail_agent_applied;")
        .unwrap();
    let mut recovering = caller("new-work");
    recovering.surface_id = "different-surface".into();
    fixture
        .service
        .submit(
            id.clone(),
            recovering,
            "new-work".into(),
            SubmittedMessage {
                uploads: Vec::new(),
                text: "now allowed".into(),
                ..Default::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    assert_eq!(
        fixture
            .metadata
            .agent_switch(&id, "original-switch")
            .await
            .unwrap()
            .unwrap()
            .state,
        ConversationAgentSwitchState::Applied
    );
    let audit: (String, String, String) = connection.query_row("SELECT principal_id,surface_id,action_id FROM audit WHERE after_status='invalidated' AND cause='agent_changed' ORDER BY id DESC LIMIT 1", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
    assert_eq!(
        audit,
        ("person".into(), "panel".into(), "original-switch".into())
    );
    // Let the admitted turn settle before a later independent usage limit.
    tokio::time::timeout(BOUND, async {
        loop {
            let live = fixture.service.resolve(&id, &caller("read")).await.unwrap();
            if live.agent.is_idle().await {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture.blocked(&id, "later-limit", AgentId::Codex).await;
    assert!(fixture.switch(&id, "original-switch").await.unwrap());
    schedule
        .set_preference(true, ScheduleActor::system(), 3)
        .await
        .unwrap();
    fixture
        .service
        .reconcile_usage_limit_schedule(&id)
        .await
        .unwrap();
    assert_eq!(
        schedule
            .open(&id.to_string())
            .await
            .unwrap()
            .unwrap()
            .causal_execution,
        "later-limit"
    );
    fixture.retire().await;
}

/// A stopped gateway may have claimed a submit without admitting a command.
/// Applied seals it even if selection returns to the original agent before recovery.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_switch_seals_a_crash_left_unattempted_continuation() {
    for enabled in [true, false] {
        let fixture = Fixture::new();
        let id = fixture.create().await;
        let schedule = fixture.schedules(true).await;
        fixture
            .blocked(&id, "original-limit", AgentId::Claude)
            .await;
        fixture
            .service
            .reconcile_usage_limit_schedule(&id)
            .await
            .unwrap();
        let row = schedule.open(&id.to_string()).await.unwrap().unwrap();
        assert_eq!(schedule.claim_due(row.due_at_ms, 8).await.unwrap().len(), 1);
        assert!(schedule
            .begin_submit(&row.execution_id, row.due_at_ms)
            .await
            .unwrap());
        schedule.release_submit(&row.execution_id).await.unwrap();
        schedule
            .set_preference(
                enabled,
                crate::conversation::application::ScheduleActor::system(),
                row.due_at_ms,
            )
            .await
            .unwrap();
        assert!(fixture.switch(&id, "switch-after-crash").await.unwrap());
        let lease = fixture
            .storage
            .open_creation(row.principal_id.clone(), row.execution_id.clone(), false)
            .await
            .unwrap()
            .unwrap();
        let receipt = lease
            .load_mutation(&row.execution_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            receipt.outcome(),
            Some(MutationOutcome::AgentChangedBeforeAttempt)
        );
        drop(lease);
        fixture
            .service
            .switch_agent(
                id.clone(),
                caller("back-after-crash"),
                AgentId::Claude,
                ConversationModelId::new("test").unwrap(),
            )
            .await
            .unwrap();
        let lease = fixture
            .storage
            .open_creation(row.principal_id.clone(), row.execution_id.clone(), false)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            lease
                .load_mutation(&row.execution_id)
                .await
                .unwrap()
                .unwrap(),
            receipt
        );
        drop(lease);
        assert!(
            !fixture.snapshot(&id).await.invocations.iter().any(|inv| inv
                .request
                .execution_id
                .as_str()
                == row.execution_id)
        );
        fixture.retire().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduled_delivery_recovers_pending_selection_before_fresh_classification() {
    let fixture = Fixture::new();
    let id = fixture.create().await;
    let schedule = fixture.schedules(true).await;
    fixture.blocked(&id, "pending-limit", AgentId::Claude).await;
    fixture
        .service
        .reconcile_usage_limit_schedule(&id)
        .await
        .unwrap();
    let row = schedule.open(&id.to_string()).await.unwrap().unwrap();
    let connection =
        Connection::open(fixture._directory.path().join("gateway/scheduled.sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_pending_selection BEFORE INSERT ON usage_limit_invalidation BEGIN SELECT RAISE(ABORT, 'injected invalidation failure'); END;").unwrap();
    assert!(fixture.switch(&id, "pending-selection").await.is_err());
    // Reset is still future. Fresh classification would defer this row. A
    // pending switch must recover first, and its failure must leave it held.
    let now = i64::try_from(TestClock.unix_milliseconds()).unwrap();
    connection
        .execute(
            "UPDATE message SET due_at_ms = ?1 WHERE execution_id = ?2",
            nessa_local_database::rusqlite::params![now, row.execution_id],
        )
        .unwrap();
    fixture.service.deliver_due_scheduled().await;
    let state: (String, i64) = connection
        .query_row(
            "SELECT status,due_at_ms FROM message WHERE execution_id = ?1",
            [&row.execution_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, ("delivering".into(), now));
    assert!(
        !fixture.snapshot(&id).await.invocations.iter().any(|inv| inv
            .request
            .execution_id
            .as_str()
            == row.execution_id)
    );
    connection
        .execute_batch("DROP TRIGGER fail_pending_selection;")
        .unwrap();
    // Positive control: pending recovery can finish once its durable write works.
    assert!(fixture.switch(&id, "pending-selection").await.unwrap());
    assert_eq!(
        fixture
            .metadata
            .agent_switch(&id, "pending-selection")
            .await
            .unwrap()
            .unwrap()
            .state,
        ConversationAgentSwitchState::Applied
    );
    fixture.retire().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_selection_recovery_retains_its_refusal_when_selection_matches_again() {
    let fixture = Fixture::new();
    let id = fixture.create().await;
    let schedule = fixture.schedules(true).await;
    fixture.blocked(&id, "old-limit", AgentId::Claude).await;
    fixture
        .service
        .reconcile_usage_limit_schedule(&id)
        .await
        .unwrap();
    let row = schedule.open(&id.to_string()).await.unwrap().unwrap();
    schedule.claim_due(row.due_at_ms, 8).await.unwrap();
    assert!(schedule
        .begin_submit(&row.execution_id, row.due_at_ms)
        .await
        .unwrap());
    schedule.release_submit(&row.execution_id).await.unwrap();
    drop(schedule);
    let fixture = fixture.restart().await;
    // Reproduce a persisted pre-fix ordering: selection changed while the
    // continuation's command had not been admitted or sealed. This service
    // has not bound scheduling yet, so the old row survives the first switch.
    assert!(fixture.switch(&id, "away-before-recovery").await.unwrap());
    let schedule = fixture.schedules(true).await;
    let connection =
        Connection::open(fixture._directory.path().join("gateway/scheduled.sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_return_selection BEFORE INSERT ON usage_limit_invalidation BEGIN SELECT RAISE(ABORT, 'injected invalidation failure'); END;").unwrap();
    assert!(fixture
        .service
        .switch_agent(
            id.clone(),
            caller("back-pending"),
            AgentId::Claude,
            ConversationModelId::new("test").unwrap()
        )
        .await
        .is_err());
    connection
        .execute_batch("DROP TRIGGER fail_return_selection;")
        .unwrap();
    let now = i64::try_from(TestClock.unix_milliseconds()).unwrap();
    connection
        .execute(
            "UPDATE message SET due_at_ms=?1, fence_until_ms=?1-1 WHERE execution_id=?2",
            nessa_local_database::rusqlite::params![now, row.execution_id],
        )
        .unwrap();
    fixture.service.deliver_due_scheduled().await;
    assert_eq!(fixture.record(&id).await.agent(), Some(AgentId::Claude));
    assert_eq!(
        schedule.status(&row.execution_id).await.unwrap().as_deref(),
        Some("refused")
    );
    let cause: String = connection.query_row("SELECT cause FROM audit WHERE execution_id=?1 AND after_status='refused' ORDER BY id DESC LIMIT 1", [&row.execution_id], |r| r.get(0)).unwrap();
    assert_eq!(cause, "agent_changed");
    assert!(
        !fixture.snapshot(&id).await.invocations.iter().any(|inv| inv
            .request
            .execution_id
            .as_str()
            == row.execution_id)
    );
    fixture.retire().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unchanged_selection_admits_one_recovered_continuation() {
    let fixture = Fixture::new();
    let id = fixture.create().await;
    let schedule = fixture.schedules(true).await;
    fixture.blocked(&id, "valid-limit", AgentId::Claude).await;
    fixture
        .service
        .reconcile_usage_limit_schedule(&id)
        .await
        .unwrap();
    let row = schedule.open(&id.to_string()).await.unwrap().unwrap();
    schedule.claim_due(row.due_at_ms, 8).await.unwrap();
    assert!(schedule
        .begin_submit(&row.execution_id, row.due_at_ms)
        .await
        .unwrap());
    schedule.release_submit(&row.execution_id).await.unwrap();
    let connection =
        Connection::open(fixture._directory.path().join("gateway/scheduled.sqlite3")).unwrap();
    let now = i64::try_from(TestClock.unix_milliseconds()).unwrap();
    connection
        .execute(
            "UPDATE message SET due_at_ms=?1, fence_until_ms=?1-1 WHERE execution_id=?2",
            nessa_local_database::rusqlite::params![now, row.execution_id],
        )
        .unwrap();
    tokio::time::timeout(BOUND, fixture.service.deliver_due_scheduled())
        .await
        .expect("owned permit reaches enqueue without reacquiring its lock");
    assert_eq!(
        schedule.status(&row.execution_id).await.unwrap().as_deref(),
        Some("delivered")
    );
    assert_eq!(
        fixture
            .snapshot(&id)
            .await
            .invocations
            .iter()
            .filter(|inv| inv.request.execution_id.as_str() == row.execution_id)
            .count(),
        1
    );
    fixture.service.deliver_due_scheduled().await;
    assert_eq!(
        fixture
            .snapshot(&id)
            .await
            .invocations
            .iter()
            .filter(|inv| inv.request.execution_id.as_str() == row.execution_id)
            .count(),
        1
    );
    fixture.retire().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retrying_an_old_selection_does_not_seal_a_later_submit() {
    let fixture = Fixture::new();
    let id = fixture.create().await;
    let schedule = fixture.schedules(true).await;
    fixture.blocked(&id, "first-limit", AgentId::Claude).await;
    fixture
        .service
        .reconcile_usage_limit_schedule(&id)
        .await
        .unwrap();
    let first = schedule.open(&id.to_string()).await.unwrap().unwrap();
    schedule.claim_due(first.due_at_ms, 8).await.unwrap();
    assert!(schedule
        .begin_submit(&first.execution_id, first.due_at_ms)
        .await
        .unwrap());
    assert!(fixture.switch(&id, "original-selection").await.unwrap());
    schedule
        .record_outcome(
            &first.execution_id,
            ScheduleSettlement::Refused,
            "agent_changed",
            first.due_at_ms,
        )
        .await
        .unwrap();
    schedule.release_submit(&first.execution_id).await.unwrap();
    fixture.blocked(&id, "later-limit", AgentId::Codex).await;
    fixture
        .service
        .reconcile_usage_limit_schedule(&id)
        .await
        .unwrap();
    let later = schedule.open(&id.to_string()).await.unwrap().unwrap();
    schedule.claim_due(later.due_at_ms, 8).await.unwrap();
    assert!(schedule
        .begin_submit(&later.execution_id, later.due_at_ms)
        .await
        .unwrap());
    fixture
        .service
        .invalidate_selection_usage_limit(
            SelectionChange::Agent,
            &id,
            "original-selection",
            ScheduleActor::person(&caller("original-selection")),
        )
        .await
        .unwrap();
    assert!(fixture
        .storage
        .open_creation(
            later.principal_id.clone(),
            later.execution_id.clone(),
            false
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        schedule
            .status(&later.execution_id)
            .await
            .unwrap()
            .as_deref(),
        Some("submitting")
    );
    schedule.release_submit(&later.execution_id).await.unwrap();
    fixture.retire().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_applied_pre_fix_selection_still_refuses_its_continuation_after_returning() {
    for enabled in [false, true] {
        let fixture = Fixture::new();
        let id = fixture.create().await;
        let schedule = fixture.schedules(true).await;
        fixture.blocked(&id, "legacy-limit", AgentId::Claude).await;
        fixture
            .service
            .reconcile_usage_limit_schedule(&id)
            .await
            .unwrap();
        let row = schedule.open(&id.to_string()).await.unwrap().unwrap();
        schedule.claim_due(row.due_at_ms, 8).await.unwrap();
        assert!(schedule
            .begin_submit(&row.execution_id, row.due_at_ms)
            .await
            .unwrap());
        schedule.release_submit(&row.execution_id).await.unwrap();
        let connection =
            Connection::open(fixture._directory.path().join("gateway/scheduled.sqlite3")).unwrap();
        let token = connection.query_row("SELECT incarnation,position,generation,agent,model FROM history_fence WHERE conversation=?1", [id.to_string()], |r| {
        Ok(PlanToken { history: HistoryGeneration { incarnation: r.get(0)?, position: u64::try_from(r.get::<_, i64>(1)?).unwrap(), generation: u64::try_from(r.get::<_, i64>(2)?).unwrap() }, preference_revision: 0, agent: r.get(3)?, model: r.get(4)? })
    }).unwrap();
        let token = PlanToken {
            preference_revision: schedule.preference().await.unwrap().revision,
            ..token
        };
        // Reconstruct the old writer's durable boundary: invalidation did not
        // seal a submitting command. The current application wrapper is omitted.
        assert!(schedule
            .invalidate_usage_limit(
                SelectionRequest {
                    change: SelectionChange::Agent,
                    id: "old-away"
                },
                &id.to_string(),
                Some(&row.causal_execution),
                &token,
                ScheduleActor::person(&caller("old-away")),
                row.due_at_ms
            )
            .await
            .unwrap());
        drop(connection);
        drop(schedule);
        let fixture = fixture.restart().await;
        // Scheduling is unbound during both real controls, preserving that old
        // writer's no-receipt state while each selection reaches Applied.
        assert!(fixture.switch(&id, "old-away").await.unwrap());
        assert!(fixture
            .service
            .switch_agent(
                id.clone(),
                caller("old-back"),
                AgentId::Claude,
                ConversationModelId::new("test").unwrap()
            )
            .await
            .unwrap());
        for request in ["old-away", "old-back"] {
            assert_eq!(
                fixture
                    .metadata
                    .agent_switch(&id, request)
                    .await
                    .unwrap()
                    .unwrap()
                    .state,
                ConversationAgentSwitchState::Applied
            );
        }
        assert!(fixture
            .metadata
            .pending_agent_switch(&id)
            .await
            .unwrap()
            .is_none());
        assert!(fixture
            .storage
            .open_creation(row.principal_id.clone(), row.execution_id.clone(), false)
            .await
            .unwrap()
            .is_none());
        let schedule = fixture.schedules(enabled).await;
        let connection =
            Connection::open(fixture._directory.path().join("gateway/scheduled.sqlite3")).unwrap();
        let now = i64::try_from(TestClock.unix_milliseconds()).unwrap();
        connection
            .execute(
                "UPDATE message SET due_at_ms=?1, fence_until_ms=?1-1 WHERE execution_id=?2",
                nessa_local_database::rusqlite::params![now, row.execution_id],
            )
            .unwrap();
        tokio::time::timeout(BOUND, fixture.service.deliver_due_scheduled())
            .await
            .unwrap();
        assert_eq!(
            schedule.status(&row.execution_id).await.unwrap().as_deref(),
            Some("refused")
        );
        let lease = fixture
            .storage
            .open_creation(row.principal_id.clone(), row.execution_id.clone(), false)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            lease
                .load_mutation(&row.execution_id)
                .await
                .unwrap()
                .unwrap()
                .outcome(),
            Some(MutationOutcome::AgentChangedBeforeAttempt)
        );
        drop(lease);
        let cause: String = connection.query_row("SELECT cause FROM audit WHERE execution_id=?1 AND after_status='refused' ORDER BY id DESC LIMIT 1", [&row.execution_id], |r| r.get(0)).unwrap();
        assert_eq!(cause, "agent_changed");
        assert!(
            !fixture.snapshot(&id).await.invocations.iter().any(|inv| inv
                .request
                .execution_id
                .as_str()
                == row.execution_id)
        );
        fixture.retire().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unread_retained_selection_evidence_holds_admission_for_bounded_retry() {
    let fixture = Fixture::new();
    let id = fixture.create().await;
    let schedule = fixture.schedules(true).await;
    fixture.blocked(&id, "unread-limit", AgentId::Claude).await;
    fixture
        .service
        .reconcile_usage_limit_schedule(&id)
        .await
        .unwrap();
    let row = schedule.open(&id.to_string()).await.unwrap().unwrap();
    schedule.claim_due(row.due_at_ms, 8).await.unwrap();
    assert!(schedule
        .begin_submit(&row.execution_id, row.due_at_ms)
        .await
        .unwrap());
    schedule.release_submit(&row.execution_id).await.unwrap();
    let connection =
        Connection::open(fixture._directory.path().join("gateway/scheduled.sqlite3")).unwrap();
    let now = i64::try_from(TestClock.unix_milliseconds()).unwrap();
    connection
        .execute(
            "UPDATE message SET due_at_ms=?1, fence_until_ms=?1-1 WHERE execution_id=?2",
            nessa_local_database::rusqlite::params![now, row.execution_id],
        )
        .unwrap();
    connection
        .execute_batch("DROP TABLE usage_limit_invalidation;")
        .unwrap();
    assert_eq!(fixture.service.deliver_due_scheduled().await, Some(1_000));
    let state: (String, i64, i64) = connection
        .query_row(
            "SELECT status,retry_at_ms,retry_failures FROM message WHERE execution_id=?1",
            [&row.execution_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(state, ("submitting".into(), now + 1_000, 1));
    assert!(fixture
        .storage
        .open_creation(row.principal_id.clone(), row.execution_id.clone(), false)
        .await
        .unwrap()
        .is_none());
    assert!(
        !fixture.snapshot(&id).await.invocations.iter().any(|inv| inv
            .request
            .execution_id
            .as_str()
            == row.execution_id)
    );
    fixture.retire().await;
}
