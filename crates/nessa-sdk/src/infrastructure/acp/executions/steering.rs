use crate::application::agent_execution::{
    agents::AgentError,
    providers::{ProviderOperationResult, SteeringOutcome},
};
use crate::infrastructure::clock::ClockInstant;
use crate::infrastructure::json_rpc;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::oneshot;

/// Bound extension acknowledgements even when the execution itself has no timeout.
///
/// This is the whole of what one steering call may spend. Reading its images
/// happens first, on the calling task, and spends part of it; the worker arms
/// the acknowledgement deadline with what is left rather than with a fresh
/// interval. The one thing added on top is the extra write time a frame of a
/// mebibyte or more is allowed, so steering that carries images is not failed
/// by its own size: the total bound is this interval plus one second for each
/// whole mebibyte of the frame.
pub(in crate::infrastructure::acp) const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

/// One sent steering request. Keeping its reply until response or teardown makes
/// ambiguous delivery visible and prevents admission of a newer prompt meanwhile.
pub(super) struct PendingSteering {
    pub id: i64,
    pub reply: oneshot::Sender<ProviderOperationResult<SteeringOutcome>>,
    pub deadline: ClockInstant,
}
pub(super) fn outcome(value: Value) -> Result<SteeringOutcome, AgentError> {
    match value.get("outcome").and_then(Value::as_str) {
        Some("injected") => Ok(SteeringOutcome::Injected),
        Some("promptRequired")
            if value.get("reason").and_then(Value::as_str) == Some("noRunningTurn") =>
        {
            Ok(SteeringOutcome::PromptRequired)
        }
        _ => Err(json_rpc::protocol("invalid or detached steering outcome")),
    }
}
