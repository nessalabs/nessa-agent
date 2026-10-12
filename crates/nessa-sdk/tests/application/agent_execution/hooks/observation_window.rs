use super::super::agents::MemoryStorage;
use super::super::support::*;
use nessa_sdk::application::agent_execution::hooks::{
    AfterInvocation, AfterInvocationEvent, BeforeInvocation, BeforePermissionReview, HookError,
    HookRegistration, InvocationContext, InvocationHook, InvocationObservationWindow,
    ObservationKind, PhrasePresence, ProposedToolCall, ToolVerdict,
};
use nessa_sdk::domain::agent_execution::permissions::RefusalOutcome;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct ScriptedEvents {
    events: Vec<ExecutionEvent>,
    started: Arc<AtomicBool>,
    release: Arc<tokio::sync::Notify>,
    index: usize,
}

impl ExecutionEventStream for ScriptedEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async move {
            if !self.started.load(Ordering::SeqCst) {
                return std::future::pending().await;
            }
            if self.index >= self.events.len() {
                self.release.notify_waiters();
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
    cancelled: Arc<AtomicUsize>,
    turn_cancels: Arc<AtomicUsize>,
    closes: Arc<Mutex<Vec<SessionCloseRequest>>>,
    answers: Arc<Mutex<Vec<PermissionAnswer>>>,
    withdrawals: Arc<Mutex<Vec<PermissionCancellationRequest>>>,
    controller: Arc<Mutex<ExecutionController>>,
    authority: PermissionAuthoritySource,
    reviews: Arc<Vec<ExecutionEvent>>,
    release: Arc<tokio::sync::Notify>,
    turn_cancel: bool,
    fail_delivery: bool,
    fail_turn_cancel: bool,
}

fn admit_reviews(
    controller: &Mutex<ExecutionController>,
    execution: &ExecutionId,
    reviews: &[ExecutionEvent],
) {
    let mut controller = controller.lock().expect("fixture controller");
    if controller.begin_execution(execution.clone()).is_err() {
        return;
    }
    for event in reviews {
        if event.execution_id() != execution {
            continue;
        }
        let ExecutionUpdate::PermissionRequested {
            id,
            tool_id,
            input,
            options,
            ..
        } = event.update()
        else {
            continue;
        };
        controller
            .request_permission(
                execution,
                id.clone(),
                ToolCallUpdate::new(tool_id.clone(), None, None, None, None, None),
                input.clone(),
                options.clone(),
            )
            .expect("fixture admits the scripted review");
    }
}

fn clear_turn_reviews(
    controller: &Mutex<ExecutionController>,
    turn: &ExecutionId,
    reviews: &[ExecutionEvent],
) {
    let mut controller = controller.lock().expect("fixture controller");
    let actor = ActionContext::new("permission-review-hook", "turn-cancel", turn.as_str())
        .expect("fixture turn-cancel actor");
    let reason = PermissionCancellationReason::custom(
        CustomPermissionCancellationReason::new("turn-cancelled", "the turn was cancelled")
            .expect("fixture turn-cancel reason"),
    );
    for event in reviews {
        if event.execution_id() != turn {
            continue;
        }
        let ExecutionUpdate::PermissionRequested { id, .. } = event.update() else {
            continue;
        };
        let _ = controller.cancel_permission(
            turn,
            id,
            reason.clone(),
            CancellationOrigin::Client(actor.clone()),
        );
    }
}

impl ProviderSessionBackend for ScriptedBackend {
    fn permission_authority(
        &self,
    ) -> Result<Option<PermissionAuthority>, PermissionAuthorityError> {
        self.authority.read()
    }
    fn supports_turn_cancel(&self) -> bool {
        self.turn_cancel
    }
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, input: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        let started = self.started.clone();
        let controller = self.controller.clone();
        let reviews = self.reviews.clone();
        let release = self.release.clone();
        Box::pin(async move {
            admit_reviews(&controller, &input.execution_id, &reviews);
            started.store(true, Ordering::SeqCst);
            release.notified().await;
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
        answer: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        let answered = self.answered.clone();
        let answers = self.answers.clone();
        let controller = self.controller.clone();
        let fail = self.fail_delivery;
        Box::pin(async move {
            answered.fetch_add(1, Ordering::SeqCst);
            answers
                .lock()
                .expect("fixture answers")
                .push(answer.clone());
            if fail {
                return Err(ProviderOperationFailure::permission_answer(
                    AgentError::StalePermission,
                    ProviderSessionState::Usable,
                    PermissionSelectionState::Pending,
                ));
            }
            controller
                .lock()
                .expect("fixture controller")
                .answer_permission(answer)
                .map_err(|error| {
                    ProviderOperationFailure::permission_answer(
                        error,
                        ProviderSessionState::Usable,
                        PermissionSelectionState::Pending,
                    )
                })
        })
    }
    fn cancel_permission(
        &self,
        input: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        let cancelled = self.cancelled.clone();
        let withdrawals = self.withdrawals.clone();
        let controller = self.controller.clone();
        let fail = self.fail_delivery;
        Box::pin(async move {
            cancelled.fetch_add(1, Ordering::SeqCst);
            withdrawals
                .lock()
                .expect("fixture withdrawals")
                .push(input.clone());
            if fail {
                return Err(ProviderOperationFailure::new(
                    AgentError::StalePermission,
                    ProviderSessionState::Usable,
                ));
            }
            controller
                .lock()
                .expect("fixture controller")
                .cancel_review(input)
                .map_err(|error| ProviderOperationFailure::new(error, ProviderSessionState::Usable))
        })
    }
    fn cancel_turn(&self, turn: ExecutionId) -> ProviderOperationFuture<'_, ()> {
        let turn_cancels = self.turn_cancels.clone();
        let controller = self.controller.clone();
        let reviews = self.reviews.clone();
        let release = self.release.clone();
        let fail = self.fail_turn_cancel;
        Box::pin(async move {
            turn_cancels.fetch_add(1, Ordering::SeqCst);
            if fail {
                return Err(ProviderOperationFailure::new(
                    AgentError::Transport("turn cancel failed".into()),
                    ProviderSessionState::Usable,
                ));
            }
            clear_turn_reviews(&controller, &turn, &reviews);
            release.notify_waiters();
            Ok(())
        })
    }
    fn close(&self, request: SessionCloseRequest) -> CleanupFuture<'_> {
        self.closes.lock().unwrap().push(request);
        self.release.notify_waiters();
        Box::pin(async { CleanupReport::confirmed(CloseOutcome { forced: false }) })
    }
}

