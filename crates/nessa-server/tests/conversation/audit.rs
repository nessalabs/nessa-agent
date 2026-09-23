use super::*;
use nessa_sdk::application::agent_execution::{
    executions::{
        AttachmentAuditCause, AttachmentAuditRecord, AttachmentAuditStage, ExecutionController,
        QueueAdmissionRecord, QueueOrderRecord, QueueSettlementRecord, SessionClosureRecord,
        SteeringAcknowledgementRecord,
    },
    permissions::{
        ActionContext, ApprovalAttribution, ApprovalBasis, CancellationOrigin, PermissionAnswer,
        PermissionAnswerDelivery, PermissionAnswerRecord, ReviewDeclineRecord,
    },
    tools::ToolReviewInput,
};
use nessa_sdk::domain::agent_execution::{
    executions::{
        ExecutionId, ExecutionOutcome, InvocationKind, QueueOrderChange, SchedulingCause,
        SubmissionMode,
    },
    sessions::{AttachmentCause, ExecutionSession, ExecutionSessionId, SessionId},
};
use nessa_sdk::domain::agent_execution::{
    permissions::{
        CustomPermissionCancellationReason, PermissionCancellationReason, PermissionDecision,
        PermissionEffect, PermissionId, PermissionOfferPolicy, PermissionOption,
        PermissionOptionId, PermissionOptions, PermissionScope, ReviewDecline, ReviewDeclineId,
        ReviewDeclineReason,
    },
    tools::{ToolCallId, ToolCallUpdate},
};
use std::fs;
struct TestClock;
impl Clock for TestClock {
    fn unix_milliseconds(&self) -> u64 {
        1234
    }
}
fn finished(outcome: ExecutionOutcome) -> ExecutionAuditRecord {
    let mut session = ExecutionSession::new(ExecutionSessionId::new("provider-session").unwrap());
    let id = ExecutionId::new("execution").unwrap();
    session.begin_execution(id.clone()).unwrap();
    ExecutionAuditRecord::Finished(session.finish_execution(&id, Ok(outcome)).unwrap().0)
}
#[test]
fn audit_maps_attachment_and_admission_evidence_without_losing_correlation() {
    let session = SessionId::new("session").unwrap();
    let execution = ExecutionId::new("steering").unwrap();
    let target = ExecutionId::new("active").unwrap();
    let attachment = record_value(&ExecutionAuditRecord::Attachment(
        AttachmentAuditRecord::new(
            session.clone(),
            AttachmentAuditStage::Waiting,
            AttachmentAuditStage::Starting,
            AttachmentAuditCause::Started(AttachmentCause::Reopen),
            Some(actor()),
        ),
    ));
    assert_eq!(attachment["kind"], "attachment_transition");
    assert_eq!(attachment["before"], "waiting");
    assert_eq!(attachment["after"], "starting");
    assert_eq!(attachment["cause"]["kind"], "started");
    assert_eq!(attachment["cause"]["attachmentCause"], "reopen");
    assert_eq!(attachment["actor"]["requestId"], "action");

    let admission = record_value(&ExecutionAuditRecord::QueueAdmitted(
        QueueAdmissionRecord::submitted(
            session.clone(),
            execution.clone(),
            SubmissionMode::Steering,
            actor(),
        ),
    ));
    assert_eq!(admission["executionId"], "steering");
    assert_eq!(admission["mode"], "steering");
    assert_eq!(admission["before"], "unowned");
    assert_eq!(admission["after"], "owned");
    assert_eq!(admission["cause"], "submitted");

    let steering = record_value(&ExecutionAuditRecord::SteeringAcknowledged(
        SteeringAcknowledgementRecord::provider_acknowledged(session, execution, target, actor()),
    ));
    assert_eq!(steering["executionId"], "steering");
    assert_eq!(steering["target"], "active");
    assert_eq!(steering["before"], "pending");
    assert_eq!(steering["after"], "injected");
    assert_eq!(steering["cause"], "provider_acknowledged");
    assert_eq!(steering["actor"]["requestId"], "action");
}
#[test]
fn audit_maps_automatic_queue_settlement_separately_from_the_input_caller() {
    for (kind, target) in [
        (InvocationKind::Queued, None),
        (
            InvocationKind::Steering,
            Some(ExecutionId::new("active").unwrap()),
        ),
    ] {
        let value = record_value(&ExecutionAuditRecord::QueueSettled(
            QueueSettlementRecord::automatic_attachment_failed(
                SessionId::new("session").unwrap(),
                ExecutionId::new("waiting").unwrap(),
                kind,
                target.clone(),
                actor(),
            )
            .unwrap(),
        ));
        assert_eq!(value["kind"], "queue_settled");
        assert_eq!(value["sessionId"], "session");
        assert_eq!(value["executionId"], "waiting");
        assert_eq!(
            value["target"],
            json!(target.as_ref().map(|id| id.as_str()))
        );
        assert_eq!(
            value["mode"],
            if target.is_some() {
                "boundary_steering"
            } else {
                "queued"
            }
        );
        assert_eq!(value["before"], "queued");
        assert_eq!(value["after"], "settled");
        assert_eq!(value["cause"], "dispatch_failed");
        assert_eq!(value["origin"], json!({"kind":"runtime"}));
        assert_eq!(value["submittedBy"]["principalId"], actor().principal_id());
        assert_eq!(value["submittedBy"]["surfaceId"], actor().surface_id());
        assert_eq!(value["submittedBy"]["requestId"], "action");
        assert!(value.get("actor").is_none());
    }
}
#[test]
fn audit_maps_queue_cancellation_with_its_own_initiator() {
    let closer = ActionContext::new("closer", "closing-surface", "close-request").unwrap();
    for (cause, initiator) in [
        (SchedulingCause::SessionClosed, Some(closer.clone())),
        (SchedulingCause::RunnerStopped, None),
    ] {
        let value = record_value(&ExecutionAuditRecord::QueueSettled(
            QueueSettlementRecord::cancelled(
                SessionId::new("session").unwrap(),
                ExecutionId::new("waiting").unwrap(),
                InvocationKind::Queued,
                None,
                cause,
                actor(),
                initiator.clone(),
            )
            .unwrap(),
        ));
        assert_eq!(value["before"], "queued");
        assert_eq!(value["after"], "cancelled");
        assert_eq!(value["submittedBy"]["requestId"], "action");
        if initiator.is_some() {
            assert_eq!(value["cause"], "session_closed");
            assert_eq!(
                value["origin"],
                json!({
                    "kind":"client",
                    "actor":{
                        "principalId":"closer",
                        "surfaceId":"closing-surface",
                        "requestId":"close-request",
                    },
                })
            );
        } else {
            assert_eq!(value["cause"], "runner_stopped");
            assert_eq!(value["origin"], json!({"kind":"runtime"}));
        }
    }
}
#[test]
fn audit_maps_complete_queue_transition_with_priority_and_actor() {
    let first = ExecutionId::new("first").unwrap();
    let second = ExecutionId::new("second").unwrap();
    let change = QueueOrderChange::new(
        vec![
            (first.clone(), InvocationKind::Steering),
            (second.clone(), InvocationKind::Queued),
        ],
        vec![first, second],
    )
    .unwrap();
    let value = record_value(&ExecutionAuditRecord::QueueReordered(
        QueueOrderRecord::caller_requested(SessionId::new("session").unwrap(), change, actor()),
    ));
    assert_eq!(value["kind"], "queue_reorder_selected");
    assert_eq!(value["before"][0]["kind"], "steering");
    assert_eq!(value["before"][1]["executionId"], "second");
    assert_eq!(value["after"][0], "first");
    assert_eq!(value["cause"], "caller_requested");
    assert_eq!(value["actor"]["requestId"], "action");
}
#[tokio::test]
async fn audit_commits_distinct_terminal_results_with_correlation_and_time() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("audit");
    let sink = DurableExecutionAudit::new(path.clone(), Arc::new(TestClock)).unwrap();
    for result in [
        ExecutionOutcome::Completed,
        ExecutionOutcome::OutputLimit,
        ExecutionOutcome::RequestLimit,
        ExecutionOutcome::Refused,
        ExecutionOutcome::Cancelled,
    ] {
        sink.record(finished(result)).await.unwrap();
    }
    let mut outcomes = Vec::new();
    for file in fs::read_dir(path).unwrap() {
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(file.unwrap().path()).unwrap()).unwrap();
        assert_eq!(value["observedAtMs"], 1234);
        assert_eq!(value["record"]["executionId"], "execution");
        assert_eq!(value["record"]["sessionId"], "provider-session");
        assert_eq!(value["record"]["before"], "active");
        assert_eq!(value["record"]["after"], "released");
        outcomes.push(
            value["record"]["result"]["outcome"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    outcomes.sort();
    assert_eq!(
        outcomes,
        vec![
            "cancelled",
            "completed",
            "output_limit",
            "refused",
            "request_limit"
        ]
    );
}
#[tokio::test]
async fn missing_audit_directory_fails_acknowledgement() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("audit");
    let sink = DurableExecutionAudit::new(path.clone(), Arc::new(TestClock)).unwrap();
    fs::remove_dir(path).unwrap();
    assert_eq!(
        sink.record(finished(ExecutionOutcome::Completed)).await,
        Err(AgentError::AuditFailure)
    );
}

