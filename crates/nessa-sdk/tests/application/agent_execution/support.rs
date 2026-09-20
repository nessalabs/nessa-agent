pub(super) use nessa_sdk::application::agent_execution::agents::*;
pub(super) use nessa_sdk::application::agent_execution::executions::*;
pub(super) use nessa_sdk::application::agent_execution::permissions::*;
pub(super) use nessa_sdk::application::agent_execution::providers::*;
pub(super) use nessa_sdk::application::agent_execution::sessions::*;
pub(super) use nessa_sdk::application::agent_execution::tools::*;
pub(super) use nessa_sdk::application::dto::{ModalitiesDto, ModelMetadataDto};
pub(super) use nessa_sdk::domain::agent_execution::executions::*;
pub(super) use nessa_sdk::domain::agent_execution::permissions::*;
pub(super) use nessa_sdk::domain::agent_execution::prompts::*;
pub(super) use nessa_sdk::domain::agent_execution::sessions::*;
pub(super) use nessa_sdk::domain::agent_execution::tools::*;
pub(super) use nessa_sdk::domain::common::value_objects::TokenLimits;
pub(super) use nessa_sdk::domain::effective_capabilities::value_objects::{
    BindingRestrictions, EffectiveCapabilities,
};
pub(super) use nessa_sdk::domain::model_metadata::entities::ModelMetadata;
pub(super) use nessa_sdk::domain::model_metadata::value_objects::{Modalities, ModelFeatures};
pub(super) use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

