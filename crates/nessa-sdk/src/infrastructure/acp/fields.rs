use crate::application::agent_execution::agents::AgentError;
use crate::infrastructure::json_rpc::protocol;
use serde_json::Value;

/// The longest provider identifier (a session's, a tool call's) kept.
pub(crate) const MAX_IDENTIFIER_BYTES: usize = 256;

pub(crate) fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str, AgentError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| protocol(&format!("missing {field}")))
}
pub(crate) fn identifier<'a>(value: &'a Value, field: &str) -> Result<&'a str, AgentError> {
    let value = string(value, field)?;
    if value.len() > MAX_IDENTIFIER_BYTES {
        return Err(protocol("identifier exceeds limit"));
    }
    Ok(value)
}
