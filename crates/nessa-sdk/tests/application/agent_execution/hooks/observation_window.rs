use super::super::agents::MemoryStorage;
use super::super::support::*;
use nessa_sdk::application::agent_execution::hooks::{
    HookError, HookFailure, InvocationContext, InvocationHook, InvocationObservationWindow,
    PhrasePresence, ProposedToolCall, ThinkingDisclosure,
};
use std::sync::atomic::{AtomicBool, Ordering};

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
        Box::pin(async {
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
    fn close(&self, _: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async { CleanupReport::confirmed(CloseOutcome { forced: false }) })
    }
}

struct ScriptedProvider {
    events: Mutex<Option<Vec<ExecutionEvent>>>,
    started: Arc<AtomicBool>,
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
    disclosure: ThinkingDisclosure,
    call: Option<String>,
    split: PhrasePresence,
    secret: PhrasePresence,
    included: usize,
}

async fn agent_with(
    events: Vec<ExecutionEvent>,
    hooks: Vec<Arc<dyn InvocationHook>>,
) -> (Agent, MemoryStorage) {
    let storage = MemoryStorage::default();
    let agent = attached_agent(
        Arc::new(ScriptedProvider {
            events: Mutex::new(Some(events)),
            started: Arc::new(AtomicBool::new(false)),
        }),
        storage.manager().await,
    )
    .await
    .unwrap();
    for hook in hooks {
        agent.add_invocation_hook(hook);
    }
    (agent, storage)
}

fn request() -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new("turn").unwrap(),
        user_message: UserMessage::text_only(PromptText::new("hello").unwrap()),
        estimated_input_tokens: 1,
        reserved_output_tokens: 100,
    }
}

#[tokio::test]
async fn pre_tool_and_turn_boundary_share_the_window_and_the_thinking_opt_in() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let tool_seen = seen.clone();
    let after_seen = seen.clone();
    let disclosed_seen = seen.clone();
    let (agent, _) = agent_with(
        script(),
        vec![
            Arc::new(BeforeToolHook {
                seen: tool_seen,
                reject: false,
            }),
            Arc::new(AfterHook {
                seen: after_seen,
                disclose: false,
            }),
            Arc::new(AfterHook {
                seen: disclosed_seen,
                disclose: true,
            }),
        ],
    )
    .await;
    agent
        .invoke(request(), close_action())
        .await
        .expect("hook admitted the review");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[0].phase, "tool");
    assert_eq!(seen[0].call.as_deref(), Some("Bash"));
    assert_eq!(seen[0].disclosure, ThinkingDisclosure::Withheld);
    assert!(seen[0].split.is_present());
    assert!(seen[0].secret.is_absent());
    assert_eq!(seen[0].included, 2);
    assert_eq!(seen[1].phase, "after");
    assert_eq!(seen[1].disclosure, ThinkingDisclosure::Withheld);
    assert!(seen[1].split.is_present());
    assert!(seen[1].secret.is_absent());
    assert_eq!(seen[2].disclosure, ThinkingDisclosure::Disclosed);
    assert!(seen[2].secret.is_present());
    assert!(seen[2].split.is_present());
}

struct BeforeToolHook {
    seen: Arc<Mutex<Vec<Seen>>>,
    reject: bool,
}

impl InvocationHook for BeforeToolHook {
    fn before_tool_call(
        &self,
        call: &ProposedToolCall<'_>,
        observations: &InvocationObservationWindow<'_>,
    ) -> Result<(), HookError> {
        self.seen.lock().unwrap().push(Seen {
            phase: "tool",
            disclosure: observations.disclosure(),
            call: Some(call.name().to_owned()),
            split: observations.contains_phrase("do not run"),
            secret: observations.contains_phrase("SECRET_REASON"),
            included: observations.included_events(),
        });
        if self.reject {
            Err(HookError::Failed("stop this call".into()))
        } else {
            Ok(())
        }
    }
}

struct AfterHook {
    seen: Arc<Mutex<Vec<Seen>>>,
    disclose: bool,
}

impl InvocationHook for AfterHook {
    fn thinking_disclosure(&self) -> ThinkingDisclosure {
        if self.disclose {
            ThinkingDisclosure::Disclosed
        } else {
            ThinkingDisclosure::Withheld
        }
    }

    fn after_invocation(
        &self,
        context: &InvocationContext<'_>,
        _: &Result<ExecutionOutcome, AgentError>,
    ) -> Result<(), HookError> {
        let window = &context.observations;
        self.seen.lock().unwrap().push(Seen {
            phase: "after",
            disclosure: window.disclosure(),
            call: None,
            split: window.contains_phrase("do not run"),
            secret: window.contains_phrase("SECRET_REASON"),
            included: window.included_events(),
        });
        Ok(())
    }
}

#[tokio::test]
async fn a_pre_tool_rejection_stops_the_turn_without_admitting_the_review() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (agent, storage) = agent_with(
        script(),
        vec![Arc::new(BeforeToolHook {
            seen: seen.clone(),
            reject: true,
        })],
    )
    .await;
    let error = agent.invoke(request(), close_action()).await.unwrap_err();
    assert_eq!(
        rejection(&error),
        Some(HookFailure {
            index: 0,
            error: HookError::Failed("stop this call".into()),
        })
    );
    assert_eq!(seen.lock().unwrap().len(), 1);
    let saved = storage.snapshot();
    let events = &saved.invocations[0].events;
    assert!(events.iter().any(|event| matches!(
        event.update(),
        ExecutionUpdate::Message(chunk) if chunk.as_str() == "do not "
    )));
    assert!(events
        .iter()
        .all(|event| !matches!(event.update(), ExecutionUpdate::PermissionRequested { .. })));
}

fn rejection(error: &AgentError) -> Option<HookFailure> {
    match error {
        AgentError::BeforeInvocationHook(failure) => Some(failure.clone()),
        AgentError::ExecutionObservation { error, .. } => rejection(error),
        AgentError::OperationAndCleanupFailure {
            operation_error, ..
        } => rejection(operation_error),
        AgentError::StorageAfterExecution {
            execution_result, ..
        } => match execution_result.as_ref() {
            Err(error) => rejection(error),
            Ok(_) => None,
        },
        _ => None,
    }
}
