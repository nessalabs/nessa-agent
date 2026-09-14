use crate::application::agent_execution::{
    agents::AgentError,
    providers::{ProviderOperationResult, SteeringOutcome},
};
use crate::infrastructure::json_rpc;
use serde_json::Value;
use std::time::Duration;
use tokio::{sync::oneshot, time::Instant};

/// Bound extension acknowledgements even when the execution itself has no timeout.
pub(super) const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

/// One sent steering request. Keeping its reply until response or teardown makes
/// ambiguous delivery visible and prevents admission of a newer prompt meanwhile.
pub(super) struct PendingSteering {
    pub id: i64,
    pub reply: oneshot::Sender<ProviderOperationResult<SteeringOutcome>>,
    pub deadline: Instant,
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