fn actor() -> ActionContext {
    ActionContext::new("verified-principal", "verified-credential", "action").unwrap()
}
fn options() -> PermissionOptions {
    PermissionOptions::new(
        vec![PermissionOption::new(
            PermissionOptionId::new("allow-once").unwrap(),
            "Allow once",
            PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
        )
        .unwrap()],
        &PermissionOfferPolicy::once_only(),
    )
    .unwrap()
}
#[test]
fn audit_maps_permission_selection_delivery_and_custom_cancellation_exactly() {
    let id = ExecutionId::new("execution").unwrap();
    let permission = PermissionId::new("permission").unwrap();
    let tool = ToolCallId::new("tool").unwrap();
    let session = ExecutionSessionId::new("session").unwrap();
    let input = ToolReviewInput {
        name: "Write".into(),
        arguments_json: r#"{"path":"reviewed-file"}"#.into(),
    };
    let mut controller = ExecutionController::new(session.clone());
    controller.begin_execution(id.clone()).unwrap();
    controller
        .request_permission(
            &id,
            permission.clone(),
            ToolCallUpdate::new(
                tool.clone(),
                Some("Write file".into()),
                None,
                None,
                None,
                None,
            ),
            input.clone(),
            options(),
        )
        .unwrap();
    let answer = controller
        .answer_permission(PermissionAnswer {
            execution_id: id.clone(),
            id: permission.clone(),
            option_id: PermissionOptionId::new("allow-once").unwrap(),
            attribution: ApprovalAttribution::new(actor(), ApprovalBasis::Explicit),
        })
        .unwrap();
    for (delivery, stage) in [
        (PermissionAnswerDelivery::Selected, "selected"),
        (PermissionAnswerDelivery::Written, "written"),
        (
            PermissionAnswerDelivery::Failed(AgentError::CleanupUncertain),
            "failed",
        ),
    ] {
        let value = record_value(&ExecutionAuditRecord::Answered(
            PermissionAnswerRecord::new(answer.clone(), delivery),
        ));
        assert_eq!(value["delivery"]["stage"], stage);
        assert_eq!(value["actor"]["surfaceId"], "verified-credential");
        assert_eq!(value["input"]["argumentsJson"], input.arguments_json);
        assert_eq!(value["request"]["transition"]["optionId"], "allow-once");
        assert_eq!(
            value["request"]["transition"]["decision"]["effect"],
            "allow"
        );
    }
    let mut controller = ExecutionController::new(session);
    controller.begin_execution(id.clone()).unwrap();
    controller
        .request_permission(
            &id,
            permission.clone(),
            ToolCallUpdate::new(tool, Some("Write file".into()), None, None, None, None),
            input,
            options(),
        )
        .unwrap();
    let cancellation = controller
        .cancel_permission(
            &id,
            &permission,
            PermissionCancellationReason::custom(
                CustomPermissionCancellationReason::new("guard.stop", "Explicit guard explanation")
                    .unwrap(),
            ),
            CancellationOrigin::Client(actor()),
        )
        .unwrap()
        .unwrap();
    let value = record_value(&ExecutionAuditRecord::Cancelled(cancellation));
    assert_eq!(value["origin"]["actor"]["requestId"], "action");
    assert_eq!(
        value["request"]["transition"]["reason"]["code"],
        "guard.stop"
    );
    assert_eq!(
        value["request"]["transition"]["reason"]["explanation"],
        "Explicit guard explanation"
    );
}
#[test]
fn audit_maps_explicit_close_without_claiming_physical_cleanup() {
    let mut session = ExecutionSession::new(ExecutionSessionId::new("session").unwrap());
    let result = session
        .close(PermissionCancellationReason::session_closed())
        .unwrap()
        .into_parts()
        .0
        .unwrap();
    let value = record_value(&ExecutionAuditRecord::SessionClosed(
        SessionClosureRecord::new(result, CancellationOrigin::Client(actor())).unwrap(),
    ));
    assert_eq!(value["before"], "open");
    assert_eq!(value["after"], "closed");
    assert_eq!(value["reason"]["code"], "session_closed");
    assert_eq!(
        value["origin"]["actor"]["principalId"],
        "verified-principal"
    );
    assert!(value.get("resourcesConfirmed").is_none());
}

