use super::super::agents::MemoryStorage;
use super::super::support::*;
use nessa_sdk::application::agent_execution::hooks::{
    BeforePermissionReview, HookError, InvocationContext, InvocationHook,
    InvocationObservationWindow, ObservationKind, PhrasePresence, ProposedToolCall, ToolVerdict,
};
use nessa_sdk::domain::agent_execution::permissions::RefusalOutcome;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct ScriptedEvents {
    events: Vec<ExecutionEvent>,
    started: Arc<AtomicBool>,
    index: usize,
}

impl ExecutionEventStream for ScriptedEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async move {
            if !self.started.load(Ordering::SeqCst) {
                return std::future::pending().await;
            }
            if self.index >= self.events.len() {
                return Ok(None);
            }
            let event = self.events[self.index].clone();
            self.index += 1;
            Ok(Some(event))
        })
    }
}

struct ScriptedBackend {
    started: Arc<AtomicBool>,
    answered: Arc<AtomicUsize>,
    closes: Arc<Mutex<Vec<SessionCloseRequest>>>,
}

impl ProviderSessionBackend for ScriptedBackend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, _: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            self.started.store(true, Ordering::SeqCst);
            ProviderExecutionReply::Finished(ExecutionReport::new(
                Some(Ok(ExecutionOutcome::Completed)),
                None,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn answer_question(&self, _: QuestionAnswer) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::Unsupported("this fixture asks nothing".into()),
                ProviderSessionState::Usable,
            ))
        })
    }
    fn answer_permission(
        &self,
        _: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        let answered = self.answered.clone();
        Box::pin(async move {
            answered.fetch_add(1, Ordering::SeqCst);
            Err(ProviderOperationFailure::permission_answer(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
                PermissionSelectionState::Pending,
            ))
        })
    }
    fn cancel_permission(
        &self,
        _: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn close(&self, request: SessionCloseRequest) -> CleanupFuture<'_> {
        self.closes.lock().unwrap().push(request);
        Box::pin(async { CleanupReport::confirmed(CloseOutcome { forced: false }) })
    }
}

struct ScriptedProvider {
    events: Mutex<Option<Vec<ExecutionEvent>>>,
    started: Arc<AtomicBool>,
    answered: Arc<AtomicUsize>,
    closes: Arc<Mutex<Vec<SessionCloseRequest>>>,
}

impl AgentProvider for ScriptedProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("hook-window", "fixture", "test").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("context").unwrap(),
                    Arc::new(ScriptedBackend {
                        started: self.started.clone(),
                        answered: self.answered.clone(),
                        closes: self.closes.clone(),
                    }),
                    capabilities(),
                ),
                events: Box::new(ScriptedEvents {
                    events: self.events.lock().unwrap().take().unwrap(),
                    started: self.started.clone(),
                    index: 0,
                }),
            })
        })
    }
}

fn message(thought: bool, body: &str) -> ExecutionEvent {
    let chunk = if thought {
        MessageChunk::thought(body)
    } else {
        MessageChunk::text(body)
    };
    ExecutionEvent::new(
        ExecutionId::new("turn").unwrap(),
        ExecutionUpdate::Message(chunk),
    )
}

fn review() -> ExecutionEvent {
    let options = PermissionOptions::new(
        vec![PermissionOption::new(
            PermissionOptionId::new("deny").unwrap(),
            "Deny",
            PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request()),
        )
        .unwrap()],
        &PermissionOfferPolicy::once_only(),
    )
    .unwrap();
    ExecutionEvent::new(
        ExecutionId::new("turn").unwrap(),
        ExecutionUpdate::PermissionRequested {
            id: PermissionId::new("review").unwrap(),
            tool_id: ToolCallId::new("bash-1").unwrap(),
            observation: ToolObservation::default(),
            input: ToolReviewInput {
                name: "Bash".into(),
                arguments_json: "{\"command\":\"rm\"}".into(),
            },
            options,
        },
    )
}

fn script() -> Vec<ExecutionEvent> {
    vec![
        message(true, "SECRET_REASON"),
        message(false, "do not "),
        message(false, "run it"),
        review(),
    ]
}

