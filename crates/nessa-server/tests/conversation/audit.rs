use super::*;
use nessa_sdk::application::agent_execution::{
    executions::{ExecutionController, QueueOrderRecord, SessionClosureRecord},
    permissions::{
        ActionContext, ApprovalAttribution, ApprovalBasis, CancellationOrigin, PermissionAnswer,
        PermissionAnswerDelivery, PermissionAnswerRecord,
    },
    tools::ToolReviewInput,
};
use nessa_sdk::domain::agent_execution::{
    executions::{ExecutionId, ExecutionOutcome, InvocationKind, QueueOrderChange},
    sessions::{ExecutionSession, ExecutionSessionId, SessionId},
};
use nessa_sdk::domain::agent_execution::{
    permissions::{
        CustomPermissionCancellationReason, PermissionCancellationReason, PermissionDecision,
        PermissionEffect, PermissionId, PermissionOfferPolicy, PermissionOption,
        PermissionOptionId, PermissionOptions, PermissionScope,
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
