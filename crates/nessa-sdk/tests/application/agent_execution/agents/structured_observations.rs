//! Borrowed structured-event validation must precede any compacting clone.
use super::{actor, capabilities, request, MemoryStorage};
use crate::application::agent_execution::support::*;
use nessa_sdk::domain::agent_execution::{
    sessions::ExecutionSession,
    tools::{FileLocation, FilePath, ToolContent, ToolObservation},
};

const BUDGET: usize = 32 * 1024 * 1024;
struct PayloadProvider(Mutex<Option<ExecutionEvent>>);
struct PayloadBackend(Arc<AtomicUsize>);
struct PayloadEvents(Option<ExecutionEvent>, Arc<AtomicUsize>);
impl ExecutionEventStream for PayloadEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async {
            // This fixture produces invocation observations only after execute starts.
            if self.1.load(Ordering::SeqCst) == 0 {
                return std::future::pending().await;
            }
            Ok(self.0.take())
        })
    }
}
impl AgentProvider for PayloadProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("payload", "model", "").unwrap()
    }
    fn open(&self, _: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        Box::pin(async {
            let started = Arc::new(AtomicUsize::new(0));
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("provider").unwrap(),
                    Arc::new(PayloadBackend(started.clone())),
                    capabilities(),
                ),
                events: Box::new(PayloadEvents(self.0.lock().unwrap().take(), started)),
            })
        })
    }
}
impl ProviderSessionBackend for PayloadBackend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, _: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            let result: Result<ExecutionOutcome, AgentError> =
                { async { Ok(ExecutionOutcome::Completed) } }.await;
            ProviderExecutionReply::Finished(ExecutionReport::new(
                Some(result),
                None,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn answer_permission(
        &self,
        _: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
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
fn options(label: String) -> PermissionOptions {
    PermissionOptions::new(
        vec![PermissionOption::new(
            PermissionOptionId::new("allow").unwrap(),
            label,
            PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
        )
        .unwrap()],
        &PermissionOfferPolicy::once_only(),
    )
    .unwrap()
}
fn empty_tool() -> ToolCallUpdate {
    ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        None,
        None,
        None,
        None,
        None,
    )
}
fn review(
    input: ToolReviewInput,
    choices: PermissionOptions,
    observation: ToolCallUpdate,
    cancel: bool,
) -> ExecutionUpdate {
    let execution = ExecutionId::new("payload").unwrap();
    let id = PermissionId::new("review").unwrap();
    if cancel {
        let mut session = ExecutionSession::new(ExecutionSessionId::new("provider").unwrap());
        session.begin_execution(execution.clone()).unwrap();
        session.observe_tool(&execution, empty_tool()).unwrap();
        session
            .request_permission(PermissionRequest::new(
                id.clone(),
                execution.clone(),
                ToolCallId::new("tool").unwrap(),
                choices,
            ))
            .unwrap();
        let request = session
            .cancel_permission(
                &execution,
                &id,
                PermissionCancellationReason::provider_withdrawal(),
            )
            .unwrap()
            .unwrap();
        ExecutionUpdate::PermissionCancelled(
            PermissionCancellation::from_record(
                ExecutionSessionId::new("provider").unwrap(),
                request,
                input,
                CancellationOrigin::Provider,
            )
            .unwrap(),
        )
    } else {
        ExecutionUpdate::PermissionRequested {
            id,
            tool_id: ToolCallId::new("tool").unwrap(),
            observation: ToolObservation::default().with_update(observation),
            input,
            options: choices,
        }
    }
}
async fn admission(update: ExecutionUpdate, accepted: bool) {
    let storage = MemoryStorage::default();
    let event = ExecutionEvent::new(ExecutionId::new("payload").unwrap(), update);
    let agent = Agent::new(
        Arc::new(PayloadProvider(Mutex::new(Some(event)))),
        storage.manager().await,
    )
    .await
    .unwrap();
    let result = agent.invoke(request("payload"), actor()).await;
    let snapshot = storage.snapshot();
    if accepted {
        assert_eq!(result, Ok(ExecutionOutcome::Completed));
        assert_eq!(snapshot.invocations[0].events.len(), 1);
    } else {
        assert!(
            matches!(&result, Err(AgentError::ExecutionObservation { error, execution_result: Some(settled) }) if matches!(error.as_ref(), AgentError::Protocol(_)) && settled.as_ref() == &Ok(ExecutionOutcome::Completed)),
            "{result:?}"
        );
        assert!(snapshot.invocations[0].events.is_empty());
    }
    assert_eq!(snapshot.invocations[0].result, Some(result));
    agent.close(actor()).await.unwrap();
}
async fn rejected(update: ExecutionUpdate) {
    admission(update, false).await;
}

#[tokio::test]
async fn structured_boundaries_include_identity_and_option_charges() {
    for extra in [0, 1] {
        let tool = ToolCallUpdate::new(
            ToolCallId::new("tool").unwrap(),
            Some("x".repeat(BUDGET - "payload".len() - 2 * "tool".len() + extra)),
            None,
            None,
            None,
            None,
        );
        admission(ExecutionUpdate::Tool(tool), extra == 0).await;
        let choices = options("Allow".into());
        let overhead = "payload".len()
            + 3 * "review".len()
            + "tool".len()
            + "tool".len()
            + choices.payload_bytes();
        let input = ToolReviewInput {
            name: "tool".into(),
            arguments_json: "x".repeat(BUDGET - overhead + extra),
        };
        admission(review(input, choices, empty_tool(), false), extra == 0).await;
    }
}

#[tokio::test]
async fn structured_preflight_rejects_all_large_components_before_storage_clones() {
    for field in ["title", "locations", "content"] {
        let mut tool = empty_tool();
        match field {
            "title" => {
                tool = ToolCallUpdate::new(
                    tool.id().clone(),
                    Some(String::with_capacity(BUDGET + 1)),
                    None,
                    None,
                    None,
                    None,
                )
            }
            "locations" => {
                tool = ToolCallUpdate::new(
                    tool.id().clone(),
                    None,
                    None,
                    None,
                    Some(Vec::<FileLocation>::with_capacity(
                        BUDGET / std::mem::size_of::<FileLocation>() + 1,
                    )),
                    None,
                )
            }
            _ => {
                tool = ToolCallUpdate::new(
                    tool.id().clone(),
                    None,
                    None,
                    None,
                    None,
                    Some(Vec::<ToolContent>::with_capacity(
                        BUDGET / std::mem::size_of::<ToolContent>() + 1,
                    )),
                )
            }
        }
        rejected(ExecutionUpdate::Tool(tool)).await;
    }
    for cancel in [false, true] {
        for name in [false, true] {
            let mut input = ToolReviewInput {
                name: "tool".into(),
                arguments_json: "{}".into(),
            };
            if name {
                input.name = String::with_capacity(BUDGET + 1);
            } else {
                input.arguments_json = String::with_capacity(BUDGET + 1);
            }
            rejected(review(input, options("Allow".into()), empty_tool(), cancel)).await;
        }
    }
    rejected(review(
        ToolReviewInput {
            name: "tool".into(),
            arguments_json: "{}".into(),
        },
        options("x".repeat(BUDGET)),
        empty_tool(),
        false,
    ))
    .await;
    let observed = ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        None,
        None,
        None,
        Some(vec![FileLocation::new(
            FilePath::new("x".repeat(BUDGET)).unwrap(),
            None,
        )]),
        None,
    );
    rejected(review(
        ToolReviewInput {
            name: "tool".into(),
            arguments_json: "{}".into(),
        },
        options("Allow".into()),
        observed,
        false,
    ))
    .await;
}