#[test]
fn audit_maps_a_declined_review_as_a_claim_about_a_tool_nobody_was_offered() {
    let declined = |tool, reason, delivery| {
        record_value(&ExecutionAuditRecord::ReviewDeclined(
            ReviewDeclineRecord::new(
                ExecutionSessionId::new("session").unwrap(),
                ExecutionId::new("run").unwrap(),
                ReviewDeclineId::new("1").unwrap(),
                ReviewDecline::new(tool, reason),
                delivery,
            ),
        ))
    };
    let value = declined(
        Some("Monitor"),
        ReviewDeclineReason::ToolNotReviewable,
        PermissionAnswerDelivery::Selected,
    );
    assert_eq!(value["kind"], "review_declined");
    assert_eq!(value["sessionId"], "session");
    assert_eq!(value["executionId"], "run");
    assert_eq!(value["declineId"], "1");
    assert_eq!(value["reason"], "tool_not_reviewable");
    assert_eq!(value["delivery"]["stage"], "selected");
    assert_eq!(value["origin"]["kind"], "runtime");
    // The provider's claim about its own frame, named as one. A reader acting
    // on this should know nobody checked it.
    assert_eq!(value["declaredTool"], "Monitor");
    assert!(value.get("tool").is_none());
    // No request and no actor: a decline happens before either exists.
    assert!(value.get("request").is_none());
    assert!(value.get("actor").is_none());

    // Each reason is its own fact, and an unnamed tool says so rather than
    // being filled in.
    for (reason, code) in [
        (ReviewDeclineReason::UnreadableRequest, "unreadable_request"),
        (ReviewDeclineReason::UnusableOptions, "unusable_options"),
    ] {
        let value = declined(None, reason, PermissionAnswerDelivery::Written);
        assert_eq!(value["reason"], code);
        assert!(value["declaredTool"].is_null());
        assert_eq!(value["delivery"]["stage"], "written");
    }
    let failed = declined(
        Some("Monitor"),
        ReviewDeclineReason::ToolNotReviewable,
        PermissionAnswerDelivery::Failed(AgentError::Transport("stdin write failed".into())),
    );
    assert_eq!(failed["delivery"]["stage"], "failed");
    assert!(failed["delivery"]["diagnostic"]
        .as_str()
        .unwrap()
        .contains("stdin write failed"));
}