struct Seen {
    phase: &'static str,
    call: Option<String>,
    kinds: Vec<ObservationKind>,
    split: PhrasePresence,
    secret: PhrasePresence,
    included: usize,
}

struct Fixture {
    agent: Agent,
    storage: MemoryStorage,
    answered: Arc<AtomicUsize>,
    closes: Arc<Mutex<Vec<SessionCloseRequest>>>,
}

async fn agent_with(events: Vec<ExecutionEvent>, hooks: Vec<Arc<dyn InvocationHook>>) -> Fixture {
    let storage = MemoryStorage::default();
    let answered = Arc::new(AtomicUsize::new(0));
    let closes = Arc::new(Mutex::new(Vec::new()));
    let agent = attached_agent(
        Arc::new(ScriptedProvider {
            events: Mutex::new(Some(events)),
            started: Arc::new(AtomicBool::new(false)),
            answered: answered.clone(),
            closes: closes.clone(),
        }),
        storage.manager().await,
    )
    .await
    .unwrap();
    for hook in hooks {
        agent.add_invocation_hook(hook);
    }
    Fixture {
        agent,
        storage,
        answered,
        closes,
    }
}

fn request() -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new("turn").unwrap(),
        user_message: UserMessage::text_only(PromptText::new("hello").unwrap()),
        estimated_input_tokens: 1,
        reserved_output_tokens: 100,
    }
}

fn second_request() -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new("turn-2").unwrap(),
        user_message: UserMessage::text_only(PromptText::new("again").unwrap()),
        estimated_input_tokens: 1,
        reserved_output_tokens: 100,
    }
}

#[tokio::test]
async fn pre_tool_and_turn_boundary_share_kinds_and_thinking() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let tool_seen = seen.clone();
    let after_seen = seen.clone();
    let fixture = agent_with(
        script(),
        vec![
            Arc::new(ReviewHook {
                seen: tool_seen,
                verdict: ToolVerdict::Allow,
            }),
            Arc::new(AfterHook { seen: after_seen }),
        ],
    )
    .await;
    fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect("hook admitted the review");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].phase, "tool");
    assert_eq!(seen[0].call.as_deref(), Some("Bash"));
    assert_eq!(
        seen[0].kinds,
        vec![
            ObservationKind::Thinking,
            ObservationKind::ModelText,
            ObservationKind::ModelText,
        ]
    );
    assert!(seen[0].split.is_present());
    assert!(seen[0].secret.is_present());
    assert_eq!(seen[0].included, 3);
    assert_eq!(seen[1].phase, "after");
    assert!(seen[1].secret.is_present());
    assert!(seen[1].split.is_present());
    assert!(seen[1].kinds.contains(&ObservationKind::PermissionReview));
}

#[tokio::test]
async fn add_hook_reports_each_event_kind() {
    let kinds = Arc::new(Mutex::new(Vec::new()));
    let recorded = kinds.clone();
    let fixture = agent_with(script(), Vec::new()).await;
    fixture.agent.add_hook(
        BeforePermissionReview::new(),
        move |call: &ProposedToolCall<'_>, observations: &InvocationObservationWindow<'_>| {
            assert_eq!(call.name(), "Bash");
            *recorded.lock().unwrap() = observations.iter().map(ObservationKind::of).collect();
            Ok(ToolVerdict::Allow)
        },
    );
    fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect("registered hook admitted the review");
    assert_eq!(
        *kinds.lock().unwrap(),
        vec![
            ObservationKind::Thinking,
            ObservationKind::ModelText,
            ObservationKind::ModelText,
        ]
    );
}

struct ReviewHook {
    seen: Arc<Mutex<Vec<Seen>>>,
    verdict: ToolVerdict,
}

impl InvocationHook for ReviewHook {
    fn before_permission_review(
        &self,
        call: &ProposedToolCall<'_>,
        observations: &InvocationObservationWindow<'_>,
    ) -> Result<ToolVerdict, HookError> {
        self.seen.lock().unwrap().push(Seen {
            phase: "tool",
            call: Some(call.name().to_owned()),
            kinds: observations.iter().map(ObservationKind::of).collect(),
            split: observations.contains_phrase("do not run"),
            secret: observations.contains_phrase("SECRET_REASON"),
            included: observations.included_events(),
        });
        Ok(self.verdict.clone())
    }
}

