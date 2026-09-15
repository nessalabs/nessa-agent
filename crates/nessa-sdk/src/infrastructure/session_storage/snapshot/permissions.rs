use super::tools::corrupt;
use crate::application::agent_execution::{
    executions::limits::validate_observation_id, permissions::*, sessions::storage::StorageError,
    tools::ToolReviewInput,
};
use crate::domain::agent_execution::{
    executions::ExecutionId, permissions::*, sessions::ExecutionSessionId, tools::ToolCallId,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Actor {
    principal_id: String,
    surface_id: String,
    request_id: String,
}
impl From<&ActionContext> for Actor {
    fn from(value: &ActionContext) -> Self {
        Self {
            principal_id: value.principal_id().into(),
            surface_id: value.surface_id().into(),
            request_id: value.request_id().into(),
        }
    }
}
impl Actor {
    pub(super) fn decode(self) -> Result<ActionContext, StorageError> {
        ActionContext::new(self.principal_id, self.surface_id, self.request_id).map_err(corrupt)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Input {
    name: String,
    arguments_json: String,
}
impl From<&ToolReviewInput> for Input {
    fn from(value: &ToolReviewInput) -> Self {
        Self {
            name: value.name.clone(),
            arguments_json: value.arguments_json.clone(),
        }
    }
}
impl From<Input> for ToolReviewInput {
    fn from(value: Input) -> Self {
        Self {
            name: value.name,
            arguments_json: value.arguments_json,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Choice {
    id: String,
    label: String,
    effect: Effect,
    scope: Scope,
}
#[derive(Serialize, Deserialize)]
enum Effect {
    Allow,
    Deny,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Scope {
    Request,
    Session {
        application_id: String,
        session_id: String,
    },
    Application(String),
}
pub(super) fn choices(options: &PermissionOptions) -> Vec<Choice> {
    options
        .choices()
        .iter()
        .map(|value| Choice {
            id: value.id().as_str().into(),
            label: value.label().into(),
            effect: match value.decision().effect() {
                PermissionEffect::Allow => Effect::Allow,
                PermissionEffect::Deny => Effect::Deny,
            },
            scope: match value.decision().scope().view() {
                PermissionScopeView::Request => Scope::Request,
                PermissionScopeView::Application(id) => Scope::Application(id.as_str().into()),
                PermissionScopeView::Session {
                    application_id,
                    session_id,
                } => Scope::Session {
                    application_id: application_id.as_str().into(),
                    session_id: session_id.as_str().into(),
                },
            },
        })
        .collect()
}
pub(super) fn decode_choices(values: Vec<Choice>) -> Result<PermissionOptions, StorageError> {
    let mut options = Vec::with_capacity(values.len());
    for value in values {
        let scope = match value.scope {
            Scope::Request => PermissionScope::request(),
            Scope::Application(id) => {
                PermissionScope::application(PermissionApplicationId::new(id).map_err(corrupt)?)
            }
            Scope::Session {
                application_id,
                session_id,
            } => PermissionScope::session(
                PermissionApplicationId::new(application_id).map_err(corrupt)?,
                PermissionSessionId::new(session_id).map_err(corrupt)?,
            ),
        };
        let decision = PermissionDecision::new(
            match value.effect {
                Effect::Allow => PermissionEffect::Allow,
                Effect::Deny => PermissionEffect::Deny,
            },
            scope,
        );
        options.push(
            PermissionOption::new(
                PermissionOptionId::new(value.id).map_err(corrupt)?,
                value.label,
                decision,
            )
            .map_err(corrupt)?,
        );
    }
    // Distinct option identities may offer the same decision. Reconstruct the
    // exact policy once in first-seen order without scanning the growing list.
    let mut seen = HashSet::with_capacity(options.len());
    let decisions = options
        .iter()
        .map(PermissionOption::decision)
        .filter(|decision| seen.insert(*decision))
        .cloned()
        .collect();
    let policy = PermissionOfferPolicy::new(decisions).map_err(corrupt)?;
    PermissionOptions::new(options, &policy).map_err(corrupt)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cancellation {
    session_id: String,
    id: String,
    execution_id: String,
    tool_id: String,
    options: Vec<Choice>,
    input: Input,
    origin: Origin,
    reason: Reason,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Origin {
    Client(Actor),
    Provider,
    Runtime,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Reason {
    ProviderWithdrawal,
    SessionClosed,
    SessionFailed,
    ExecutionFinished,
    ExecutionFailed,
    DeadlineExceeded,
    EventConsumerDropped,
    SessionHandlesDropped,
    Custom { code: String, explanation: String },
}
impl From<&PermissionCancellation> for Cancellation {
    fn from(value: &PermissionCancellation) -> Self {
        let reason = match value.request().state() {
            PermissionStateView::Cancelled { reason } => reason,
            _ => unreachable!("application cancellation is validated"),
        };
        Self {
            session_id: value.session_id().as_str().into(),
            id: value.request().id().as_str().into(),
            execution_id: value.request().execution_id().as_str().into(),
            tool_id: value.request().tool_id().as_str().into(),
            options: choices(value.request().options()),
            input: value.input().into(),
            origin: match value.origin() {
                CancellationOrigin::Client(actor) => Origin::Client(actor.into()),
                CancellationOrigin::Provider => Origin::Provider,
                CancellationOrigin::Runtime => Origin::Runtime,
            },
            reason: match reason.view() {
                PermissionCancellationReasonView::ProviderWithdrawal => Reason::ProviderWithdrawal,
                PermissionCancellationReasonView::SessionClosed => Reason::SessionClosed,
                PermissionCancellationReasonView::ExecutionFinished => Reason::ExecutionFinished,
                PermissionCancellationReasonView::ExecutionFailed => Reason::ExecutionFailed,
                PermissionCancellationReasonView::SessionFailed => Reason::SessionFailed,
                PermissionCancellationReasonView::DeadlineExceeded => Reason::DeadlineExceeded,
                PermissionCancellationReasonView::EventConsumerDropped => {
                    Reason::EventConsumerDropped
                }
                PermissionCancellationReasonView::SessionHandlesDropped => {
                    Reason::SessionHandlesDropped
                }
                PermissionCancellationReasonView::Custom(reason) => Reason::Custom {
                    code: reason.code().into(),
                    explanation: reason.explanation().into(),
                },
            },
        }
    }
}
impl Cancellation {
    pub(super) fn decode(self) -> Result<PermissionCancellation, StorageError> {
        validate_observation_id(&self.id).map_err(corrupt)?;
        validate_observation_id(&self.tool_id).map_err(corrupt)?;
        let mut request = PermissionRequest::new(
            PermissionId::new(self.id).map_err(corrupt)?,
            ExecutionId::new(self.execution_id).map_err(corrupt)?,
            ToolCallId::new(self.tool_id).map_err(corrupt)?,
            decode_choices(self.options)?,
        );
        let reason = match self.reason {
            Reason::ProviderWithdrawal => PermissionCancellationReason::provider_withdrawal(),
            Reason::SessionClosed => PermissionCancellationReason::session_closed(),
            Reason::ExecutionFinished => PermissionCancellationReason::execution_finished(),
            Reason::ExecutionFailed => PermissionCancellationReason::execution_failed(),
            Reason::SessionFailed => PermissionCancellationReason::session_failed(),
            Reason::DeadlineExceeded => PermissionCancellationReason::deadline_exceeded(),
            Reason::EventConsumerDropped => PermissionCancellationReason::event_consumer_dropped(),
            Reason::SessionHandlesDropped => {
                PermissionCancellationReason::session_handles_dropped()
            }
            Reason::Custom { code, explanation } => PermissionCancellationReason::custom(
                CustomPermissionCancellationReason::new(code, explanation).map_err(corrupt)?,
            ),
        };
        request.cancel(reason).map_err(corrupt)?;
        PermissionCancellation::from_record(
            ExecutionSessionId::new(self.session_id).map_err(corrupt)?,
            request,
            self.input.into(),
            match self.origin {
                Origin::Client(actor) => CancellationOrigin::Client(actor.decode()?),
                Origin::Provider => CancellationOrigin::Provider,
                Origin::Runtime => CancellationOrigin::Runtime,
            },
        )
        .map_err(corrupt)
    }
}