struct ScriptedProvider {
    events: Mutex<Option<Vec<ExecutionEvent>>>,
    started: Arc<AtomicBool>,
    answered: Arc<AtomicUsize>,
    cancelled: Arc<AtomicUsize>,
    turn_cancels: Arc<AtomicUsize>,
    closes: Arc<Mutex<Vec<SessionCloseRequest>>>,
    answers: Arc<Mutex<Vec<PermissionAnswer>>>,
    withdrawals: Arc<Mutex<Vec<PermissionCancellationRequest>>>,
    controller: Arc<Mutex<ExecutionController>>,
    authority: PermissionAuthoritySource,
    release: Arc<tokio::sync::Notify>,
    turn_cancel: bool,
    fail_delivery: bool,
    fail_turn_cancel: bool,
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
            let events = self.events.lock().unwrap().take().unwrap();
            let reviews = Arc::new(events.clone());
            let release = self.release.clone();
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("context").unwrap(),
                    Arc::new(ScriptedBackend {
                        started: self.started.clone(),
                        answered: self.answered.clone(),
                        cancelled: self.cancelled.clone(),
                        turn_cancels: self.turn_cancels.clone(),
                        closes: self.closes.clone(),
                        answers: self.answers.clone(),
                        withdrawals: self.withdrawals.clone(),
                        controller: self.controller.clone(),
                        authority: self.authority.clone(),
                        reviews,
                        release: release.clone(),
                        turn_cancel: self.turn_cancel,
                        fail_delivery: self.fail_delivery,
                        fail_turn_cancel: self.fail_turn_cancel,
                    }),
                    capabilities(),
                ),
                events: Box::new(ScriptedEvents {
                    events,
                    started: self.started.clone(),
                    release,
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
    cancelled: Arc<AtomicUsize>,
    turn_cancels: Arc<AtomicUsize>,
    closes: Arc<Mutex<Vec<SessionCloseRequest>>>,
    answers: Arc<Mutex<Vec<PermissionAnswer>>>,
    withdrawals: Arc<Mutex<Vec<PermissionCancellationRequest>>>,
    authority: PermissionAuthoritySource,
}

struct FixtureKnobs {
    turn_cancel: bool,
    fail_delivery: bool,
    fail_turn_cancel: bool,
}

async fn agent_with(events: Vec<ExecutionEvent>, hooks: Vec<Arc<dyn InvocationHook>>) -> Fixture {
    open_fixture(
        events,
        hooks,
        FixtureKnobs {
            turn_cancel: true,
            fail_delivery: false,
            fail_turn_cancel: false,
        },
    )
    .await
}

async fn open_fixture(
    events: Vec<ExecutionEvent>,
    hooks: Vec<Arc<dyn InvocationHook>>,
    knobs: FixtureKnobs,
) -> Fixture {
    open_on_clock(events, hooks, knobs, None).await
}

async fn open_on_clock(
    events: Vec<ExecutionEvent>,
    hooks: Vec<Arc<dyn InvocationHook>>,
    knobs: FixtureKnobs,
    clock: Option<Arc<dyn MessageCommitClock>>,
) -> Fixture {
    let storage = MemoryStorage::default();
    let answered = Arc::new(AtomicUsize::new(0));
    let cancelled = Arc::new(AtomicUsize::new(0));
    let turn_cancels = Arc::new(AtomicUsize::new(0));
    let closes = Arc::new(Mutex::new(Vec::new()));
    let answers = Arc::new(Mutex::new(Vec::new()));
    let withdrawals = Arc::new(Mutex::new(Vec::new()));
    let controller = ExecutionController::new(ExecutionSessionId::new("context").unwrap());
    let authority = controller.permission_authority_source();
    let agent = attached_agent(
        Arc::new(ScriptedProvider {
            events: Mutex::new(Some(events)),
            started: Arc::new(AtomicBool::new(false)),
            answered: answered.clone(),
            cancelled: cancelled.clone(),
            turn_cancels: turn_cancels.clone(),
            closes: closes.clone(),
            answers: answers.clone(),
            withdrawals: withdrawals.clone(),
            controller: Arc::new(Mutex::new(controller)),
            authority: authority.clone(),
            release: Arc::new(tokio::sync::Notify::new()),
            turn_cancel: knobs.turn_cancel,
            fail_delivery: knobs.fail_delivery,
            fail_turn_cancel: knobs.fail_turn_cancel,
        }),
        match clock {
            Some(clock) => storage.manager_with_clock(clock).await,
            None => storage.manager().await,
        },
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
        cancelled,
        turn_cancels,
        closes,
        answers,
        withdrawals,
        authority,
    }
}

fn hook_refusal(answer: &PermissionAnswer) -> &HookRefusal {
    match answer.attribution.basis() {
        ApprovalBasis::Hook(refusal) => refusal,
        other => panic!("hook refusal was recorded as {other:?}"),
    }
}

fn review_still_pending(fixture: &Fixture, permission: &str) -> bool {
    fixture
        .authority
        .read()
        .expect("fixture authority")
        .expect("fixture published a review authority")
        .pending(&PermissionId::new(permission).unwrap())
        .expect("fixture review owner is alive")
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

struct AllowHook;

impl InvocationHook for AllowHook {
    fn before_permission_review(
        &self,
        _: &ProposedToolCall<'_>,
        _: &InvocationObservationWindow<'_>,
    ) -> Result<ToolVerdict, HookError> {
        Ok(ToolVerdict::Allow)
    }
}

#[tokio::test]
async fn a_continue_refusal_keeps_the_review_and_the_session() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let fixture = agent_with(
        script(),
        vec![
            Arc::new(AllowHook),
            Arc::new(ReviewHook {
                seen: seen.clone(),
                verdict: ToolVerdict::refuse("stop this call", RefusalOutcome::Continue),
            }),
        ],
    )
    .await;
    fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect("deny-and-continue finishes the turn");
    assert_eq!(fixture.answered.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.cancelled.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.turn_cancels.load(Ordering::SeqCst), 0);
    {
        let answers = fixture.answers.lock().unwrap();
        let refusal = hook_refusal(&answers[0]);
        assert_eq!(refusal.index(), Some(1));
        assert_eq!(refusal.reason(), "stop this call");
        assert_eq!(refusal.outcome(), &RefusalOutcome::Continue);
        assert_eq!(
            answers[0].attribution.actor().principal_id(),
            "permission-review-hook"
        );
        assert_eq!(answers[0].attribution.actor().surface_id(), "1");
        assert_ne!(
            answers[0].attribution.actor().principal_id(),
            close_action().principal_id()
        );
    }
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
async fn a_refused_review_is_not_published_for_a_person_to_answer() {
    let fixture = agent_with(
        script(),
        vec![Arc::new(ReviewHook {
            seen: Arc::new(Mutex::new(Vec::new())),
            verdict: ToolVerdict::refuse("stop this call", RefusalOutcome::Continue),
        })],
    )
    .await;
    let mut published = fixture.agent.subscribe();
    fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect("deny-and-continue finishes the turn");
    assert_eq!(fixture.answered.load(Ordering::SeqCst), 1);
    let mut saw_message = false;
    while let Ok(Ok(Some(event))) =
        tokio::time::timeout(std::time::Duration::from_millis(50), published.next()).await
    {
        assert!(
            !matches!(event.update(), ExecutionUpdate::PermissionRequested { .. }),
            "a review the hook already answered was published as pending"
        );
        saw_message |= matches!(event.update(), ExecutionUpdate::Message(_));
    }
    assert!(saw_message, "the turn's other events were not published");
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
    assert_eq!(fixture.cancelled.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.turn_cancels.load(Ordering::SeqCst), 1);
    {
        let withdrawals = fixture.withdrawals.lock().unwrap();
        assert_eq!(
            withdrawals[0].actor.principal_id(),
            "permission-review-hook"
        );
        assert_eq!(withdrawals[0].actor.surface_id(), "0");
        assert_eq!(withdrawals[0].actor.request_id(), "review");
        match withdrawals[0].reason.view() {
            PermissionCancellationReasonView::Custom(reason) => {
                assert_eq!(reason.code(), "hook-refusal.end_turn");
                assert_eq!(reason.explanation(), "no");
            }
            other => panic!("withdrawal reason was {other:?}"),
        }
    }
    assert!(!review_still_pending(&fixture, "review"));
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
    assert_eq!(fixture.answered.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.cancelled.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.turn_cancels.load(Ordering::SeqCst), 1);
    {
        let answers = fixture.answers.lock().unwrap();
        assert_eq!(
            hook_refusal(&answers[0]).outcome(),
            &RefusalOutcome::EndTurn
        );
        assert_eq!(hook_refusal(&answers[0]).reason(), "stop the turn");
        assert_eq!(
            answers[0].attribution.actor().principal_id(),
            "permission-review-hook"
        );
    }
    assert!(!review_still_pending(&fixture, "review"));
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
    assert_eq!(fixture.turn_cancels.load(Ordering::SeqCst), 0);
    assert_eq!(
        hook_refusal(&fixture.answers.lock().unwrap()[0]).reason(),
        "hook failed: error"
    );
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
    assert_eq!(
        hook_refusal(&fixture.answers.lock().unwrap()[0]).reason(),
        "hook failed: panic"
    );
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
async fn a_hook_that_returns_stopped_is_deny_and_continue() {
    let fixture = agent_with(
        script(),
        vec![Arc::new(ReviewHook {
            seen: Arc::new(Mutex::new(Vec::new())),
            verdict: ToolVerdict::Stopped,
        })],
    )
    .await;
    fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect("a hook-supplied stop is a deny");
    assert_eq!(fixture.answered.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.turn_cancels.load(Ordering::SeqCst), 0);
    {
        let answers = fixture.answers.lock().unwrap();
        let refusal = hook_refusal(&answers[0]);
        assert_eq!(refusal.reason(), "hook failed: error");
        assert_eq!(refusal.index(), Some(0));
        assert_eq!(refusal.outcome(), &RefusalOutcome::Continue);
    }
    assert!(fixture.closes.lock().unwrap().is_empty());
    fixture
        .agent
        .invoke(second_request(), close_action())
        .await
        .expect("the session stays usable");
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
    assert_eq!(fixture.answered.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.cancelled.load(Ordering::SeqCst), 1);
    {
        let withdrawals = fixture.withdrawals.lock().unwrap();
        assert_eq!(
            withdrawals[0].actor.principal_id(),
            "permission-review-hook"
        );
        assert_eq!(withdrawals[0].actor.surface_id(), "0");
        match withdrawals[0].reason.view() {
            PermissionCancellationReasonView::Custom(reason) => {
                assert_eq!(reason.code(), "hook-refusal.close_session");
                assert_eq!(reason.explanation(), "stop the session");
            }
            other => panic!("withdrawal reason was {other:?}"),
        }
    }
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
    let mut published = fixture.agent.subscribe();
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
    assert_eq!(
        fixture.answered.load(Ordering::SeqCst),
        0,
        "a stop answered the review"
    );
    let mut saw_review = false;
    loop {
        match tokio::time::timeout(std::time::Duration::from_millis(50), published.next()).await {
            Ok(Ok(Some(event)))
                if matches!(event.update(), ExecutionUpdate::PermissionRequested { .. }) =>
            {
                saw_review = true;
            }
            Ok(Ok(Some(_))) => {}
            _ => break,
        }
    }
    assert!(
        !saw_review,
        "a stop published the review for a person to answer"
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hook_past_the_deadline_is_deny_and_continue() {
    let entered = Arc::new(AtomicBool::new(false));
    let fixture = agent_with(
        script(),
        vec![Arc::new(SlowToolHook {
            entered,
            hold: std::time::Duration::from_secs(4),
        })],
    )
    .await;
    let started = std::time::Instant::now();
    fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect("timeout is deny-and-continue");
    let elapsed = started.elapsed();
    assert_eq!(
        fixture.answered.load(Ordering::SeqCst),
        1,
        "a hook that overran the deadline was not answered as a deny"
    );
    assert!(
        elapsed < std::time::Duration::from_millis(3500),
        "turn waited {elapsed:?}"
    );
    {
        let answers = fixture.answers.lock().unwrap();
        let refusal = hook_refusal(&answers[0]);
        assert_eq!(refusal.reason(), "hook failed: timeout");
        assert_eq!(refusal.index(), None);
        assert_eq!(answers[0].attribution.actor().surface_id(), "runtime");
    }
    fixture
        .agent
        .invoke(second_request(), close_action())
        .await
        .expect("the session stays usable");
}

struct ManualMessageClock(tokio::sync::watch::Sender<std::time::Duration>);

impl ManualMessageClock {
    fn new() -> Self {
        Self(tokio::sync::watch::channel(std::time::Duration::ZERO).0)
    }

    fn advance(&self, duration: std::time::Duration) {
        let next = *self.0.borrow() + duration;
        self.0.send_replace(next);
    }
}

impl MessageCommitClock for ManualMessageClock {
    fn now(&self) -> std::time::Duration {
        *self.0.borrow()
    }

    fn sleep_until(&self, deadline: std::time::Duration) -> MessageCommitSleep {
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

struct ReleasableHook {
    entered: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
}

impl InvocationHook for ReleasableHook {
    fn before_permission_review(
        &self,
        _: &ProposedToolCall<'_>,
        _: &InvocationObservationWindow<'_>,
    ) -> Result<ToolVerdict, HookError> {
        self.entered.store(true, Ordering::SeqCst);
        while !self.release.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Ok(ToolVerdict::Allow)
    }
}

struct ReleaseOnDrop(Arc<AtomicBool>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_substituted_message_clock_times_out_a_slow_hook() {
    let clock = Arc::new(ManualMessageClock::new());
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let _release = ReleaseOnDrop(release.clone());
    let fixture = open_on_clock(
        script(),
        vec![Arc::new(ReleasableHook {
            entered: entered.clone(),
            release,
        })],
        FixtureKnobs {
            turn_cancel: true,
            fail_delivery: false,
            fail_turn_cancel: false,
        },
        Some(clock.clone()),
    )
    .await;
    let runner = fixture.agent.clone();
    let turn = tokio::spawn(async move { runner.invoke(request(), close_action()).await });
    let wait_started = std::time::Instant::now();
    while !entered.load(Ordering::SeqCst) {
        assert!(
            wait_started.elapsed() < std::time::Duration::from_secs(2),
            "the hook never started"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let started = std::time::Instant::now();
    clock.advance(std::time::Duration::from_secs(2));
    let finished = tokio::time::timeout(std::time::Duration::from_millis(800), turn).await;
    let elapsed = started.elapsed();
    let turn_result = finished
        .expect("the substituted clock did not time the hook out")
        .unwrap();
    turn_result.expect("timeout is deny-and-continue");
    assert!(
        elapsed < std::time::Duration::from_secs(1),
        "turn waited {elapsed:?} of real time for a two-second message-commit deadline"
    );
    {
        let answers = fixture.answers.lock().unwrap();
        let refusal = hook_refusal(&answers[0]);
        assert_eq!(refusal.reason(), "hook failed: timeout");
        assert_eq!(refusal.index(), None);
        assert_eq!(answers[0].attribution.actor().surface_id(), "runtime");
    }
}

/// Refuses every review, but says it does not review permissions.
struct OptedOutRefuser;

impl InvocationHook for OptedOutRefuser {
    fn before_permission_review(
        &self,
        _: &ProposedToolCall<'_>,
        _: &InvocationObservationWindow<'_>,
    ) -> Result<ToolVerdict, HookError> {
        Ok(ToolVerdict::refuse(
            "not mine to refuse",
            RefusalOutcome::Continue,
        ))
    }

    fn reviews_permissions(&self) -> bool {
        false
    }
}

#[tokio::test]
async fn hooks_that_do_not_review_leave_the_review_to_the_person() {
    let fixture = open_on_clock(
        script(),
        vec![
            BeforeInvocation.register(|_context: &InvocationContext<'_>| Ok(())),
            AfterInvocation.register(|_event: &AfterInvocationEvent<'_>| Ok(())),
            Arc::new(OptedOutRefuser),
        ],
        FixtureKnobs {
            turn_cancel: true,
            fail_delivery: false,
            fail_turn_cancel: false,
        },
        None,
    )
    .await;
    fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect("the turn runs as it would with no hooks");
    assert_eq!(
        fixture.answered.load(Ordering::SeqCst),
        0,
        "the runtime refused a review that no hook reviews"
    );
    assert!(fixture.withdrawals.lock().unwrap().is_empty());
    assert!(review_still_pending(&fixture, "review"));
}

/// Reviews with the default verdict, which admits every call.
struct AllowingReviewer;

impl InvocationHook for AllowingReviewer {}

#[tokio::test]
async fn a_hook_that_does_not_review_is_skipped_beside_one_that_does() {
    let fixture = open_on_clock(
        script(),
        vec![Arc::new(OptedOutRefuser), Arc::new(AllowingReviewer)],
        FixtureKnobs {
            turn_cancel: true,
            fail_delivery: false,
            fail_turn_cancel: false,
        },
        None,
    )
    .await;
    fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect("the reviewing hook admits the call");
    assert_eq!(
        fixture.answered.load(Ordering::SeqCst),
        0,
        "a hook that does not review refused the call"
    );
    assert!(fixture.withdrawals.lock().unwrap().is_empty());
    assert!(review_still_pending(&fixture, "review"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn output_before_a_review_is_saved_while_its_hook_runs() {
    let clock = Arc::new(ManualMessageClock::new());
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let _release = ReleaseOnDrop(release.clone());
    let fixture = open_on_clock(
        vec![message(false, "SAVED_BEFORE_REVIEW"), review()],
        vec![Arc::new(ReleasableHook {
            entered: entered.clone(),
            release: release.clone(),
        })],
        FixtureKnobs {
            turn_cancel: true,
            fail_delivery: false,
            fail_turn_cancel: false,
        },
        Some(clock),
    )
    .await;
    let runner = fixture.agent.clone();
    let turn = tokio::spawn(async move { runner.invoke(request(), close_action()).await });
    let wait_started = std::time::Instant::now();
    while !entered.load(Ordering::SeqCst) {
        assert!(
            wait_started.elapsed() < std::time::Duration::from_secs(2),
            "the hook never started"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let saved = format!("{:?}", fixture.storage.snapshot()).contains("SAVED_BEFORE_REVIEW");
    release.store(true, Ordering::SeqCst);
    turn.await
        .unwrap()
        .expect("the allowed review finishes the turn");
    assert!(
        saved,
        "output before the review waited behind the hook for its commit deadline"
    );
}

#[tokio::test]
async fn a_hook_refusal_leaves_evidence_of_who_refused_and_why() {
    let fixture = agent_with(
        script(),
        vec![Arc::new(ReviewHook {
            seen: Arc::new(Mutex::new(Vec::new())),
            verdict: ToolVerdict::refuse("UNIQUE_HOOK_REASON", RefusalOutcome::Continue),
        })],
    )
    .await;
    fixture
        .agent
        .invoke(request(), close_action())
        .await
        .unwrap();
    // The session snapshot does not store an approval basis. The answer the
    // adapter received is the refusal evidence.
    let answers = fixture.answers.lock().unwrap();
    let refusal = hook_refusal(&answers[0]);
    assert_eq!(refusal.reason(), "UNIQUE_HOOK_REASON");
    assert_eq!(refusal.index(), Some(0));
    assert_eq!(refusal.outcome(), &RefusalOutcome::Continue);
    assert_eq!(
        answers[0].attribution.actor().principal_id(),
        "permission-review-hook"
    );
    assert_ne!(
        answers[0].attribution.actor().principal_id(),
        close_action().principal_id()
    );
}

fn named_review(id: &str, tool: &str) -> ExecutionEvent {
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
            id: PermissionId::new(id).unwrap(),
            tool_id: ToolCallId::new(tool).unwrap(),
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
async fn an_end_turn_the_binding_cannot_cancel_is_recorded_as_continue() {
    let fixture = open_fixture(
        script(),
        vec![Arc::new(ReviewHook {
            seen: Arc::new(Mutex::new(Vec::new())),
            verdict: ToolVerdict::refuse("stop the turn", RefusalOutcome::EndTurn),
        })],
        FixtureKnobs {
            turn_cancel: false,
            fail_delivery: false,
            fail_turn_cancel: false,
        },
    )
    .await;
    fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect("an end-turn the binding cannot perform continues");
    assert_eq!(fixture.turn_cancels.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.answered.load(Ordering::SeqCst), 1);
    assert_eq!(
        hook_refusal(&fixture.answers.lock().unwrap()[0]).outcome(),
        &RefusalOutcome::Continue
    );
    fixture
        .agent
        .invoke(second_request(), close_action())
        .await
        .expect("the session stays usable");
}

#[tokio::test]
async fn a_continue_without_a_deny_option_stays_open_when_the_binding_cannot_cancel() {
    let mut events = script();
    events.pop();
    events.push(allow_only_review());
    let fixture = open_fixture(
        events,
        vec![Arc::new(ReviewHook {
            seen: Arc::new(Mutex::new(Vec::new())),
            verdict: ToolVerdict::refuse("no", RefusalOutcome::Continue),
        })],
        FixtureKnobs {
            turn_cancel: false,
            fail_delivery: false,
            fail_turn_cancel: false,
        },
    )
    .await;
    fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect("withdrawing the review leaves the session open");
    assert_eq!(fixture.answered.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.cancelled.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.turn_cancels.load(Ordering::SeqCst), 0);
    {
        let withdrawals = fixture.withdrawals.lock().unwrap();
        assert_eq!(
            withdrawals[0].actor.principal_id(),
            "permission-review-hook"
        );
        match withdrawals[0].reason.view() {
            PermissionCancellationReasonView::Custom(reason) => {
                assert_eq!(reason.code(), "hook-refusal.continue");
            }
            other => panic!("withdrawal reason was {other:?}"),
        }
    }
    assert!(!review_still_pending(&fixture, "review"));
    assert!(fixture.closes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_failed_hook_denial_ends_the_turn_and_leaves_no_review_pending() {
    let fixture = open_fixture(
        script(),
        vec![Arc::new(ReviewHook {
            seen: Arc::new(Mutex::new(Vec::new())),
            verdict: ToolVerdict::refuse("stop this call", RefusalOutcome::Continue),
        })],
        FixtureKnobs {
            turn_cancel: true,
            fail_delivery: true,
            fail_turn_cancel: false,
        },
    )
    .await;
    let failure = fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect_err("a failed denial is reported");
    let AgentError::ExecutionObservation {
        error,
        execution_result,
    } = &failure
    else {
        panic!("a failed denial was reported as {failure:?}");
    };
    assert_eq!(**error, AgentError::StalePermission);
    assert!(
        matches!(execution_result.as_deref(), Some(Ok(_))),
        "the provider result is kept beside the failure: {execution_result:?}"
    );
    assert_eq!(fixture.answered.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.turn_cancels.load(Ordering::SeqCst), 1);
    assert_eq!(
        hook_refusal(&fixture.answers.lock().unwrap()[0]).outcome(),
        &RefusalOutcome::Continue,
        "a failed delivery still records the outcome that was intended"
    );
    assert!(fixture.closes.lock().unwrap().is_empty());
    assert_eq!(
        fixture.agent.pending_permission(
            &ExecutionId::new("turn").unwrap(),
            &PermissionId::new("review").unwrap(),
        ),
        Ok(false)
    );
    assert!(
        !review_still_pending(&fixture, "review"),
        "the failed denial left the review pending"
    );
    fixture
        .agent
        .invoke(second_request(), close_action())
        .await
        .expect("the session stays usable");
}

#[tokio::test]
async fn a_failed_denial_and_a_failed_turn_cancel_are_both_reported() {
    let fixture = open_fixture(
        script(),
        vec![Arc::new(ReviewHook {
            seen: Arc::new(Mutex::new(Vec::new())),
            verdict: ToolVerdict::refuse("stop this call", RefusalOutcome::Continue),
        })],
        FixtureKnobs {
            turn_cancel: true,
            fail_delivery: true,
            fail_turn_cancel: true,
        },
    )
    .await;
    let failure = fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect_err("both failures are reported");
    let AgentError::ExecutionObservation { error, .. } = &failure else {
        panic!("both failures were reported as {failure:?}");
    };
    let AgentError::MultipleOperationFailures {
        first_error,
        subsequent_error,
    } = error.as_ref()
    else {
        panic!("both failures were reported as {error:?}");
    };
    assert_eq!(
        **first_error,
        AgentError::StalePermission,
        "delivery is first"
    );
    assert_eq!(
        **subsequent_error,
        AgentError::Transport("turn cancel failed".into())
    );
    assert_eq!(fixture.turn_cancels.load(Ordering::SeqCst), 1);
    assert!(!fixture.closes.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_review_does_not_start_another_hook_thread() {
    let entered = Arc::new(AtomicBool::new(false));
    let fixture = agent_with(
        vec![
            message(false, "hello"),
            review(),
            named_review("review-2", "bash-2"),
        ],
        vec![Arc::new(SlowToolHook {
            entered,
            hold: std::time::Duration::from_secs(4),
        })],
    )
    .await;
    let started = std::time::Instant::now();
    fixture
        .agent
        .invoke(request(), close_action())
        .await
        .expect("a busy hook refuses the next review immediately");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(3500),
        "the second review waited for another hook thread"
    );
    assert_eq!(fixture.answered.load(Ordering::SeqCst), 2);
    let answers = fixture.answers.lock().unwrap();
    assert_eq!(hook_refusal(&answers[0]).reason(), "hook failed: timeout");
    assert_eq!(hook_refusal(&answers[0]).index(), None);
    assert_eq!(hook_refusal(&answers[1]).reason(), "hook failed: busy");
    assert_eq!(hook_refusal(&answers[1]).index(), None);
    assert_eq!(answers[1].attribution.actor().surface_id(), "runtime");
}