struct AfterHook {
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl InvocationHook for AfterHook {
    fn after_invocation(
        &self,
        context: &InvocationContext<'_>,
        _: &Result<ExecutionOutcome, AgentError>,
    ) -> Result<(), HookError> {
        let window = &context.observations;
        self.seen.lock().unwrap().push(Seen {
            phase: "after",
            call: None,
            kinds: window.iter().map(ObservationKind::of).collect(),
            split: window.contains_phrase("do not run"),
            secret: window.contains_phrase("SECRET_REASON"),
            included: window.included_events(),
        });
        Ok(())
    }
}

#[tokio::test]
async fn a_continue_refusal_keeps_the_review_and_the_session() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let fixture = agent_with(
        script(),
        vec![Arc::new(ReviewHook {
            seen: seen.clone(),
            verdict: ToolVerdict::refuse("stop this call", RefusalOutcome::Continue),
        })],
    )
    .await;
    fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect("deny-and-continue finishes the turn");
    assert_eq!(fixture.answered.load(Ordering::SeqCst), 1);
    assert!(fixture.closes.lock().unwrap().is_empty());
    let saved = fixture.storage.snapshot();
    let events = &saved.invocations[0].events;
    assert!(events
        .iter()
        .any(|event| matches!(event.update(), ExecutionUpdate::PermissionRequested { .. })));
    fixture
        .agent
        .invoke(second_request(), close_action())
        .await
        .expect("the session stays usable");
}

#[tokio::test]
async fn a_continue_without_a_deny_option_does_not_invent_one() {
    let mut events = script();
    events.pop();
    events.push(allow_only_review());
    let fixture = agent_with(
        events,
        vec![Arc::new(ReviewHook {
            seen: Arc::new(Mutex::new(Vec::new())),
            verdict: ToolVerdict::refuse("no", RefusalOutcome::Continue),
        })],
    )
    .await;
    fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect("a continue with no deny option ends the turn without closing");
    assert_eq!(fixture.answered.load(Ordering::SeqCst), 0);
    assert!(fixture.closes.lock().unwrap().is_empty());
    fixture
        .agent
        .invoke(second_request(), close_action())
        .await
        .expect("the session stays usable");
}

#[tokio::test]
async fn an_end_turn_refusal_does_not_close_the_session() {
    let fixture = agent_with(
        script(),
        vec![Arc::new(ReviewHook {
            seen: Arc::new(Mutex::new(Vec::new())),
            verdict: ToolVerdict::refuse("stop the turn", RefusalOutcome::EndTurn),
        })],
    )
    .await;
    fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect("ending the turn leaves the session open");
    assert!(fixture.closes.lock().unwrap().is_empty());
    fixture
        .agent
        .invoke(second_request(), close_action())
        .await
        .expect("the session stays usable");
}

#[tokio::test]
async fn a_hook_error_denies_and_continues() {
    let fixture = agent_with(script(), vec![Arc::new(FailingHook)]).await;
    fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect("a hook error does not close the session");
    assert_eq!(fixture.answered.load(Ordering::SeqCst), 1);
    let saved = fixture.storage.snapshot();
    assert!(saved.invocations[0]
        .events
        .iter()
        .any(|event| matches!(event.update(), ExecutionUpdate::PermissionRequested { .. })));
    fixture
        .agent
        .invoke(second_request(), close_action())
        .await
        .expect("the session stays usable");
}

struct FailingHook;

impl InvocationHook for FailingHook {
    fn before_permission_review(
        &self,
        _: &ProposedToolCall<'_>,
        _: &InvocationObservationWindow<'_>,
    ) -> Result<ToolVerdict, HookError> {
        Err(HookError::Failed("stop this call".into()))
    }
}

