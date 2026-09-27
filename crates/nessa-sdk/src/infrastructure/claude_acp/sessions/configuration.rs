use crate::application::agent_execution::agents::AgentError;
use crate::infrastructure::json_rpc::protocol;
use serde_json::Value;
pub(super) fn verify_config(
    result: &Value,
    model: &str,
    require_mode: bool,
) -> Result<(), AgentError> {
    let options = result
        .get("configOptions")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol("missing session config options"))?;
    let mut model_option = None;
    let mut mode_option = None;
    for option in options {
        let selected = match option.get("id").and_then(Value::as_str) {
            Some("model") => &mut model_option,
            Some("mode") => &mut mode_option,
            _ => continue,
        };
        if selected.replace(option).is_some() {
            return Err(protocol("duplicate model or mode config option"));
        }
    }
    // Establish an unambiguous selection before interpreting either value. Mode
    // duplicates are invalid even before the host applies its initial default.
    if current(model_option) != Some(model) {
        return Err(protocol(
            "provider did not select the exact configured model",
        ));
    }
    if require_mode && current(mode_option) != Some("default") {
        return Err(protocol("provider permission mode is not default"));
    }
    Ok(())
}

fn current(option: Option<&Value>) -> Option<&str> {
    option
        .and_then(|option| option.get("currentValue"))
        .and_then(Value::as_str)
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/claude_acp/sessions/configuration.rs"]
mod tests;