pub(super) struct AcceptingAudit;
impl ExecutionAudit for AcceptingAudit {
    fn record(&self, _record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

pub(super) struct RecordingSession {
    pub(super) prompts: AtomicUsize,
}
impl ProviderSessionBackend for RecordingSession {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, _input: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            let result: Result<ExecutionOutcome, AgentError> = {
                async move {
                    self.prompts.fetch_add(1, Ordering::SeqCst);
                    Ok(ExecutionOutcome::Completed)
                }
            }
            .await;
            ProviderExecutionReply::Finished(ExecutionReport::new(
                Some(result),
                None,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn answer_permission(
        &self,
        _answer: PermissionAnswer,
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
        _input: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn close(&self, _origin: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async { CleanupReport::confirmed(CloseOutcome { forced: false }) })
    }
}
pub(super) struct OfflineSession;
impl ProviderSessionBackend for OfflineSession {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, _input: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            let result: Result<ExecutionOutcome, AgentError> =
                { async { Err(AgentError::Closed) } }.await;
            ProviderExecutionReply::Finished(ExecutionReport::new(
                Some(result),
                None,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn answer_permission(
        &self,
        _answer: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async {
            Err(ProviderOperationFailure::permission_answer(
                AgentError::Closed,
                ProviderSessionState::Usable,
                PermissionSelectionState::Pending,
            ))
        })
    }
    fn cancel_permission(
        &self,
        _input: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn close(&self, _origin: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async move {
            let result: Result<CloseOutcome, AgentError> =
                { async { Err(AgentError::CleanupUncertain) } }.await;
            match result {
                Ok(outcome) => CleanupReport::confirmed(outcome),
                Err(error) => CleanupReport::unconfirmed(error),
            }
        })
    }
}
pub(super) fn capabilities() -> EffectiveCapabilities {
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let model = ModelMetadata::try_from(ModelMetadataDto {
        provider: "anthropic".into(),
        model_id: "fixture".into(),
        display_name: "Fixture".into(),
        input: text,
        image_input: None,
        output: text,
        tool_use: true,
        reasoning: false,
        max_context_window_tokens: 1000,
        max_output_tokens: 100,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
    })
    .unwrap();
    let text = Modalities::new(true, false, false).unwrap();
    EffectiveCapabilities::new(
        &model,
        BindingRestrictions::new(ModelFeatures::new(text, text, true, false), model.limits()),
        model.limits(),
    )
    .unwrap()
}
pub(super) fn attribution() -> ApprovalAttribution {
    ApprovalAttribution::new(
        ActionContext::new("nessa.binding-fixture", "nessa.cli", "fixture-answer").unwrap(),
        ApprovalBasis::Mode(ApprovalModeSnapshot::new("fixture-file-policy", "1").unwrap()),
    )
}

pub(super) fn pending_permission() -> PermissionRequest {
    PermissionRequest::new(
        PermissionId::new("permission").unwrap(),
        ExecutionId::new("execution").unwrap(),
        ToolCallId::new("tool").unwrap(),
        PermissionOptions::new(
            vec![PermissionOption::new(
                PermissionOptionId::new("allow").unwrap(),
                "Allow this write",
                PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
            )
            .unwrap()],
            &PermissionOfferPolicy::once_only(),
        )
        .unwrap(),
    )
}

pub(super) fn pending_controller() -> ExecutionController {
    let request = pending_permission();
    let mut controller = ExecutionController::new(ExecutionSessionId::new("fixture").unwrap());
    controller
        .begin_execution(request.execution_id().clone())
        .unwrap();
    let tool = ToolCallUpdate::new(request.tool_id().clone(), None, None, None, None, None);
    controller
        .tool_event(request.execution_id(), tool.clone())
        .unwrap();
    controller
        .request_permission(
            request.execution_id(),
            request.id().clone(),
            tool,
            review_input(),
            request.options().clone(),
        )
        .unwrap();
    controller
}

pub(super) struct InMemoryPermissionBackend {
    pub(super) execution: Mutex<ExecutionController>,
    pub(super) cancellations: Mutex<Vec<PermissionCancellation>>,
    pub(super) closures: Mutex<Vec<SessionClosureRecord>>,
}
impl ProviderSessionBackend for InMemoryPermissionBackend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, _: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            let result: Result<ExecutionOutcome, AgentError> =
                { async { Err(AgentError::Unsupported("permission-only fixture".into())) } }.await;
            ProviderExecutionReply::Rejected(result.expect_err("fixture rejects execution"))
        })
    }
    fn answer_permission(
        &self,
        answer: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async move {
            let result: Result<PermissionResolution, AgentError> =
                { async move { self.execution.lock().unwrap().answer_permission(answer) } }.await;
            result.map_err(|error| {
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
        Box::pin(async move {
            let result: Result<PermissionCancellation, AgentError> = {
                async move {
                    let record = self.execution.lock().unwrap().cancel_review(input)?;
                    self.cancellations.lock().unwrap().push(record.clone());
                    Ok(record)
                }
            }
            .await;
            result
                .map_err(|error| ProviderOperationFailure::new(error, ProviderSessionState::Usable))
        })
    }
    fn close(&self, origin: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async move {
            let result: Result<CloseOutcome, AgentError> = {
                async move {
                    let records = self
                        .execution
                        .lock()
                        .unwrap()
                        .close(origin.reason(), origin.origin())?;
                    for record in records {
                        match record {
                            ExecutionAuditRecord::Cancelled(record) => {
                                self.cancellations.lock().unwrap().push(record)
                            }
                            ExecutionAuditRecord::SessionClosed(record) => {
                                self.closures.lock().unwrap().push(record)
                            }
                            ExecutionAuditRecord::Finished(_) => {
                                panic!("close cannot finish an execution")
                            }
                            ExecutionAuditRecord::Answered(_) => {
                                panic!("close cannot answer permissions")
                            }
                            ExecutionAuditRecord::QueueReordered(_) => {
                                panic!("close cannot reorder pending work")
                            }
                        }
                    }
                    Ok(CloseOutcome { forced: false })
                }
            }
            .await;
            match result {
                Ok(outcome) => CleanupReport::confirmed(outcome),
                Err(error) => CleanupReport::unconfirmed(error),
            }
        })
    }
}

pub(super) fn review_input() -> ToolReviewInput {
    ToolReviewInput {
        name: "fixture-write".into(),
        arguments_json: r#"{"target":"file.txt","text":"exact content"}"#.into(),
    }
}

pub(super) fn close_action() -> ActionContext {
    ActionContext::new("test-user", "test", "close").unwrap()
}

/// A fixture that has physically cleaned up but failed its independent evidence effect.
pub(super) fn cleaned_with_error(error: AgentError) -> CleanupReport {
    CleanupReport::new(
        ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
        Err(error),
    )
}
