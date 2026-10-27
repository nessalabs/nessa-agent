use super::super::agents::MemoryStorage;
use super::super::support::*;
use nessa_sdk::application::agent_execution::hooks::{
    AfterInvocation, AfterInvocationEvent, BeforeInvocation, BeforePermissionReview, HookError,
    HookRegistration, InvocationContext, InvocationHook, InvocationObservationWindow,
    ObservationKind, PhrasePresence, ProposedToolCall, ToolVerdict,
};
use nessa_sdk::application::agent_execution::providers::HarnessHostIdentity;
use nessa_sdk::application::agent_execution::providers::NativeExecutionHandoff;
use nessa_sdk::domain::agent_execution::executions::{GrantInitiator, Initiation};
use nessa_sdk::domain::agent_execution::permissions::RefusalOutcome;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Set once the provider's execution has admitted its reviews.
#[derive(Default)]
struct Started {
    flag: AtomicBool,
    signal: tokio::sync::Notify,
}

impl Started {
    fn set(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.signal.notify_waiters();
    }

    async fn wait(&self) {
        loop {
            // Register before checking, so a set in between is not lost.
            let signalled = self.signal.notified();
            if self.flag.load(Ordering::SeqCst) {
                return;
            }
            signalled.await;
        }
    }
}

struct ScriptedEvents {
    events: Vec<ExecutionEvent>,
    started: Arc<Started>,
    release: Arc<tokio::sync::Notify>,
    index: usize,
}

