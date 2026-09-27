use super::super::fields::string;
use crate::application::agent_execution::agents::AgentError;
use crate::domain::agent_execution::executions::ExecutionOutcome;
use crate::infrastructure::json_rpc::protocol;
use serde_json::Value;
pub(crate) fn outcome(value: &Value) -> Result<ExecutionOutcome, AgentError> {
    match string(value, "stopReason")? {
        "end_turn" => Ok(ExecutionOutcome::Completed),
        "max_tokens" => Ok(ExecutionOutcome::OutputLimit),
        "max_turn_requests" => Ok(ExecutionOutcome::RequestLimit),
        "refusal" => Ok(ExecutionOutcome::Refused),
        "cancelled" => Ok(ExecutionOutcome::Cancelled),
        _ => Err(protocol("unknown prompt stop reason")),
    }
}
