//! Explicit mapping retains target, transition, cause, actor, and delivery separately.
use nessa_sdk::application::agent_execution::{
    executions::{
        AdmissionAuditCause, AdmissionAuditStage, AttachmentAuditCause, AttachmentAuditStage,
        ExecutionAuditRecord, QueueOrderCause, SteeringAuditCause, SteeringAuditStage,
    },
    permissions::{
        ActionContext, ApprovalBasis, CancellationOrigin, PermissionAnswerDelivery,
        QuestionRefusalRecord, ReviewDeclineRecord,
    },
};
use nessa_sdk::domain::agent_execution::{
    executions::{
        ExecutionOutcome, InvocationKind, InvocationStage, SchedulingCause, SubmissionMode,
    },
    permissions::{
        PermissionCancellationReason, PermissionCancellationReasonView, PermissionDecision,
        PermissionEffect, PermissionRequest, PermissionScopeView, PermissionStateView,
        ReviewDeclineReason,
    },
    questions::{
        AgentQuestion, AnswerShape, QuestionCancellation, QuestionRefusalReason, QuestionResponse,
    },
    sessions::AttachmentCause,
};
use serde_json::{json, Value};

pub(super) fn record_value(record: &ExecutionAuditRecord) -> Value {
    match record {
        ExecutionAuditRecord::Attachment(record) => {
            json!({
                "kind":"attachment_transition",
                "sessionId":record.session_id().as_str(),
                "generation":record.generation(),
                "before":attachment_stage(record.before()),
                "after":attachment_stage(record.after()),
                "cause":attachment_cause(record.cause()),
                "actor":record.actor().map(actor),
            })
        }
        ExecutionAuditRecord::QueueAdmitted(record) => {
            json!({
                "kind":"queue_admitted",
                "sessionId":record.session_id().as_str(),
                "executionId":record.execution_id().as_str(),
                "mode":submission_mode(record.mode()),
                "before":admission_stage(record.before()),
                "after":admission_stage(record.after()),
                "cause":match record.cause() { AdmissionAuditCause::Submitted => "submitted" },
                "actor":actor(record.actor()),
            })
        }
        ExecutionAuditRecord::QueueSettled(record) => {
            json!({
                "kind":"queue_settled",
                "sessionId":record.session_id().as_str(),
                "executionId":record.execution_id().as_str(),
                "target":record.target().map(|id| id.as_str()),
                "mode":submission_mode(record.mode()),
                "before":record.before().map(invocation_stage),
                "after":invocation_stage(record.after()),
                "cause":scheduling_cause(record.cause()),
                "submittedBy":actor(record.submitted_by()),
                "origin":record.initiated_by().map_or_else(
                    || json!({"kind":"runtime"}),
                    |initiator| json!({"kind":"client","actor":actor(initiator)}),
                ),
            })
        }
        ExecutionAuditRecord::SteeringAcknowledged(record) => {
            json!({
                "kind":"steering_acknowledged",
                "sessionId":record.session_id().as_str(),
                "executionId":record.execution_id().as_str(),
                "target":record.target().as_str(),
                "before":steering_stage(record.before()),
                "after":steering_stage(record.after()),
                "cause":match record.cause() { SteeringAuditCause::ProviderAcknowledged => "provider_acknowledged" },
                "actor":actor(record.actor()),
            })
        }
        ExecutionAuditRecord::QueueReordered(record) => {
            let change = record.change();
            json!({
                "kind":"queue_reorder_selected",
                "sessionId":record.session_id().as_str(),
                "before":change.before().iter().map(|(id, priority)| json!({
                    "executionId":id.as_str(),
                    "kind":match priority { InvocationKind::Steering => "steering", InvocationKind::Queued => "queued" },
                })).collect::<Vec<_>>(),
                "after":change.after().iter().map(|id|id.as_str()).collect::<Vec<_>>(),
                "cause":match record.cause() { QueueOrderCause::CallerRequested => "caller_requested" },
                "actor":actor(record.actor()),
            })
        }
        ExecutionAuditRecord::Finished(finish) => {
            json!({"kind":"execution_finished","sessionId":finish.session_id().as_str(),"executionId":finish.execution_id().as_str(),"before":"active","after":"released","origin":{"kind":"runtime"},"result":match finish.result() { Ok(outcome)=>json!({"outcome":match outcome {ExecutionOutcome::Completed=>"completed",ExecutionOutcome::OutputLimit=>"output_limit",ExecutionOutcome::RequestLimit=>"request_limit",ExecutionOutcome::Refused=>"refused",ExecutionOutcome::Cancelled=>"cancelled"}}),Err(cause)=>json!({"failure":reason(cause)}) }})
        }
        ExecutionAuditRecord::SessionClosed(record) => {
            let closure = record.closure();
            json!({"kind":"session_closed","sessionId":closure.session_id().as_str(),"executionId":closure.execution_id().map(|id|id.as_str()),"before":"open","after":"closed","reason":reason(closure.reason()),"origin":origin(record.origin())})
        }
        ExecutionAuditRecord::Cancelled(record) => {
            json!({"kind":"permission_cancelled","sessionId":record.session_id().as_str(),"request":permission(record.request()),"input":{"name":record.input().name,"argumentsJson":record.input().arguments_json},"origin":origin(record.origin())})
        }
        ExecutionAuditRecord::Answered(record) => {
            let resolution = record.resolution();
            let basis = match resolution.attribution().basis() {
                ApprovalBasis::Explicit => json!({"kind":"explicit"}),
                ApprovalBasis::Mode(mode) => {
                    json!({"kind":"mode","name":mode.name(),"revision":mode.configuration_revision()})
                }
                ApprovalBasis::Rule(rule) => {
                    json!({"kind":"rule","ruleId":rule.rule_id(),"revision":rule.revision(),"grantedBy":actor(rule.granted_by())})
                }
            };
            let delivery = delivery(record.delivery());
            json!({"kind":"permission_answered","sessionId":record.session_id().as_str(),"request":permission(resolution.request()),"input":{"name":resolution.input().name,"argumentsJson":resolution.input().arguments_json},"actor":actor(resolution.attribution().actor()),"basis":basis,"delivery":delivery})
        }
        ExecutionAuditRecord::ReviewDeclined(record) => declined(record),
        ExecutionAuditRecord::QuestionRefused(record) => refused(record),
        ExecutionAuditRecord::QuestionAnswered(record) => {
            json!({
                "kind":"question_answered",
                "sessionId":record.session_id().as_str(),
                "executionId":record.execution_id().as_str(),
                "questionId":record.question_id().as_str(),
                "question":asked(record.question()),
                "response":match record.response() {
                    QuestionResponse::Declined => json!({"kind":"declined"}),
                    QuestionResponse::Answered(answer) => json!({
                        "kind":"answered",
                        "choices":answer.choices().iter().map(|choice| json!({
                            "key":choice.key(),
                            "values":choice.values().collect::<Vec<_>>(),
                            "ownWords":choice.own_words(),
                        })).collect::<Vec<_>>(),
                    }),
                    QuestionResponse::Cancelled(cause) => json!({
                        "kind":"cancelled",
                        "cause":match cause {
                            QuestionCancellation::ProviderWithdrawal => "provider_withdrawal",
                            QuestionCancellation::ExecutionFinished => "execution_finished",
                            QuestionCancellation::SessionEnded => "session_ended",
                        },
                    }),
                },
                "delivery":delivery(record.delivery()),
                // The initiator is whoever actually ended the ask. An answer or
                // a decline names its verified caller; a cancellation had none,
                // and is labelled by what caused it rather than given one.
                "origin":match (record.actor(), record.response()) {
                    (Some(caller), _) => json!({"kind":"client","actor":actor(caller)}),
                    (None, QuestionResponse::Cancelled(QuestionCancellation::ProviderWithdrawal)) => {
                        json!({"kind":"provider"})
                    }
                    (None, _) => json!({"kind":"runtime"}),
                },
            })
        }
    }
}
fn attachment_stage(stage: AttachmentAuditStage) -> &'static str {
    match stage {
        AttachmentAuditStage::Absent => "absent",
        AttachmentAuditStage::Waiting => "waiting",
        AttachmentAuditStage::Starting => "starting",
        AttachmentAuditStage::ContextPublished => "context_published",
        AttachmentAuditStage::Attached => "attached",
        AttachmentAuditStage::Failed => "failed",
    }
}
fn attachment_cause(cause: AttachmentAuditCause) -> Value {
    match cause {
        AttachmentAuditCause::Started(cause) => json!({
            "kind":"started",
            "attachmentCause":match cause {
                AttachmentCause::Initial => "initial",
                AttachmentCause::Reopen => "reopen",
                AttachmentCause::AutomaticRecovery => "automatic_recovery",
            },
        }),
        AttachmentAuditCause::AuthorizationAbandoned => {
            json!({"kind":"authorization_abandoned"})
        }
        AttachmentAuditCause::Closed => json!({"kind":"closed"}),
        AttachmentAuditCause::Published => json!({"kind":"published"}),
        AttachmentAuditCause::Failed => json!({"kind":"failed"}),
    }
}
fn admission_stage(stage: AdmissionAuditStage) -> &'static str {
    match stage {
        AdmissionAuditStage::Unowned => "unowned",
        AdmissionAuditStage::Owned => "owned",
    }
}
fn invocation_stage(stage: InvocationStage) -> &'static str {
    match stage {
        InvocationStage::Queued => "queued",
        InvocationStage::Running => "running",
        InvocationStage::Injected => "injected",
        InvocationStage::Settled => "settled",
        InvocationStage::Cancelled => "cancelled",
    }
}
fn scheduling_cause(cause: SchedulingCause) -> &'static str {
    match cause {
        SchedulingCause::Submitted => "submitted",
        SchedulingCause::Dispatched => "dispatched",
        SchedulingCause::DispatchFailed => "dispatch_failed",
        SchedulingCause::SteeringInjected => "steering_injected",
        SchedulingCause::ExecutionSettled => "execution_settled",
        SchedulingCause::ExecutionFailed => "execution_failed",
        SchedulingCause::SessionClosed => "session_closed",
        SchedulingCause::RunnerStopped => "runner_stopped",
        SchedulingCause::Withdrawn => "withdrawn",
    }
}
fn steering_stage(stage: SteeringAuditStage) -> &'static str {
    match stage {
        SteeringAuditStage::Pending => "pending",
        SteeringAuditStage::Injected => "injected",
    }
}
fn submission_mode(mode: SubmissionMode) -> &'static str {
    match mode {
        SubmissionMode::Immediate => "immediate",
        SubmissionMode::Queued => "queued",
        SubmissionMode::BoundarySteering => "boundary_steering",
        SubmissionMode::Steering => "steering",
    }
}
/// An ask the binding refused before anybody was asked.
///
/// Its own identity, minted for the refused request, pairs its decision with
/// its write. No actor, because nobody chose it: the binding did, which
/// `origin` says.
fn refused(record: &QuestionRefusalRecord) -> Value {
    json!({
        "kind":"question_refused",
        "sessionId":record.session_id().as_str(),
        "executionId":record.execution_id().as_str(),
        "refusalId":record.id().as_str(),
        "reason":match record.reason() {
            QuestionRefusalReason::TooManyOpen => "too_many_open",
            QuestionRefusalReason::Unsupported => "unsupported",
            QuestionRefusalReason::UnreadableQuestion => "unreadable_question",
            QuestionRefusalReason::SessionEnding => "session_ending",
        },
        "delivery":delivery(record.delivery()),
        "origin":{"kind":"runtime"},
    })
}
/// The ask as it was asked, kept beside every answer and ending of it: what
/// was offered, what was required, and where own words were to go — so the
/// record alone shows a choice was one the question offered.
fn asked(question: &AgentQuestion) -> Value {
    json!({
        "message":question.message(),
        "questions":question.questions().iter().map(|asked| json!({
            "key":asked.key(),
            "prompt":asked.prompt(),
            "header":asked.header(),
            "multiSelect":asked.shape() == AnswerShape::Many,
            "freeTextKey":asked.free_text_key(),
            "required":asked.required(),
            "options":asked.options().iter().map(|option| json!({
                "value":option.value(),
                "label":option.label(),
                "description":option.description(),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}
/// A review the binding refused before anyone was offered it.
///
/// There is no request and no actor here, and neither is omitted by accident: a
/// decline happens before a request exists, and nobody chose it. The tool name
/// is the provider's claim about its own frame, never checked against what was
/// observed, so the field says `declaredTool` rather than `tool`: a reader
/// deciding anything on it should know whose word it is. It is `null` where the
/// frame named the tool in a way not worth retaining.
fn declined(record: &ReviewDeclineRecord) -> Value {
    let decline = record.decline();
    json!({
        "kind":"review_declined",
        "sessionId":record.session_id().as_str(),
        "executionId":record.execution_id().as_str(),
        "declineId":record.id().as_str(),
        "declaredTool":decline.declared(),
        "reason":match decline.reason() {
            ReviewDeclineReason::ToolNotReviewable => "tool_not_reviewable",
            ReviewDeclineReason::UnreadableRequest => "unreadable_request",
            ReviewDeclineReason::UnusableOptions => "unusable_options",
        },
        "delivery":delivery(record.delivery()),
        "origin":{"kind":"runtime"},
    })
}
/// One vocabulary for how a decision reached the provider, answered or refused.
fn delivery(value: &PermissionAnswerDelivery) -> Value {
    match value {
        PermissionAnswerDelivery::Selected => json!({"stage":"selected"}),
        PermissionAnswerDelivery::Written => json!({"stage":"written"}),
        PermissionAnswerDelivery::Failed(error) => {
            json!({"stage":"failed","diagnostic":error.to_string()})
        }
    }
}
fn actor(actor: &ActionContext) -> Value {
    json!({"principalId":actor.principal_id(),"surfaceId":actor.surface_id(),"requestId":actor.request_id()})
}
fn origin(value: &CancellationOrigin) -> Value {
    match value {
        CancellationOrigin::Client(value) => json!({"kind":"client","actor":actor(value)}),
        CancellationOrigin::Provider => json!({"kind":"provider"}),
        CancellationOrigin::Runtime => json!({"kind":"runtime"}),
    }
}
fn reason(value: &PermissionCancellationReason) -> Value {
    let code = match value.view() {
        PermissionCancellationReasonView::ProviderWithdrawal => "provider_withdrawal",
        PermissionCancellationReasonView::SessionClosed => "session_closed",
        PermissionCancellationReasonView::SessionFailed => "session_failed",
        PermissionCancellationReasonView::ExecutionFinished => "execution_finished",
        PermissionCancellationReasonView::ExecutionFailed => "execution_failed",
        PermissionCancellationReasonView::DeadlineExceeded => "deadline_exceeded",
        PermissionCancellationReasonView::EventConsumerDropped => "event_consumer_dropped",
        PermissionCancellationReasonView::SessionHandlesDropped => "session_handles_dropped",
        PermissionCancellationReasonView::Custom(value) => {
            return json!({"code":value.code(),"explanation":value.explanation()})
        }
    };
    json!({"code":code})
}
fn decision(value: &PermissionDecision) -> Value {
    let scope = match value.scope().view() {
        PermissionScopeView::Request => json!({"kind":"request"}),
        PermissionScopeView::Application(id) => {
            json!({"kind":"application","applicationId":id.as_str()})
        }
        PermissionScopeView::Session {
            application_id,
            session_id,
        } => {
            json!({"kind":"session","applicationId":application_id.as_str(),"sessionId":session_id.as_str()})
        }
    };
    json!({"effect":match value.effect(){PermissionEffect::Allow=>"allow",PermissionEffect::Deny=>"deny"},"scope":scope})
}
fn permission(value: &PermissionRequest) -> Value {
    let state = match value.state() {
        PermissionStateView::Pending => json!({"after":"pending"}),
        PermissionStateView::Answered {
            option_id,
            decision: chosen,
        } => {
            json!({"before":"pending","after":"answered","optionId":option_id.as_str(),"decision":decision(chosen)})
        }
        PermissionStateView::Cancelled { reason: cause } => {
            json!({"before":"pending","after":"cancelled","reason":reason(cause)})
        }
    };
    json!({"permissionId":value.id().as_str(),"executionId":value.execution_id().as_str(),"toolId":value.tool_id().as_str(),"options":value.options().choices().iter().map(|option|json!({"id":option.id().as_str(),"label":option.label(),"decision":decision(option.decision())})).collect::<Vec<_>>(),"transition":state})
}
