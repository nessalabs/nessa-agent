use crate::application::agent_execution::agents::AgentError;
use crate::infrastructure::json_rpc::protocol;
use serde_json::Value;
pub(crate) fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str, AgentError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| protocol(&format!("missing {field}")))
}
pub(crate) fn identifier<'a>(value: &'a Value, field: &str) -> Result<&'a str, AgentError> {
    let value = string(value, field)?;
    if value.len() > 256 {
        return Err(protocol("identifier exceeds limit"));
    }
    Ok(value)
}