fn allow_only_review() -> ExecutionEvent {
    let options = PermissionOptions::new(
        vec![PermissionOption::new(
            PermissionOptionId::new("allow").unwrap(),
            "Allow",
            PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
        )
        .unwrap()],
        &PermissionOfferPolicy::once_only(),
    )
    .unwrap();
    ExecutionEvent::new(
        ExecutionId::new("turn").unwrap(),
        ExecutionUpdate::PermissionRequested {
            id: PermissionId::new("review").unwrap(),
            tool_id: ToolCallId::new("bash-1").unwrap(),
            observation: ToolObservation::default(),
            input: ToolReviewInput {
                name: "Bash".into(),
                arguments_json: "{\"command\":\"rm\"}".into(),
            },
            options,
        },
    )
}

#[tokio::test]
async fn a_panicked_review_hook_denies_and_continues() {
    let fixture = agent_with(script(), vec![Arc::new(PanicHook)]).await;
    fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect("a panicked hook does not close the session");
    assert_eq!(fixture.answered.load(Ordering::SeqCst), 1);
    fixture
        .agent
        .invoke(second_request(), close_action())
        .await
        .expect("the session stays usable after a panicked hook");
}

struct PanicHook;

impl InvocationHook for PanicHook {
    fn before_permission_review(
        &self,
        _: &ProposedToolCall<'_>,
        _: &InvocationObservationWindow<'_>,
    ) -> Result<ToolVerdict, HookError> {
        panic!("hook bug");
    }
}

#[tokio::test]
async fn close_session_is_a_policy_refusal() {
    let fixture = agent_with(
        script(),
        vec![Arc::new(ReviewHook {
            seen: Arc::new(Mutex::new(Vec::new())),
            verdict: ToolVerdict::refuse("stop the session", RefusalOutcome::CloseSession),
        })],
    )
    .await;
    let _ = fixture.agent.invoke(request(), close_action()).await;
    assert_eq!(
        fixture.closes.lock().unwrap().as_slice(),
        [SessionCloseRequest::PolicyRefusal]
    );
    let second = fixture.agent.invoke(second_request(), close_action()).await;
    assert!(
        matches!(second, Err(AgentError::AttachmentUnavailable(_))),
        "closing the session left it usable: {second:?}"
    );
}

struct SlowToolHook {
    entered: Arc<AtomicBool>,
    hold: std::time::Duration,
}

impl InvocationHook for SlowToolHook {
    fn before_permission_review(
        &self,
        _: &ProposedToolCall<'_>,
        _: &InvocationObservationWindow<'_>,
    ) -> Result<ToolVerdict, HookError> {
        self.entered.store(true, Ordering::SeqCst);
        std::thread::sleep(self.hold);
        Ok(ToolVerdict::Allow)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn review_slow_pre_tool_hook_delays_close() {
    let entered = Arc::new(AtomicBool::new(false));
    let fixture = agent_with(
        script(),
        vec![Arc::new(SlowToolHook {
            entered: entered.clone(),
            hold: std::time::Duration::from_secs(3),
        })],
    )
    .await;
    let runner = fixture.agent.clone();
    let turn = tokio::spawn(async move { runner.invoke(request(), close_action()).await });
    while !entered.load(Ordering::SeqCst) {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let started = std::time::Instant::now();
    let closed = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        fixture.agent.close(close_action()),
    )
    .await;
    let close_elapsed = started.elapsed();
    let turn_result = turn.await.unwrap();
    assert!(
        close_elapsed < std::time::Duration::from_secs(1),
        "close waited {close_elapsed:?} behind a permission-review hook ({closed:?}); turn {turn_result:?}"
    );
}

#[tokio::test]
async fn review_pre_tool_rejection_leaves_the_session_usable() {
    let fixture = agent_with(
        script(),
        vec![Arc::new(ReviewHook {
            seen: Arc::new(Mutex::new(Vec::new())),
            verdict: ToolVerdict::refuse("stop this call", RefusalOutcome::Continue),
        })],
    )
    .await;
    fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect("the refused call does not fail the turn");
    fixture
        .agent
        .invoke(second_request(), close_action())
        .await
        .expect("a hook refusal of one tool left the session unusable");
}