impl ExecutionEventStream for ScriptedEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async move {
            self.started.wait().await;
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

/// A provider whose withdrawal completes only after its execution handles the
/// reply, as an adapter with one reader for both does.
#[derive(Default)]
struct Handshake {
    required: AtomicBool,
    /// A successful withdrawal also lets the execution settle.
    settles_execution: AtomicBool,
    request: tokio::sync::Notify,
    reply: tokio::sync::Notify,
}

struct ScriptedBackend {
    started: Arc<Started>,
    /// The session state a failed delivery or turn cancel reports.
    failure_state: Arc<Mutex<ProviderSessionState>>,
    handshake: Arc<Handshake>,
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
                None,
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
    fn execute(
        &self,
        input: ExecutionRequest,
        handoff: NativeExecutionHandoff,
    ) -> ProviderExecutionFuture<'_> {
        let started = self.started.clone();
        let controller = self.controller.clone();
        let reviews = self.reviews.clone();
        let release = self.release.clone();
        let handshake = self.handshake.clone();
        Box::pin(async move {
            if let Err(error) = handoff() {
                return ProviderExecutionReply::Rejected(error);
            }

            // Register for release before the stream can run to its end.
            let released = release.notified();
            tokio::pin!(released);
            admit_reviews(&controller, &input.execution_id, &reviews);
            started.set();
            loop {
                tokio::select! {
                    _ = &mut released => break,
                    // A control that needs the execution polled is acknowledged
                    // only while something polls it.
                    _ = handshake.request.notified() => handshake.reply.notify_one(),
                }
            }
            if handshake.settles_execution.load(Ordering::SeqCst) {
                // The reply is ready one poll after the release, as when a
                // provider hands it over through its own task.
                tokio::task::yield_now().await;
            }
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
        let failure_state = self.failure_state.lock().unwrap().clone();
        let release = self.release.clone();
        Box::pin(async move {
            answered.fetch_add(1, Ordering::SeqCst);
            answers
                .lock()
                .expect("fixture answers")
                .push(answer.clone());
            if fail {
                // A failure that reports cleanup has ended the session, and
                // its execution with it.
                if failure_state != ProviderSessionState::Usable {
                    release.notify_waiters();
                }
                return Err(ProviderOperationFailure::permission_answer(
                    AgentError::StalePermission,
                    failure_state,
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
        let failure_state = self.failure_state.lock().unwrap().clone();
        let release = self.release.clone();
        let handshake = self.handshake.clone();
        Box::pin(async move {
            cancelled.fetch_add(1, Ordering::SeqCst);
            if handshake.required.load(Ordering::SeqCst) {
                handshake.request.notify_one();
                handshake.reply.notified().await;
            }
            withdrawals
                .lock()
                .expect("fixture withdrawals")
                .push(input.clone());
            if fail {
                // A failure that reports cleanup has ended the session, and
                // its execution with it.
                if failure_state != ProviderSessionState::Usable {
                    release.notify_waiters();
                }
                return Err(ProviderOperationFailure::new(
                    AgentError::StalePermission,
                    failure_state,
                ));
            }
            let withdrawn = controller
                .lock()
                .expect("fixture controller")
                .cancel_review(input)
                .map_err(|error| {
                    ProviderOperationFailure::new(error, ProviderSessionState::Usable)
                });
            if handshake.settles_execution.load(Ordering::SeqCst) {
                release.notify_waiters();
            }
            withdrawn
        })
    }
    fn cancel_turn(&self, turn: ExecutionId) -> ProviderOperationFuture<'_, ()> {
        let turn_cancels = self.turn_cancels.clone();
        let controller = self.controller.clone();
        let reviews = self.reviews.clone();
        let release = self.release.clone();
        let fail = self.fail_turn_cancel;
        let failure_state = self.failure_state.lock().unwrap().clone();
        Box::pin(async move {
            turn_cancels.fetch_add(1, Ordering::SeqCst);
            if fail {
                // A failure that reports cleanup has ended the session, and
                // its execution with it.
                if failure_state != ProviderSessionState::Usable {
                    release.notify_waiters();
                }
                return Err(ProviderOperationFailure::new(
                    AgentError::Transport("turn cancel failed".into()),
                    failure_state,
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
    handshake: Arc<Handshake>,
    events: Mutex<Option<Vec<ExecutionEvent>>>,
    started: Arc<Started>,
    failure_state: Arc<Mutex<ProviderSessionState>>,
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
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        HarnessHostIdentity::local()
    }
    /// Ask, so a grant's checked turn may be sent here.
    fn approval_mode(
        &self,
    ) -> Option<nessa_sdk::application::agent_execution::providers::ApprovalMode> {
        Some(nessa_sdk::application::agent_execution::providers::ApprovalMode::Ask)
    }
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
                        failure_state: self.failure_state.clone(),
                        handshake: self.handshake.clone(),
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
            deadline: None,
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
    /// Lets the scripted provider settle its execution.
    release: Arc<tokio::sync::Notify>,
    failure_state: Arc<Mutex<ProviderSessionState>>,
    handshake: Arc<Handshake>,
    /// Set once the provider is asked to execute a turn.
    started: Arc<Started>,
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
    let release = Arc::new(tokio::sync::Notify::new());
    let failure_state = Arc::new(Mutex::new(ProviderSessionState::Usable));
    let handshake = Arc::new(Handshake::default());
    let started = Arc::new(Started::default());
    let agent = attached_agent(
        Arc::new(ScriptedProvider {
            events: Mutex::new(Some(events)),
            started: started.clone(),
            failure_state: failure_state.clone(),
            handshake: handshake.clone(),
            answered: answered.clone(),
            cancelled: cancelled.clone(),
            turn_cancels: turn_cancels.clone(),
            closes: closes.clone(),
            answers: answers.clone(),
            withdrawals: withdrawals.clone(),
            controller: Arc::new(Mutex::new(controller)),
            authority: authority.clone(),
            release: release.clone(),
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
        release,
        failure_state,
        handshake,
        started,
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
        .invoke(request(), Submitter::owner(close_action()))
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
        .invoke(request(), Submitter::owner(close_action()))
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
        .invoke(request(), Submitter::owner(close_action()))
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
        .invoke(second_request(), Submitter::owner(close_action()))
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
        .invoke(request(), Submitter::owner(close_action()))
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
        .invoke(request(), Submitter::owner(close_action()))
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
        .invoke(second_request(), Submitter::owner(close_action()))
        .await
        .expect("the session stays usable");
}

#[tokio::test]
async fn an_end_turn_refusal_does_not_close_the_session() {
    if let Some(log) = isolated_warn_log("an_end_turn_refusal_does_not_close_the_session") {
        assert!(
            log.contains("a hook refused a tool and stopped the turn"),
            "{log}"
        );
        assert!(log.contains("stop the turn"), "{log}");
        return;
    }
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
        .invoke(request(), Submitter::owner(close_action()))
        .await
        .expect("ending the turn leaves the session open");
    // An end-turn withdraws the review rather than answering it, so no answer
    // carries a terminal outcome an adapter could replay.
    assert_eq!(fixture.answered.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.cancelled.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.turn_cancels.load(Ordering::SeqCst), 1);
    {
        let withdrawals = fixture.withdrawals.lock().unwrap();
        assert_eq!(
            withdrawals[0].actor.principal_id(),
            "permission-review-hook"
        );
        match withdrawals[0].reason.view() {
            PermissionCancellationReasonView::Custom(reason) => {
                assert_eq!(reason.code(), "hook-refusal.end_turn");
                assert_eq!(reason.explanation(), "stop the turn");
            }
            other => panic!("withdrawal reason was {other:?}"),
        }
    }
    assert!(!review_still_pending(&fixture, "review"));
    assert!(fixture.closes.lock().unwrap().is_empty());
    fixture
        .agent
        .invoke(second_request(), Submitter::owner(close_action()))
        .await
        .expect("the session stays usable");
}

#[tokio::test]
async fn a_hook_error_denies_and_continues() {
    let fixture = agent_with(script(), vec![Arc::new(FailingHook)]).await;
    fixture
        .agent
        .invoke(request(), Submitter::owner(close_action()))
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
        .invoke(second_request(), Submitter::owner(close_action()))
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
            deadline: None,
        },
    )
}

#[tokio::test]
async fn a_panicked_review_hook_denies_and_continues() {
    let fixture = agent_with(script(), vec![Arc::new(PanicHook)]).await;
    fixture
        .agent
        .invoke(request(), Submitter::owner(close_action()))
        .await
        .expect("a panicked hook does not close the session");
    assert_eq!(fixture.answered.load(Ordering::SeqCst), 1);
    assert_eq!(
        hook_refusal(&fixture.answers.lock().unwrap()[0]).reason(),
        "hook failed: panic"
    );
    fixture
        .agent
        .invoke(second_request(), Submitter::owner(close_action()))
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

/// Panics when asked whether it reviews, and would otherwise allow.
struct PanickingParticipant;

impl InvocationHook for PanickingParticipant {
    fn reviews_permissions(&self) -> bool {
        panic!("participation bug");
    }
}

#[tokio::test]
async fn a_hook_that_panics_on_participation_refuses_the_review() {
    let fixture = agent_with(script(), vec![Arc::new(PanickingParticipant)]).await;
    fixture
        .agent
        .invoke(request(), Submitter::owner(close_action()))
        .await
        .expect("a panicked hook does not close the session");
    assert_eq!(fixture.answered.load(Ordering::SeqCst), 1);
    let answers = fixture.answers.lock().unwrap();
    let refusal = hook_refusal(&answers[0]);
    assert_eq!(refusal.reason(), "hook failed: panic");
    assert_eq!(refusal.index(), Some(0));
}

/// Registers another hook on the same agent, from another thread, while
/// `reviews_permissions` runs, and records whether that registration had to wait.
struct RegisteringHook {
    agent: Arc<Mutex<Option<Agent>>>,
    blocked: Arc<AtomicBool>,
    registered: Arc<AtomicBool>,
}

impl InvocationHook for RegisteringHook {
    fn reviews_permissions(&self) -> bool {
        let agent = self.agent.lock().unwrap().clone();
        if let Some(agent) = agent {
            let (done, registered) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                agent.add_invocation_hook(Arc::new(AllowingReviewer));
                let _ = done.send(());
            });
            match registered.recv_timeout(std::time::Duration::from_secs(2)) {
                Ok(()) => self.registered.store(true, Ordering::SeqCst),
                Err(_) => self.blocked.store(true, Ordering::SeqCst),
            }
        }
        false
    }
}

#[tokio::test]
async fn hook_code_runs_after_the_hook_registry_is_released() {
    let agent = Arc::new(Mutex::new(None));
    let blocked = Arc::new(AtomicBool::new(false));
    let registered = Arc::new(AtomicBool::new(false));
    let fixture = agent_with(
        script(),
        vec![Arc::new(RegisteringHook {
            agent: agent.clone(),
            blocked: blocked.clone(),
            registered: registered.clone(),
        })],
    )
    .await;
    *agent.lock().unwrap() = Some(fixture.agent.clone());
    let invoked = fixture
        .agent
        .invoke(request(), Submitter::owner(close_action()))
        .await;
    // Break the agent -> hook -> agent cycle so the agent is dropped.
    agent.lock().unwrap().take();
    invoked.expect("the turn runs");
    assert!(
        registered.load(Ordering::SeqCst),
        "the hook did not register another hook during the invocation"
    );
    assert!(
        !blocked.load(Ordering::SeqCst),
        "hook code ran while the invocation held the hook registry"
    );
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
        .invoke(request(), Submitter::owner(close_action()))
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
        .invoke(second_request(), Submitter::owner(close_action()))
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
    let _ = fixture
        .agent
        .invoke(request(), Submitter::owner(close_action()))
        .await;
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
    let second = fixture
        .agent
        .invoke(second_request(), Submitter::owner(close_action()))
        .await;
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
    let turn = tokio::spawn(async move {
        runner
            .invoke(request(), Submitter::owner(close_action()))
            .await
    });
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
        .invoke(request(), Submitter::owner(close_action()))
        .await
        .expect("the refused call does not fail the turn");
    fixture
        .agent
        .invoke(second_request(), Submitter::owner(close_action()))
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
        .invoke(request(), Submitter::owner(close_action()))
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
        .invoke(second_request(), Submitter::owner(close_action()))
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

    fn unix_milliseconds(&self) -> u64 {
        self.now().as_millis() as u64
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
    let turn = tokio::spawn(async move {
        runner
            .invoke(request(), Submitter::owner(close_action()))
            .await
    });
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

/// A [`ReleasableHook`] that reports when it is about to return.
struct ReportingHook {
    inner: ReleasableHook,
    returning: Arc<AtomicBool>,
}

impl InvocationHook for ReportingHook {
    fn before_permission_review(
        &self,
        call: &ProposedToolCall<'_>,
        observations: &InvocationObservationWindow<'_>,
    ) -> Result<ToolVerdict, HookError> {
        let verdict = self.inner.before_permission_review(call, observations);
        self.returning.store(true, Ordering::SeqCst);
        verdict
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_verdict_first_seen_after_the_deadline_is_a_timeout() {
    let clock = Arc::new(ManualMessageClock::new());
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let returning = Arc::new(AtomicBool::new(false));
    let _release = ReleaseOnDrop(release.clone());
    let fixture = open_on_clock(
        script(),
        vec![Arc::new(ReportingHook {
            inner: ReleasableHook {
                entered: entered.clone(),
                release: release.clone(),
            },
            returning: returning.clone(),
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
    let turn = tokio::spawn(async move {
        runner
            .invoke(request(), Submitter::owner(close_action()))
            .await
    });
    let wait_started = std::time::Instant::now();
    while !entered.load(Ordering::SeqCst) {
        assert!(
            wait_started.elapsed() < std::time::Duration::from_secs(2),
            "the hook never started"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    // Pass the deadline and let the hook allow, while this test holds the
    // only runtime thread, so the supervisor next sees both at once.
    clock.advance(std::time::Duration::from_secs(2));
    release.store(true, Ordering::SeqCst);
    let returned_by = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !returning.load(Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < returned_by,
            "the hook never returned"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // The hook has its verdict; give its blocking task time to complete.
    std::thread::sleep(std::time::Duration::from_millis(100));
    turn.await.unwrap().expect("timeout is deny-and-continue");
    let answers = fixture.answers.lock().unwrap();
    assert_eq!(answers.len(), 1, "a late allow was accepted");
    assert_eq!(hook_refusal(&answers[0]).reason(), "hook failed: timeout");
}

/// Runs this test once in a child process with a process-wide warn subscriber,
/// and returns that process's stderr. `None` in the child, which then runs
/// the test. Worker threads do not see a thread-local subscriber, and the
/// hook stop is logged from one.
fn isolated_warn_log(test: &str) -> Option<String> {
    const CHILD: &str = "NESSA_HOOK_STOP_LOG";
    if std::env::var_os(CHILD).is_some() {
        tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .try_init()
            .expect("the child process has no subscriber yet");
        return None;
    }
    let name = format!("{}::{test}", module_path!().split_once("::").unwrap().1);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &name, "--test-threads=1"])
        .env(CHILD, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "child {name} failed\n{stdout}\n{stderr}"
    );
    Some(format!("{stdout}\n{stderr}"))
}

/// Lets the provider settle its execution, waits for that to be seen, then
/// ends the turn.
struct SettlingRefuser(Arc<tokio::sync::Notify>);

impl InvocationHook for SettlingRefuser {
    fn before_permission_review(
        &self,
        _: &ProposedToolCall<'_>,
        _: &InvocationObservationWindow<'_>,
    ) -> Result<ToolVerdict, HookError> {
        self.0.notify_waiters();
        std::thread::sleep(std::time::Duration::from_millis(300));
        Ok(ToolVerdict::refuse("stop", RefusalOutcome::EndTurn))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_end_turn_after_the_provider_settled_keeps_the_session() {
    if let Some(log) = isolated_warn_log("an_end_turn_after_the_provider_settled_keeps_the_session")
    {
        assert!(
            !log.contains("a hook refused a tool and stopped the turn"),
            "{log}"
        );
        return;
    }
    let fixture = agent_with(script(), Vec::new()).await;
    fixture
        .agent
        .add_invocation_hook(Arc::new(SettlingRefuser(fixture.release.clone())));
    fixture
        .agent
        .invoke(request(), Submitter::owner(close_action()))
        .await
        .expect("a turn that already ended needs no cancel");
    assert_eq!(fixture.turn_cancels.load(Ordering::SeqCst), 0);
    assert!(fixture.closes.lock().unwrap().is_empty());
    fixture
        .agent
        .invoke(second_request(), Submitter::owner(close_action()))
        .await
        .expect("the session stays usable");
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
        .invoke(request(), Submitter::owner(close_action()))
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
        .invoke(request(), Submitter::owner(close_action()))
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
    let turn = tokio::spawn(async move {
        runner
            .invoke(request(), Submitter::owner(close_action()))
            .await
    });
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
        .invoke(request(), Submitter::owner(close_action()))
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

/// Records who started the turn each reviewed call belongs to.
struct InitiationHook {
    seen: Arc<Mutex<Vec<Initiation>>>,
}

impl InvocationHook for InitiationHook {
    fn before_permission_review(
        &self,
        call: &ProposedToolCall<'_>,
        _observations: &InvocationObservationWindow<'_>,
    ) -> Result<ToolVerdict, HookError> {
        self.seen.lock().unwrap().push(call.initiation().clone());
        Ok(ToolVerdict::Allow)
    }
}

/// The review hook is told who started the turn, from the turn's saved record,
/// whether the turn ran at once or waited in the queue.
#[tokio::test]
async fn a_review_hook_sees_who_started_the_turn() {
    let grant = Initiation::checked_grant(GrantInitiator::new("receiver", 7).unwrap());
    for (queued, initiation) in [
        (false, Initiation::owner()),
        (false, grant.clone()),
        (true, grant.clone()),
    ] {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let fixture = agent_with(
            script(),
            vec![Arc::new(InitiationHook { seen: seen.clone() })],
        )
        .await;
        let submitter = Submitter::new(close_action(), initiation.clone());
        if queued {
            let admission = fixture.agent.enqueue(request(), submitter).await.unwrap();
            admission.wait().await.unwrap();
        } else {
            fixture.agent.invoke(request(), submitter).await.unwrap();
        }
        assert_eq!(*seen.lock().unwrap(), [initiation], "queued: {queued}");
    }
}

/// A grant's turn saved before its tools were checked, as one restored from
/// an older save is, is never sent: nothing would check its calls.
#[tokio::test]
async fn a_grant_turn_not_saved_as_checked_is_not_sent() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let fixture = agent_with(
        script(),
        vec![Arc::new(InitiationHook { seen: seen.clone() })],
    )
    .await;
    let submitter = Submitter::new(
        close_action(),
        Initiation::grant(GrantInitiator::new("receiver", 7).unwrap()),
    );
    let result = fixture.agent.invoke(request(), submitter).await;
    assert!(
        matches!(&result, Err(AgentError::Unsupported(reason)) if reason.contains("checked")),
        "{result:?}"
    );
    assert!(!fixture.started.flag.load(Ordering::SeqCst));
    assert!(seen.lock().unwrap().is_empty());
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
            deadline: None,
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
        .invoke(request(), Submitter::owner(close_action()))
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
        .invoke(second_request(), Submitter::owner(close_action()))
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
        .invoke(request(), Submitter::owner(close_action()))
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
async fn a_recorded_hook_withdrawal_cannot_withdraw_the_same_review_in_another_session() {
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
        .invoke(request(), Submitter::owner(close_action()))
        .await
        .unwrap();
    // The recorded reason is a value any adapter can copy from the evidence.
    let copied = fixture.withdrawals.lock().unwrap()[0].reason.clone();
    // Another session reuses the turn and review ids the hook refused.
    let execution = ExecutionId::new("turn").unwrap();
    let other = PermissionId::new("review").unwrap();
    let mut controller = ExecutionController::new(ExecutionSessionId::new("other").unwrap());
    controller.begin_execution(execution.clone()).unwrap();
    let tool = ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        None,
        None,
        None,
        None,
        None,
    );
    controller.tool_event(&execution, tool.clone()).unwrap();
    let deny = PermissionOption::new(
        PermissionOptionId::new("deny").unwrap(),
        "Deny",
        PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request()),
    )
    .unwrap();
    controller
        .request_permission(
            &execution,
            other.clone(),
            tool,
            ToolReviewInput {
                name: "Bash".into(),
                arguments_json: "{}".into(),
            },
            PermissionOptions::new(vec![deny], &PermissionOfferPolicy::once_only()).unwrap(),
            None,
        )
        .unwrap();
    let as_the_hook = CancellationOrigin::Client(
        ActionContext::new("permission-review-hook", "0", "review").unwrap(),
    );
    assert!(matches!(
        controller.cancel_permission(&execution, &other, copied.clone(), as_the_hook.clone()),
        Err(AgentError::InvalidInput(_))
    ));
    assert!(matches!(
        controller.cancel_permissions(&execution, copied, as_the_hook),
        Err(AgentError::InvalidInput(_))
    ));
    // The review stayed pending, so a caller's own reason still withdraws it.
    assert!(controller
        .cancel_permission(
            &execution,
            &other,
            PermissionCancellationReason::custom(
                CustomPermissionCancellationReason::new("guard", "guard withdrew").unwrap(),
            ),
            CancellationOrigin::Client(close_action()),
        )
        .unwrap()
        .is_some());
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
        .invoke(request(), Submitter::owner(close_action()))
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
        .invoke(second_request(), Submitter::owner(close_action()))
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
        .invoke(request(), Submitter::owner(close_action()))
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

#[tokio::test]
async fn a_failed_turn_cancel_that_reports_cleanup_is_still_reported() {
    let fixture = open_fixture(
        script(),
        vec![Arc::new(ReviewHook {
            seen: Arc::new(Mutex::new(Vec::new())),
            verdict: ToolVerdict::refuse("stop the turn", RefusalOutcome::EndTurn),
        })],
        FixtureKnobs {
            turn_cancel: true,
            fail_delivery: false,
            fail_turn_cancel: true,
        },
    )
    .await;
    // The failed cancel confirms cleanup, which ends the turn's activity.
    *fixture.failure_state.lock().unwrap() =
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        }));
    let failure = fixture
        .agent
        .invoke(request(), Submitter::owner(close_action()))
        .await
        .expect_err("a failed cancel is not mistaken for a settled turn");
    let AgentError::ExecutionObservation { error, .. } = &failure else {
        panic!("the cancel failure was reported as {failure:?}");
    };
    assert_eq!(**error, AgentError::Transport("turn cancel failed".into()));
    assert_eq!(fixture.turn_cancels.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_reply_that_arrives_during_a_failed_delivery_needs_no_cancel() {
    let fixture = open_fixture(
        script(),
        vec![Arc::new(ReviewHook {
            seen: Arc::new(Mutex::new(Vec::new())),
            verdict: ToolVerdict::refuse("stop the turn", RefusalOutcome::EndTurn),
        })],
        FixtureKnobs {
            turn_cancel: true,
            fail_delivery: true,
            fail_turn_cancel: false,
        },
    )
    .await;
    // The failed withdrawal confirms cleanup, which ends the execution: its
    // reply arrives while the withdrawal is still being awaited.
    *fixture.failure_state.lock().unwrap() =
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        }));
    let failure = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        fixture
            .agent
            .invoke(request(), Submitter::owner(close_action())),
    )
    .await
    .expect("the invocation settles")
    .expect_err("the failed delivery is reported");
    let AgentError::ExecutionObservation { error, .. } = &failure else {
        panic!("the delivery failure was reported as {failure:?}");
    };
    assert_eq!(**error, AgentError::StalePermission);
    assert_eq!(
        fixture.turn_cancels.load(Ordering::SeqCst),
        0,
        "the reply was seen during the delivery, so no turn was left to cancel"
    );
}

#[tokio::test]
async fn a_withdrawal_that_needs_the_execution_polled_completes() {
    let fixture = agent_with(
        script(),
        vec![Arc::new(ReviewHook {
            seen: Arc::new(Mutex::new(Vec::new())),
            verdict: ToolVerdict::refuse("stop the turn", RefusalOutcome::EndTurn),
        })],
    )
    .await;
    fixture.handshake.required.store(true, Ordering::SeqCst);
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        fixture
            .agent
            .invoke(request(), Submitter::owner(close_action())),
    )
    .await
    .expect("the execution is polled while the hook's withdrawal runs")
    .expect("the refused turn ends");
    assert_eq!(fixture.cancelled.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.turn_cancels.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_reply_seen_while_cancelling_keeps_the_session() {
    let fixture = agent_with(
        script(),
        vec![Arc::new(ReviewHook {
            seen: Arc::new(Mutex::new(Vec::new())),
            verdict: ToolVerdict::refuse("stop the turn", RefusalOutcome::EndTurn),
        })],
    )
    .await;
    // The withdrawal settles the execution, but its reply is first polled
    // while the turn cancel runs.
    fixture
        .handshake
        .settles_execution
        .store(true, Ordering::SeqCst);
    fixture
        .agent
        .invoke(request(), Submitter::owner(close_action()))
        .await
        .expect("a turn that settled on its own is not a cancel failure");
    assert_eq!(
        fixture.turn_cancels.load(Ordering::SeqCst),
        0,
        "the cancel was refused before reaching the provider"
    );
    assert!(
        fixture.closes.lock().unwrap().is_empty(),
        "the session was kept"
    );
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
        .invoke(request(), Submitter::owner(close_action()))
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

#[test]
fn a_refusal_reason_is_capped_when_the_verdict_is_built() {
    let max = nessa_sdk::application::agent_execution::permissions::HookRefusal::MAX_REASON_BYTES;
    let ToolVerdict::Refuse(refusal) =
        ToolVerdict::refuse("é".repeat(max * 64), RefusalOutcome::Continue)
    else {
        panic!("refuse builds a refusal");
    };
    assert!(refusal.reason().len() <= max);
    assert!(refusal.reason().len() > max - 2);
    assert!(refusal.reason().chars().all(|c| c == 'é'));
    let ToolVerdict::Refuse(blank) = ToolVerdict::refuse("  ", RefusalOutcome::Continue) else {
        panic!("refuse builds a refusal");
    };
    assert_eq!(blank.reason(), "hook refused");
}
