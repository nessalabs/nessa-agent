use super::*;
use nessa_sdk::application::agent_execution::{
    executions::{
        AttachmentAuditCause, AttachmentAuditRecord, AttachmentAuditStage, ExecutionController,
        QueueAdmissionRecord, QueueOrderRecord, QueueSettlementRecord, SessionClosureRecord,
        SteeringAcknowledgementRecord,
    },
    permissions::{
        ActionContext, ApprovalAttribution, ApprovalBasis, CancellationOrigin, PermissionAnswer,
        PermissionAnswerDelivery, PermissionAnswerRecord, QuestionAnswerRecord,
        QuestionRefusalRecord, RefusedAsk, ReviewDeclineRecord,
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
    questions::{
        AgentQuestion, AnswerOption, AnswerShape, Question, QuestionCancellation, QuestionChoice,
        QuestionId, QuestionRefusalReason, MAX_OPEN_ASK_COST, MAX_OPEN_QUESTIONS,
    },
    tools::{ToolCallId, ToolCallUpdate},
    ExecutionError,
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
            7,
            AttachmentAuditStage::Waiting,
            AttachmentAuditStage::Starting,
            AttachmentAuditCause::Started(AttachmentCause::Reopen),
            Some(actor()),
        ),
    ));
    assert_eq!(attachment["kind"], "attachment_transition");
    assert_eq!(attachment["generation"], 7);
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
    let for_room = |open_asks, open_cost| {
        QuestionRefusalRecord::for_room(
            session(),
            run(),
            id(),
            RefusedAsk::new(deploy_question(), open_asks, open_cost),
            PermissionAnswerDelivery::Written,
        )
    };
    // Evidence showing there was room records no refusal at all.
    assert_eq!(
        for_room(MAX_OPEN_QUESTIONS - 1, 0).unwrap_err(),
        ExecutionError::InvalidQuestionRefusal
    );
    let too_large = MAX_OPEN_ASK_COST - ask.carrying_cost() + 1;
    for ((open_asks, open_cost), code) in [
        ((MAX_OPEN_QUESTIONS, 0), "too_many_open"),
        ((1, too_large), "too_large"),
    ] {
        let value = record_value(&ExecutionAuditRecord::QuestionRefused(
            for_room(open_asks, open_cost).unwrap(),
        ));
        assert_eq!(value["reason"], code);
        let refused = &value["refused"];
        assert_eq!(refused["ask"]["message"], "Where to?");
        assert_eq!(refused["openAsks"], open_asks);
        assert_eq!(refused["openCost"], open_cost);
        assert_eq!(refused["askCost"], ask.carrying_cost());
        assert_eq!(refused["maxOpenAsks"], MAX_OPEN_QUESTIONS);
        assert_eq!(refused["maxOpenCost"], MAX_OPEN_ASK_COST);
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
                ExecutionSessionId::new("session").unwrap(),
                ExecutionId::new("run").unwrap(),
                QuestionId::new("1").unwrap(),
                deploy_question(),
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
            ExecutionSessionId::new("session").unwrap(),
            ExecutionId::new("run").unwrap(),
            QuestionId::new("1").unwrap(),
            asked,
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
