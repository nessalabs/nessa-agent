//! Every command field must agree with independently valid adapter evidence.
use super::*;

struct ReturnedEvidence {
    answer: Option<PermissionResolution>,
    cancellation: Option<PermissionCancellation>,
}
impl ProviderSessionBackend for ReturnedEvidence {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, _: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            let result: Result<ExecutionOutcome, AgentError> =
                { async { Err(AgentError::Unsupported("evidence fixture".into())) } }.await;
            ProviderExecutionReply::Rejected(result.expect_err("fixture rejects execution"))
        })
    }
    fn answer_permission(
        &self,
        _: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async move {
            let result: Result<PermissionResolution, AgentError> =
                { async { Ok(self.answer.clone().unwrap()) } }.await;
            result
                .map_err(|error| ProviderOperationFailure::new(error, ProviderSessionState::Usable))
        })
    }
    fn cancel_permission(
        &self,
        _: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        Box::pin(async move {
            let result: Result<PermissionCancellation, AgentError> =
                { async { Ok(self.cancellation.clone().unwrap()) } }.await;
            result
                .map_err(|error| ProviderOperationFailure::new(error, ProviderSessionState::Usable))
        })
    }
    fn close(&self, _: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async { CleanupReport::confirmed(CloseOutcome { forced: false }) })
    }
}
fn controller(
    session: &str,
    execution: &str,
    permission: &str,
    option: &str,
) -> ExecutionController {
    let mut controller = ExecutionController::new(ExecutionSessionId::new(session).unwrap());
    controller
        .begin_execution(ExecutionId::new(execution).unwrap())
        .unwrap();
    controller
        .request_permission(
            &ExecutionId::new(execution).unwrap(),
            PermissionId::new(permission).unwrap(),
            ToolCallUpdate::new(
                ToolCallId::new("tool").unwrap(),
                None,
                None,
                None,
                None,
                None,
            ),
            review_input(),
            PermissionOptions::new(
                vec![PermissionOption::new(
                    PermissionOptionId::new(option).unwrap(),
                    "Allow this write",
                    PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
                )
                .unwrap()],
                &PermissionOfferPolicy::once_only(),
            )
            .unwrap(),
        )
        .unwrap();
    controller
}
fn answer() -> PermissionAnswer {
    PermissionAnswer {
        execution_id: ExecutionId::new("execution").unwrap(),
        id: PermissionId::new("permission").unwrap(),
        option_id: PermissionOptionId::new("allow").unwrap(),
        attribution: attribution(),
    }
}
fn reason(code: &str, explanation: &str) -> PermissionCancellationReason {
    PermissionCancellationReason::custom(
        CustomPermissionCancellationReason::new(code, explanation).unwrap(),
    )
}

#[tokio::test]
async fn answer_response_checks_session_execution_review_option_and_entire_attribution() {
    for field in 0..11 {
        let command = answer();
        let mut actual = command.clone();
        let mut session = "fixture";
        match field {
            0 => {} // Fully matching evidence remains accepted.
            1 => session = "other-session",
            2 => actual.execution_id = ExecutionId::new("other-execution").unwrap(),
            3 => actual.id = PermissionId::new("other-permission").unwrap(),
            4 => actual.option_id = PermissionOptionId::new("other-option").unwrap(),
            5..=7 => {
                let original = command.attribution.actor();
                let mut actor = [
                    original.principal_id(),
                    original.surface_id(),
                    original.request_id(),
                ];
                actor[field - 5] = "other-actor-field";
                actual.attribution = ApprovalAttribution::new(
                    ActionContext::new(actor[0], actor[1], actor[2]).unwrap(),
                    command.attribution.basis().clone(),
                );
            }
            8 => {
                actual.attribution = ApprovalAttribution::new(
                    command.attribution.actor().clone(),
                    ApprovalBasis::Explicit,
                )
            }
            9 => {
                actual.attribution = ApprovalAttribution::new(
                    command.attribution.actor().clone(),
                    ApprovalBasis::Mode(ApprovalModeSnapshot::new("other-mode", "1").unwrap()),
                )
            }
            10 => {
                actual.attribution = ApprovalAttribution::new(
                    command.attribution.actor().clone(),
                    ApprovalBasis::Mode(
                        ApprovalModeSnapshot::new("fixture-file-policy", "other-revision").unwrap(),
                    ),
                )
            }
            _ => unreachable!(),
        }
        let resolution = controller(
            session,
            actual.execution_id.as_str(),
            actual.id.as_str(),
            actual.option_id.as_str(),
        )
        .answer_permission(actual)
        .unwrap();
        let agent = reviewed_agent(Arc::new(ReturnedEvidence {
            answer: Some(resolution.clone()),
            cancellation: None,
        }))
        .await;
        let result = agent.answer_permission(command).await;
        if field == 0 {
            assert_eq!(result, Ok(resolution));
        } else {
            assert!(
                matches!(result, Err(AgentError::Protocol(_))),
                "field {field}: {result:?}"
            );
        }
        agent.close(close_action()).await.unwrap();
    }
}

#[tokio::test]
async fn cancellation_response_checks_session_execution_review_cause_and_entire_actor() {
    for field in 0..9 {
        let command = PermissionCancellationRequest {
            execution_id: ExecutionId::new("execution").unwrap(),
            id: PermissionId::new("permission").unwrap(),
            reason: reason("withdraw", "review no longer needed"),
            actor: close_action(),
        };
        let mut actual = command.clone();
        let mut session = "fixture";
        match field {
            0 => {}
            1 => session = "other-session",
            2 => actual.execution_id = ExecutionId::new("other-execution").unwrap(),
            3 => actual.id = PermissionId::new("other-permission").unwrap(),
            4 => actual.reason = reason("other-code", "review no longer needed"),
            5 => actual.reason = reason("withdraw", "different explanation"),
            6..=8 => {
                let original = &command.actor;
                let mut actor = [
                    original.principal_id(),
                    original.surface_id(),
                    original.request_id(),
                ];
                actor[field - 6] = "other-actor-field";
                actual.actor = ActionContext::new(actor[0], actor[1], actor[2]).unwrap();
            }
            _ => unreachable!(),
        }
        let record = controller(
            session,
            actual.execution_id.as_str(),
            actual.id.as_str(),
            "allow",
        )
        .cancel_review(actual)
        .unwrap();
        let agent = reviewed_agent(Arc::new(ReturnedEvidence {
            answer: None,
            cancellation: Some(record.clone()),
        }))
        .await;
        let result = agent.cancel_permission(command).await;
        if field == 0 {
            assert_eq!(result, Ok(record));
        } else {
            assert!(
                matches!(result, Err(AgentError::Protocol(_))),
                "field {field}: {result:?}"
            );
        }
        agent.close(close_action()).await.unwrap();
    }
}
