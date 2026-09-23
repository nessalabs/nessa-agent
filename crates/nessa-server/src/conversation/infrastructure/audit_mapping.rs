//! Explicit mapping retains target, transition, cause, actor, and delivery separately.
use nessa_sdk::application::agent_execution::{
    executions::{ExecutionAuditRecord, QueueOrderCause},
    permissions::{
        ActionContext, ApprovalBasis, CancellationOrigin, PermissionAnswerDelivery,
        ReviewDeclineRecord,
    },
};
use nessa_sdk::domain::agent_execution::{
    executions::{ExecutionOutcome, InvocationKind},
    permissions::{
        PermissionCancellationReason, PermissionCancellationReasonView, PermissionDecision,
        PermissionEffect, PermissionRequest, PermissionScopeView, PermissionStateView,
        ReviewDeclineReason,
    },
};
use serde_json::{json, Value};

pub(super) fn record_value(record: &ExecutionAuditRecord) -> Value {
    match record {
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
    }
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
