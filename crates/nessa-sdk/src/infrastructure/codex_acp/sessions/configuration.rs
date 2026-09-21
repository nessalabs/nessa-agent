use crate::application::agent_execution::agents::AgentError;
use crate::infrastructure::json_rpc::protocol;
use serde_json::Value;

/// Check a session or configuration response against the configured context.
///
/// Codex does not take a model in its session parameters: it opens the session
/// with whatever its own configuration selects and offers the rest through
/// config options. So the two checks are different questions, and `mode` is what
/// tells them apart.
///
/// `None` — this profile's selections are not all applied yet. That covers the
/// newly created session, the response to the model request that precedes the
/// mode request, and any `config_option_update` Codex sends while those requests
/// are still going out. The only thing that can be settled is whether the model
/// this binding is configured for is one Codex will accept, which is worth
/// settling here: "Codex does not offer this model" is a configuration mistake,
/// and finding it out at the moment of selection would report it as a refused
/// selection instead. What it deliberately does not check is `currentValue`: the
/// selection that would set it is the one still in flight.
///
/// `Some(mode)` — every selection has been applied, and the response must now
/// read back exactly what was asked for.
pub(super) fn verify_config(
    result: &Value,
    model: &str,
    mode: Option<&str>,
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
    // Establish an unambiguous selection before interpreting either value.
    let model_option = model_option.ok_or_else(|| protocol("missing model config option"))?;
    let Some(mode) = mode else {
        return offers(model_option, model);
    };
    if current(model_option) != Some(model) {
        return Err(protocol(
            "provider did not select the exact configured model",
        ));
    }
    if current(mode_option.ok_or_else(|| protocol("missing mode config option"))?) != Some(mode) {
        return Err(protocol("provider is not in the configured approval mode"));
    }
    Ok(())
}

/// Whether `model` is one this option would accept.
///
/// Its current value counts: a provider that has already opened the session on
/// the configured model offers it whether or not the model appears in a catalog
/// it enumerates, which is how a model served by a custom provider arrives.
fn offers(option: &Value, model: &str) -> Result<(), AgentError> {
    if current(option) == Some(model) {
        return Ok(());
    }
    let offered = option
        .get("options")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol("missing model config choices"))?
        .iter()
        .any(|choice| choice.get("value").and_then(Value::as_str) == Some(model));
    if offered {
        Ok(())
    } else {
        Err(protocol("provider does not offer the configured model"))
    }
}

fn current(option: &Value) -> Option<&str> {
    option.get("currentValue").and_then(Value::as_str)
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/codex_acp/sessions/configuration.rs"]
mod tests;
