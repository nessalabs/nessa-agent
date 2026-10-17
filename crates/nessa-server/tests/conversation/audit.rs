use super::*;
use crate::conversation_test_support::{Provider, ProviderFactory, RecordingModeExecutionAudit};
use nessa_sdk::application::agent_execution::{
    agents::{Agent, AttachmentRequest},
    executions::{
        AdmittedQuestion, AttachmentAuditCause, AttachmentAuditStage, ExecutionController,
        ExecutionUpdate, LateProviderResultRecord, QueueAdmissionRecord, QueueOrderRecord,
        QueueSettlementRecord, SessionClosureRecord, SteeringAcknowledgementRecord,
    },
    permissions::{
        ActionContext, ApprovalAttribution, ApprovalBasis, ApprovalRuleReference,
        CancellationOrigin, HookRefusal, PermissionAnswer, PermissionAnswerDelivery,
        PermissionAnswerRecord, QuestionAnswerRecord, QuestionRefusalRecord, RefusedAsk,
        ReviewDeclineRecord,
    },
    sessions::SessionManager,
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
        PermissionOptionId, PermissionOptions, PermissionScope, RefusalOutcome, ReviewDecline,
        ReviewDeclineId, ReviewDeclineReason,
    },
    questions::{
        AgentQuestion, AnswerOption, AnswerShape, Question, QuestionCancellation, QuestionChoice,
        QuestionId, QuestionRefusalReason, MAX_OPEN_ASK_COST, MAX_OPEN_QUESTIONS,
    },
    tools::{ToolCallId, ToolCallUpdate},
    ExecutionError,
};
use nessa_sdk::infrastructure::session_storage::{InMemoryStorage, RuntimeMessageCommitClock};
use serde_json::Value;
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
#[tokio::test]
async fn audit_maps_attachment_and_admission_evidence_without_losing_correlation() {
    let session = SessionId::new("session").unwrap();
    let execution = ExecutionId::new("steering").unwrap();
    let target = ExecutionId::new("active").unwrap();
    let audit = Arc::new(RecordingModeExecutionAudit::default());
    let manager = SessionManager::open(
        Some(session.clone()),
        Arc::new(InMemoryStorage::new()),
        Arc::new(RuntimeMessageCommitClock::new()),
    )
    .await
    .unwrap();
    let agent = Agent::prepare(
        Arc::new(Provider::new(Arc::new(ProviderFactory::default()))),
        manager,
        audit.clone(),
    )
    .await
    .unwrap();
    for _ in 0..2 {
        let authorization = agent
            .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
            .unwrap();
        agent
            .start_attachment(authorization)
            .unwrap()
            .wait()
            .await
            .unwrap();
        agent.close(actor()).await.unwrap();
    }
    let attachment = {
        let records = audit.records.lock().unwrap();
        let reopened = records
            .iter()
            .find(|record| {
                matches!(record,
                    ExecutionAuditRecord::Attachment(record)
                    if record.cause() == AttachmentAuditCause::Started(AttachmentCause::Reopen)
                        && record.after() == AttachmentAuditStage::Starting
                )
            })
            .unwrap();
        record_value(reopened)
    };
    assert_eq!(attachment["kind"], "attachment_transition");
    assert_eq!(attachment["sessionId"], "session");
    assert_eq!(attachment["generation"], 1);
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
    assert!(admission["approvalMode"].is_null());
    assert!(admission["effortLevel"].is_null());
    assert!(admission["admissionGeneration"].is_null());
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
#[test]
fn effort_level_change_keeps_target_levels_stage_and_actor() {
    use nessa_sdk::application::agent_execution::executions::{
        EffortChangeOutcome, EffortLevelChange,
    };
    use nessa_sdk::domain::model_metadata::value_objects::EffortLevel;
    let change = EffortLevelChange::new(
        SessionId::new("session").unwrap(),
        "instance:2".into(),
        actor(),
        None,
        EffortLevel::new("xhigh".into()).unwrap(),
    );
    for (record, stage) in [
        (change.requested(), "requested"),
        (change.settled(EffortChangeOutcome::Applied), "applied"),
        (change.settled(EffortChangeOutcome::Refused), "refused"),
        (change.settled(EffortChangeOutcome::Failed), "failed"),
    ] {
        let value = record_value(&ExecutionAuditRecord::EffortLevelChanged(record));
        assert_eq!(value["kind"], "effort_level_change");
        assert_eq!(value["sessionId"], "session");
        assert_eq!(value["attachmentGeneration"], "instance:2");
        assert_eq!(value["before"], serde_json::Value::Null);
        assert_eq!(value["after"], "xhigh");
        assert_eq!(value["stage"], stage);
        assert_eq!(value["cause"], "caller_requested");
        assert_eq!(value["actor"]["requestId"], "action");
    }
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
fn deny_options() -> PermissionOptions {
    PermissionOptions::new(
        vec![PermissionOption::new(
            PermissionOptionId::new("deny-once").unwrap(),
            "Deny",
            PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request()),
        )
        .unwrap()],
        &PermissionOfferPolicy::once_only(),
    )
    .unwrap()
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
            None,
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
            None,
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

fn deadline_answer_value(
    deadline: nessa_sdk::domain::agent_execution::permissions::PermissionReviewDeadline,
) -> serde_json::Value {
    let id = ExecutionId::new("execution").unwrap();
    let permission = PermissionId::new("permission").unwrap();
    let tool = ToolCallId::new("tool").unwrap();
    let session = ExecutionSessionId::new("session").unwrap();
    let input = ToolReviewInput {
        name: "Write".into(),
        arguments_json: r#"{"path":"reviewed-file"}"#.into(),
    };
    let choices = PermissionOptions::new(
        vec![
            PermissionOption::new(
                PermissionOptionId::new("deny-once").unwrap(),
                "Deny once",
                PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request()),
            )
            .unwrap(),
            PermissionOption::new(
                PermissionOptionId::new("allow-once").unwrap(),
                "Allow once",
                PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
            )
            .unwrap(),
        ],
        &PermissionOfferPolicy::once_only(),
    )
    .unwrap();
    let mut controller = ExecutionController::new(session);
    controller.begin_execution(id.clone()).unwrap();
    controller
        .request_permission(
            &id,
            permission.clone(),
            ToolCallUpdate::new(tool, Some("Write file".into()), None, None, None, None),
            input.clone(),
            choices,
            Some(deadline.clone().promise(1_700_000_030_000).unwrap()),
        )
        .unwrap();
    let answer = controller
        .settle_deadline_for_test(
            &id,
            &permission,
            &PermissionOptionId::new("deny-once").unwrap(),
            deadline,
        )
        .unwrap();
    record_value(&ExecutionAuditRecord::Answered(
        PermissionAnswerRecord::new(answer, PermissionAnswerDelivery::Selected),
    ))
}

#[test]
fn audit_maps_a_rule_answer_exactly() {
    use serde_json::json;

    let id = ExecutionId::new("execution").unwrap();
    let permission = PermissionId::new("permission").unwrap();
    let tool = ToolCallId::new("tool").unwrap();
    let session = ExecutionSessionId::new("session").unwrap();
    let input = ToolReviewInput {
        name: "Write".into(),
        arguments_json: r#"{"path":"reviewed-file"}"#.into(),
    };
    let choices = PermissionOptions::new(
        vec![PermissionOption::new(
            PermissionOptionId::new("allow-once").unwrap(),
            "Allow once",
            PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
        )
        .unwrap()],
        &PermissionOfferPolicy::once_only(),
    )
    .unwrap();
    let mut controller = ExecutionController::new(session);
    controller.begin_execution(id.clone()).unwrap();
    controller
        .request_permission(
            &id,
            permission.clone(),
            ToolCallUpdate::new(tool, Some("Write file".into()), None, None, None, None),
            input,
            choices,
            None,
        )
        .unwrap();
    let grantor = ActionContext::new("owner", "desktop", "grant-request").unwrap();
    let rule = ApprovalRuleReference::new("pr_rule", "2", grantor.clone()).unwrap();
    let answer = controller
        .answer_permission(PermissionAnswer {
            attribution: ApprovalAttribution::new(grantor, ApprovalBasis::Rule(rule)),
            execution_id: id,
            id: permission,
            option_id: PermissionOptionId::new("allow-once").unwrap(),
        })
        .unwrap();
    let value = record_value(&ExecutionAuditRecord::Answered(
        PermissionAnswerRecord::new(answer, PermissionAnswerDelivery::Selected),
    ));
    assert_eq!(
        value["basis"],
        json!({
            "kind": "rule",
            "ruleId": "pr_rule",
            "revision": "2",
            "grantedBy": {
                "principalId": "owner",
                "surfaceId": "desktop",
                "requestId": "grant-request",
            },
        })
    );
    assert_eq!(value["request"]["transition"]["optionId"], "allow-once");
    assert_eq!(value["actor"]["principalId"], "owner");
}

#[test]
fn audit_maps_a_deadline_answer_exactly() {
    use nessa_sdk::domain::agent_execution::permissions::{
        PermissionReviewDeadline, PermissionToolClass, STANDARD_DENY_REASON,
    };
    use serde_json::json;
    use std::time::Duration;

    let value = deadline_answer_value(
        PermissionReviewDeadline::new(
            PermissionToolClass::Other,
            Duration::from_secs(30),
            PermissionEffect::Deny,
            STANDARD_DENY_REASON,
        )
        .unwrap(),
    );
    assert_eq!(
        value,
        json!({
            "kind": "permission_answered",
            "sessionId": "session",
            "request": {
                "permissionId": "permission",
                "executionId": "execution",
                "toolId": "tool",
                "options": [
                    {
                        "id": "deny-once",
                        "label": "Deny once",
                        "decision": {"effect": "deny", "scope": {"kind": "request"}},
                    },
                    {
                        "id": "allow-once",
                        "label": "Allow once",
                        "decision": {"effect": "allow", "scope": {"kind": "request"}},
                    },
                ],
                "transition": {
                    "before": "pending",
                    "after": "answered",
                    "optionId": "deny-once",
                    "decision": {"effect": "deny", "scope": {"kind": "request"}},
                },
            },
            "input": {"name": "Write", "argumentsJson": r#"{"path":"reviewed-file"}"#},
            "actor": {
                "principalId": "permission-deadline",
                "surfaceId": "other",
                "requestId": "permission",
            },
            "basis": {
                "kind": "deadline",
                "class": "other",
                "waitMs": 30000,
                "effect": "deny",
                "reason": STANDARD_DENY_REASON,
                "stopsTurn": false,
            },
            "delivery": {"stage": "selected"},
        })
    );
}

#[test]
fn audit_records_that_a_deadline_refusal_stopped_the_turn() {
    use nessa_sdk::domain::agent_execution::permissions::{
        PermissionReviewDeadline, PermissionToolClass, STANDARD_DENY_REASON, STANDARD_STOP_REASON,
    };
    use serde_json::json;
    use std::time::Duration;

    let value = deadline_answer_value(
        PermissionReviewDeadline::new(
            PermissionToolClass::Other,
            Duration::from_secs(30),
            PermissionEffect::Deny,
            STANDARD_DENY_REASON,
        )
        .unwrap()
        .stopping(),
    );
    assert_eq!(
        value["basis"],
        json!({
            "kind": "deadline",
            "class": "other",
            "waitMs": 30000,
            "effect": "deny",
            "reason": STANDARD_STOP_REASON,
            "stopsTurn": true,
        })
    );
}
#[test]
fn audit_maps_a_hook_refusal_as_the_hook_and_not_the_person() {
    let id = ExecutionId::new("execution").unwrap();
    let permission = PermissionId::new("permission").unwrap();
    let session = ExecutionSessionId::new("session").unwrap();
    let input = ToolReviewInput {
        name: "Bash".into(),
        arguments_json: "{}".into(),
    };
    let mut controller = ExecutionController::new(session);
    controller.begin_execution(id.clone()).unwrap();
    controller
        .request_permission(
            &id,
            permission.clone(),
            ToolCallUpdate::new(
                ToolCallId::new("tool").unwrap(),
                None,
                None,
                None,
                None,
                None,
            ),
            input,
            deny_options(),
            None,
        )
        .unwrap();
    let attribution =
        ApprovalAttribution::hook(&permission, HookRefusal::new(Some(2), "UNIQUE_HOOK_REASON"))
            .unwrap();
    let answer = controller
        .answer_permission(PermissionAnswer {
            execution_id: id,
            id: permission,
            option_id: PermissionOptionId::new("deny-once").unwrap(),
            attribution,
        })
        .unwrap();
    let value = record_value(&ExecutionAuditRecord::Answered(
        PermissionAnswerRecord::new(answer, PermissionAnswerDelivery::Selected),
    ));
    assert_eq!(value["basis"]["kind"], "hook");
    assert_eq!(value["basis"]["index"], 2);
    assert_eq!(value["basis"]["reason"], "UNIQUE_HOOK_REASON");
    assert_eq!(value["basis"]["outcome"], "continue");
    assert_eq!(value["actor"]["principalId"], "permission-review-hook");
    assert_eq!(value["actor"]["surfaceId"], "2");
    assert_ne!(value["actor"]["principalId"], actor().principal_id());
}

#[test]
fn audit_maps_a_runtime_hook_refusal_with_no_hook_index() {
    let id = ExecutionId::new("execution").unwrap();
    let permission = PermissionId::new("permission").unwrap();
    let session = ExecutionSessionId::new("session").unwrap();
    let input = ToolReviewInput {
        name: "Bash".into(),
        arguments_json: "{}".into(),
    };
    let mut controller = ExecutionController::new(session);
    controller.begin_execution(id.clone()).unwrap();
    controller
        .request_permission(
            &id,
            permission.clone(),
            ToolCallUpdate::new(
                ToolCallId::new("tool").unwrap(),
                None,
                None,
                None,
                None,
                None,
            ),
            input,
            deny_options(),
            None,
        )
        .unwrap();
    let attribution =
        ApprovalAttribution::hook(&permission, HookRefusal::new(None, "hook failed: timeout"))
            .unwrap();
    let answer = controller
        .answer_permission(PermissionAnswer {
            execution_id: id,
            id: permission,
            option_id: PermissionOptionId::new("deny-once").unwrap(),
            attribution,
        })
        .unwrap();
    let value = record_value(&ExecutionAuditRecord::Answered(
        PermissionAnswerRecord::new(answer, PermissionAnswerDelivery::Selected),
    ));
    assert_eq!(value["basis"]["kind"], "hook");
    assert!(value["basis"]["index"].is_null());
    assert_eq!(value["basis"]["reason"], "hook failed: timeout");
    assert_eq!(value["basis"]["outcome"], "continue");
    assert_eq!(value["actor"]["principalId"], "permission-review-hook");
    assert_eq!(value["actor"]["surfaceId"], "runtime");
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

fn decline_record(
    id: ReviewDeclineId,
    decline: ReviewDecline,
    delivery: PermissionAnswerDelivery,
    rule: Option<ApprovalRuleReference>,
) -> Result<ReviewDeclineRecord, AgentError> {
    let mut controller = ExecutionController::new(ExecutionSessionId::new("session").unwrap());
    let execution = ExecutionId::new("run").unwrap();
    controller.begin_execution(execution.clone())?;
    controller
        .decline_review(&execution, id, decline, rule)
        .map(|selected| selected.with_delivery(delivery))
}

#[test]
fn audit_maps_a_declined_review_as_a_claim_about_a_tool_nobody_was_offered() {
    let declined = |tool, reason, delivery| {
        record_value(&ExecutionAuditRecord::ReviewDeclined(
            decline_record(
                ReviewDeclineId::new("1").unwrap(),
                ReviewDecline::new(
                    tool,
                    reason,
                    nessa_sdk::domain::agent_execution::permissions::RefusalOutcome::Continue,
                ),
                delivery,
                None,
            )
            .unwrap(),
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
    assert_eq!(value["outcome"], "continue");
    assert!(value.get("stop").is_none());
}

#[test]
fn audit_maps_a_stopping_refusal_as_the_cause_of_the_stop() {
    let rule = ApprovalRuleReference::new("refuse-shell", "3", actor()).unwrap();
    let value = record_value(&ExecutionAuditRecord::ReviewDeclined(
        decline_record(
            ReviewDeclineId::new("9").unwrap(),
            ReviewDecline::new(
                Some("Bash"),
                ReviewDeclineReason::ToolNotReviewable,
                RefusalOutcome::EndTurn,
            ),
            PermissionAnswerDelivery::Selected,
            Some(rule),
        )
        .unwrap(),
    ));
    assert_eq!(value["outcome"], "end_turn");
    assert_eq!(value["origin"]["kind"], "rule");
    assert_eq!(value["origin"]["ruleId"], "refuse-shell");
    assert_eq!(value["origin"]["revision"], "3");
    assert_eq!(value["reason"], "tool_not_reviewable");
    assert_eq!(value["declineId"], "9");
    assert!(value.get("stop").is_none());
    assert!(value.get("actor").is_none());

    let closed = record_value(&ExecutionAuditRecord::ReviewDeclined(
        decline_record(
            ReviewDeclineId::new("9").unwrap(),
            ReviewDecline::new(
                None,
                ReviewDeclineReason::UnusableOptions,
                RefusalOutcome::CloseSession,
            ),
            PermissionAnswerDelivery::Written,
            Some(ApprovalRuleReference::new("refuse-shell", "3", actor()).unwrap()),
        )
        .unwrap(),
    ));
    assert_eq!(closed["outcome"], "close_session");
    assert!(closed.get("stop").is_none());

    let late = record_value(&ExecutionAuditRecord::LateProviderResult(
        LateProviderResultRecord::new(
            ExecutionSessionId::new("session").unwrap(),
            ExecutionId::new("run").unwrap(),
            Ok(ExecutionOutcome::Completed),
        ),
    ));
    assert_eq!(late["kind"], "late_provider_result");
    assert_eq!(late["result"], "completed");
    assert_eq!(late["executionId"], "run");
}

#[test]
fn audit_maps_a_refused_ask_as_the_bindings_decision_about_no_question() {
    let session = || ExecutionSessionId::new("session").unwrap();
    let run = || ExecutionId::new("run").unwrap();
    let id = || QuestionId::new("7").unwrap();
    for (reason, code) in [
        (QuestionRefusalReason::Unsupported, "unsupported"),
        (
            QuestionRefusalReason::UnreadableQuestion,
            "unreadable_question",
        ),
        (QuestionRefusalReason::SessionEnding, "session_ending"),
    ] {
        let value = record_value(&ExecutionAuditRecord::QuestionRefused(
            QuestionRefusalRecord::new(
                session(),
                run(),
                id(),
                reason,
                PermissionAnswerDelivery::Written,
            )
            .unwrap(),
        ));
        assert_eq!(value["kind"], "question_refused");
        assert_eq!(value["sessionId"], "session");
        assert_eq!(value["executionId"], "run");
        assert_eq!(value["reason"], code);
        assert_eq!(value["delivery"]["stage"], "written");
        assert_eq!(value["origin"]["kind"], "runtime");
        // Refused before there was an ask to keep, and chosen by nobody; its
        // own identity is what pairs its decision with its write.
        assert_eq!(value["refusalId"], "7");
        assert!(value["refused"].is_null());
        assert!(value.get("questionId").is_none());
        assert!(value.get("actor").is_none());
    }
    // A refusal for room is only ever recorded with the evidence it was
    // decided on, and takes its reason from that evidence.
    for reason in [
        QuestionRefusalReason::TooManyOpen,
        QuestionRefusalReason::TooLarge,
    ] {
        assert_eq!(
            QuestionRefusalRecord::new(
                session(),
                run(),
                id(),
                reason,
                PermissionAnswerDelivery::Written
            )
            .unwrap_err(),
            ExecutionError::InvalidQuestionRefusal
        );
    }
    let ask = deploy_question();
    let full_count = vec![ask.clone(); MAX_OPEN_QUESTIONS];
    let too_many = vec![ask.clone(); MAX_OPEN_QUESTIONS + 1];
    let large = AgentQuestion::new(
        "m",
        vec![Question::new(
            "k",
            "p",
            None,
            AnswerShape::One,
            (0..20)
                .map(|i| AnswerOption::new(format!("v{i}"), "l".repeat(949), None).unwrap())
                .collect(),
            None,
            false,
        )
        .unwrap()],
    )
    .unwrap();
    assert!(large.carrying_cost() <= MAX_OPEN_ASK_COST);
    assert!(large.carrying_cost() + ask.carrying_cost() > MAX_OPEN_ASK_COST);
    for open in [&too_many, &vec![large.clone(), ask.clone()]] {
        assert_eq!(
            RefusedAsk::new(ask.clone(), open).unwrap_err(),
            ExecutionError::InvalidQuestionRefusal,
        );
    }
    let for_room = |open: &[AgentQuestion]| {
        QuestionRefusalRecord::for_room(
            session(),
            run(),
            id(),
            RefusedAsk::new(ask.clone(), open).unwrap(),
            PermissionAnswerDelivery::Selected,
        )
    };
    for open in [&[][..], &full_count[..MAX_OPEN_QUESTIONS - 1]] {
        assert_eq!(
            for_room(open).unwrap_err(),
            ExecutionError::InvalidQuestionRefusal
        );
    }
    for (open, reason, code) in [
        (
            full_count,
            QuestionRefusalReason::TooManyOpen,
            "too_many_open",
        ),
        (vec![large], QuestionRefusalReason::TooLarge, "too_large"),
    ] {
        let selected = for_room(&open).unwrap();
        let written = selected
            .clone()
            .with_delivery(PermissionAnswerDelivery::Written);
        let failed = selected
            .clone()
            .with_delivery(PermissionAnswerDelivery::Failed(AgentError::Transport(
                "refusal write failed".into(),
            )));
        for record in [selected.clone(), written, failed] {
            assert_eq!(record.reason(), reason);
            assert_eq!(record.session_id(), selected.session_id());
            assert_eq!(record.execution_id(), selected.execution_id());
            assert_eq!(record.id(), selected.id());
            assert_eq!(record.refused(), selected.refused());
            let value = record_value(&ExecutionAuditRecord::QuestionRefused(record));
            assert_eq!(value["reason"], code);
            assert_eq!(value["sessionId"], "session");
            assert_eq!(value["executionId"], "run");
            assert_eq!(value["refusalId"], "7");
            assert_eq!(value["origin"]["kind"], "runtime");
            assert!(value.get("actor").is_none());
            let refused = &value["refused"];
            assert_eq!(refused["ask"]["message"], "Where to?");
            assert_eq!(refused["openAsks"], open.len());
            assert_eq!(
                refused["openCost"],
                open.iter().map(AgentQuestion::carrying_cost).sum::<usize>()
            );
            assert_eq!(refused["askCost"], ask.carrying_cost());
            assert_eq!(refused["maxOpenAsks"], MAX_OPEN_QUESTIONS);
            assert_eq!(refused["maxOpenCost"], MAX_OPEN_ASK_COST);
        }
    }
}

#[test]
fn audit_maps_each_unanswered_ending_of_an_ask_to_what_ended_it() {
    for (cause, code, origin) in [
        (
            QuestionCancellation::ProviderWithdrawal,
            "provider_withdrawal",
            "provider",
        ),
        (
            QuestionCancellation::ExecutionFinished,
            "execution_finished",
            "runtime",
        ),
        (
            QuestionCancellation::SessionEnded,
            "session_ended",
            "runtime",
        ),
    ] {
        let value = record_value(&ExecutionAuditRecord::QuestionAnswered(
            QuestionAnswerRecord::ended(
                admit_question(deploy_question()),
                cause,
                PermissionAnswerDelivery::Written,
            ),
        ));
        assert_eq!(value["kind"], "question_answered");
        assert_eq!(value["response"]["kind"], "cancelled");
        assert_eq!(value["response"]["cause"], code);
        assert_eq!(value["origin"]["kind"], origin);
        assert!(value.get("actor").is_none() && value["origin"].get("actor").is_none());
        assert_eq!(value["question"]["message"], "Where to?");
    }
}

fn admit_question(question: AgentQuestion) -> AdmittedQuestion {
    let mut controller = ExecutionController::new(ExecutionSessionId::new("session").unwrap());
    let execution = ExecutionId::new("run").unwrap();
    controller.begin_execution(execution.clone()).unwrap();
    controller
        .ask_question(&execution, QuestionId::new("1").unwrap(), question)
        .unwrap()
}

fn deploy_question() -> AgentQuestion {
    AgentQuestion::new(
        "Where to?",
        vec![Question::new(
            "question_0",
            "Which environment?",
            Some("Environment".into()),
            AnswerShape::One,
            vec![AnswerOption::new("staging", "Staging", Some("Safe".into())).unwrap()],
            Some("question_0_custom".into()),
            true,
        )
        .unwrap()],
    )
    .unwrap()
}

#[test]
fn audit_maps_an_answer_with_its_answerer_and_the_ask_it_answered() {
    // The ask travels with the answer, so the record alone shows the choice
    // was one the question offered, and who chose it is the verified caller.
    let chosen = |asked: AgentQuestion, value: &str| {
        QuestionAnswerRecord::chosen(
            admit_question(asked),
            Some(vec![QuestionChoice::new(
                "question_0",
                vec![value.into()],
                Some("eu".into()),
            )
            .unwrap()]),
            actor(),
            PermissionAnswerDelivery::Written,
        )
    };
    // A choice the recorded ask never offered cannot be recorded beside it,
    // whatever other question it might have been checked against.
    assert_eq!(
        chosen(deploy_question(), "production").unwrap_err(),
        ExecutionError::UnofferedAnswer
    );
    let value = record_value(&ExecutionAuditRecord::QuestionAnswered(
        chosen(deploy_question(), "staging").unwrap(),
    ));
    assert_eq!(value["response"]["kind"], "answered");
    assert_eq!(value["response"]["choices"][0]["values"][0], "staging");
    assert_eq!(value["origin"]["kind"], "client");
    assert_eq!(
        value["origin"]["actor"]["principalId"],
        "verified-principal"
    );
    let question = &value["question"]["questions"][0];
    assert_eq!(question["options"][0]["value"], "staging");
    assert_eq!(question["required"], true);
    assert_eq!(question["freeTextKey"], "question_0_custom");
    assert_eq!(question["multiSelect"], false);
}

#[test]
fn answer_audit_correlation_matches_the_admitted_question_event() {
    let mut controller = ExecutionController::new(ExecutionSessionId::new("provider").unwrap());
    let execution = ExecutionId::new("run").unwrap();
    controller.begin_execution(execution.clone()).unwrap();
    for id in ["A", "B"] {
        let admitted = controller
            .ask_question(&execution, QuestionId::new(id).unwrap(), deploy_question())
            .unwrap();
        let event = admitted.event();
        let ExecutionUpdate::QuestionAsked { id, question } = event.update() else {
            panic!("expected ask")
        };
        let records = [
            QuestionAnswerRecord::chosen(
                admitted.clone(),
                None,
                actor(),
                PermissionAnswerDelivery::Selected,
            )
            .unwrap(),
            QuestionAnswerRecord::ended(
                admitted.clone(),
                QuestionCancellation::ExecutionFinished,
                PermissionAnswerDelivery::Written,
            ),
        ];
        for record in records {
            let value = record_value(&ExecutionAuditRecord::QuestionAnswered(record));
            assert_eq!(value["sessionId"], admitted.session_id().as_str());
            assert_eq!(value["executionId"], event.execution_id().as_str());
            assert_eq!(value["questionId"], id.as_str());
            assert_eq!(value["question"]["message"], question.message());
            assert_eq!(
                value["question"]["questions"][0]["options"][0]["value"],
                question.questions()[0].options()[0].value()
            );
        }
    }
}

#[test]
fn audit_names_the_deadline_that_cancelled_a_review_it_could_not_refuse() {
    use nessa_sdk::domain::agent_execution::permissions::{
        PermissionReviewDeadline, PermissionToolClass, STANDARD_DENY_REASON,
    };
    use serde_json::json;
    use std::time::Duration;

    let id = ExecutionId::new("execution").unwrap();
    let permission = PermissionId::new("permission").unwrap();
    let deadline = PermissionReviewDeadline::new(
        PermissionToolClass::Other,
        Duration::from_secs(30),
        PermissionEffect::Deny,
        STANDARD_DENY_REASON,
    )
    .unwrap();
    let mut controller = ExecutionController::new(ExecutionSessionId::new("session").unwrap());
    controller.begin_execution(id.clone()).unwrap();
    controller
        .request_permission(
            &id,
            permission.clone(),
            ToolCallUpdate::new(
                ToolCallId::new("tool").unwrap(),
                Some("Write file".into()),
                None,
                None,
                None,
                None,
            ),
            ToolReviewInput {
                name: "Write".into(),
                arguments_json: "{}".into(),
            },
            options(),
            Some(deadline.clone().promise(1_700_000_030_000).unwrap()),
        )
        .unwrap();
    let cancellation = controller
        .cancel_deadline_for_test(&id, &permission, deadline)
        .unwrap()
        .unwrap();
    let value = record_value(&ExecutionAuditRecord::Cancelled(cancellation));
    assert_eq!(
        value["origin"],
        json!({
            "kind": "deadline",
            "actor": {
                "principalId": "permission-deadline",
                "surfaceId": "other",
                "requestId": "permission",
            },
            "basis": {
                "kind": "deadline",
                "class": "other",
                "waitMs": 30000,
                "effect": "deny",
                "reason": STANDARD_DENY_REASON,
                "stopsTurn": false,
            },
        })
    );
    assert_eq!(
        value["request"]["transition"]["reason"]["code"],
        "deadline_exceeded"
    );
}

#[test]
fn audit_maps_why_a_hooks_launch_happened() {
    use nessa_sdk::application::agent_execution::executions::{
        NativeHooksLaunchCause, NativeHooksLaunchRecord,
    };
    let open = record_value(&ExecutionAuditRecord::NativeHooksLaunch(
        NativeHooksLaunchRecord::new(
            Some("conversation".into()),
            7,
            "codex",
            true,
            NativeHooksLaunchCause::Open,
            false,
        ),
    ));
    assert_eq!(
        open,
        serde_json::json!({
            "kind": "native_hooks_launch",
            "sessionId": "conversation",
            "launchId": 7,
            "harness": "codex",
            "nativeHooks": "on",
            "cause": "open",
            "initiator": {"kind": "attachment", "sessionId": "conversation"},
            "restoredContext": false,
        })
    );
    let restart = record_value(&ExecutionAuditRecord::NativeHooksLaunch(
        NativeHooksLaunchRecord::new(
            Some("conversation".into()),
            8,
            "claude",
            false,
            NativeHooksLaunchCause::Restart,
            true,
        ),
    ));
    assert_eq!(restart["cause"], "restart");
    assert_eq!(restart["initiator"], serde_json::json!({"kind": "runtime"}));
    assert_eq!(restart["restoredContext"], true);
    assert_eq!(restart["nativeHooks"], "off");
    // An open with no session has no attachment to point at.
    let direct = record_value(&ExecutionAuditRecord::NativeHooksLaunch(
        NativeHooksLaunchRecord::new(
            None,
            9,
            "codex",
            true,
            NativeHooksLaunchCause::DirectOpen,
            false,
        ),
    ));
    assert_eq!(direct["cause"], "open");
    assert_eq!(direct["initiator"], serde_json::json!({"kind": "direct"}));
}

#[path = "audit/physical.rs"]
mod physical;

#[tokio::test]
async fn native_configuration_audit_keeps_requested_and_acknowledged_settings_separate() {
    use nessa_sdk::application::agent_execution::providers::ApprovalMode;
    let factory = Arc::new(ProviderFactory::default());
    let audit = Arc::new(RecordingModeExecutionAudit::default());
    let manager = SessionManager::open(
        Some(SessionId::new("configuration-session").unwrap()),
        Arc::new(InMemoryStorage::new()),
        Arc::new(RuntimeMessageCommitClock::new()),
    )
    .await
    .unwrap();
    let agent = Agent::prepare(
        Arc::new(Provider::new(factory.clone())),
        manager,
        audit.clone(),
    )
    .await
    .unwrap();
    let attachment = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    agent
        .start_attachment(attachment)
        .unwrap()
        .wait()
        .await
        .unwrap();
    agent
        .set_approval_mode(
            ApprovalMode::Auto,
            ActionContext::new("person", "window", "approval-choice").unwrap(),
        )
        .await
        .unwrap();
    let target = Arc::new(Provider::new(factory.clone()).with_model("second-model"));
    agent
        .set_model(
            target.clone(),
            ActionContext::new("person", "window", "model-choice").unwrap(),
        )
        .await
        .unwrap();
    assert!(agent
        .set_model(
            target,
            ActionContext::new("person", "window", "unchanged-model").unwrap()
        )
        .await
        .is_err());
    *factory.mode_failure.lock().unwrap() =
        Some(AgentError::Transport("controlled preset failure".into()));
    assert!(agent
        .set_approval_mode(
            ApprovalMode::Full,
            ActionContext::new("person", "window", "failed-preset").unwrap()
        )
        .await
        .is_err());
    let values = audit
        .records
        .lock()
        .unwrap()
        .iter()
        .filter(|record| matches!(record, ExecutionAuditRecord::ConfigurationControlled(_)))
        .map(record_value)
        .collect::<Vec<_>>();
    let model = values
        .iter()
        .filter(|value| value["actor"]["requestId"] == "model-choice")
        .collect::<Vec<_>>();
    assert_eq!(model.len(), 3);
    for record in &model {
        assert_eq!(record["kind"], "configuration_control");
        assert_eq!(record["sessionId"], "configuration-session");
        assert!(!record["attachmentGeneration"].as_str().unwrap().is_empty());
        assert_eq!(record["before"]["provider"]["name"], "gateway-test");
        assert_eq!(record["before"]["provider"]["modelId"], "test");
        assert_eq!(record["before"]["provider"]["context"], "test");
        assert_eq!(record["before"]["configuration"]["approvalMode"], "auto");
        assert_eq!(record["request"]["kind"], "model");
        assert_eq!(record["request"]["provider"]["modelId"], "second-model");
        assert_eq!(record["actor"]["principalId"], "person");
        assert_eq!(record["actor"]["surfaceId"], "window");
        assert_eq!(record["cause"], "caller_requested");
    }
    assert_eq!(model[0]["stage"], "requested");
    assert_eq!(model[0]["acknowledged"], Value::Null);
    for record in &model[1..] {
        assert_eq!(
            record["acknowledged"]["provider"]["modelId"],
            "second-model"
        );
        assert_eq!(
            record["acknowledged"]["configuration"]["approvalMode"],
            "ask"
        );
    }
    assert_eq!(model[1]["stage"], "acknowledged");
    assert_eq!(model[2]["stage"], "applied");
    for (request, stage) in [("unchanged-model", "refused"), ("failed-preset", "failed")] {
        let record = values
            .iter()
            .find(|value| value["actor"]["requestId"] == request && value["stage"] == stage)
            .unwrap();
        assert_eq!(record["acknowledged"], Value::Null);
        assert_eq!(record["before"]["provider"]["modelId"], "second-model");
    }
    let approval = values
        .iter()
        .find(|value| {
            value["actor"]["requestId"] == "approval-choice" && value["stage"] == "applied"
        })
        .unwrap();
    assert_eq!(approval["request"]["kind"], "approval_mode");
    assert_eq!(approval["request"]["approvalMode"], "auto");
    assert_eq!(
        approval["acknowledged"]["configuration"]["approvalMode"],
        "auto"
    );
    agent.close(actor()).await.unwrap();
}
