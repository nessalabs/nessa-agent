//! The agent's reasoning effort option, as ACP advertises it.
//!
//! ACP names the kind of a session config option by its `category`, and
//! `thought_level` is the one for reasoning effort. Agents call the option
//! itself what they like — `effort` for Claude's adapter, `reasoning_effort`
//! for Codex's — so it is found here by category, and the profile that sends
//! a level names the id it sends to.
use crate::application::agent_execution::agents::AgentError;
use crate::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use crate::domain::model_metadata::value_objects::{EffortLevel, OfferedEffortLevels};
use crate::infrastructure::json_rpc::protocol;
use serde_json::Value;

/// The ACP config option category for reasoning effort.
const CATEGORY: &str = "thought_level";

/// Most choices read from one option: several times what any agent offers, so
/// an agent's answer cannot make this runtime hold or compare an unbounded list.
const MAX_CHOICES: usize = 64;

/// One agent's `thought_level` option: its id, its current value, and the
/// values it will take.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ThoughtLevel<'a> {
    pub id: &'a str,
    pub current: Option<&'a str>,
    pub choices: Vec<&'a str>,
}

/// The single `thought_level` option in a session or configuration response,
/// or `None` where the agent advertises none (Claude's adapter on Haiku, say).
///
/// # Errors
///
/// A protocol error when the response has no config options, lists two
/// `thought_level` options, or lists one without an id or with choices that are
/// not a bounded list of strings — a flat list, or ACP's groups of them.
pub(crate) fn thought_level(result: &Value) -> Result<Option<ThoughtLevel<'_>>, AgentError> {
    let options = result
        .get("configOptions")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol("missing session config options"))?;
    let mut found = None;
    for option in options {
        if option.get("category").and_then(Value::as_str) != Some(CATEGORY) {
            continue;
        }
        if found.replace(option).is_some() {
            return Err(protocol("duplicate thought_level config option"));
        }
    }
    let Some(option) = found else {
        return Ok(None);
    };
    let id = option
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| protocol("thought_level config option without an id"))?;
    let listed = option
        .get("options")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol("thought_level config option without choices"))?;
    let mut choices = Vec::new();
    for entry in listed {
        // A group holds choices of its own; a choice holds a value.
        let values = match entry.get("options").and_then(Value::as_array) {
            Some(grouped) => grouped.iter().map(|choice| choice.get("value")).collect(),
            None => vec![entry.get("value")],
        };
        for value in values {
            let value = value
                .and_then(Value::as_str)
                .ok_or_else(|| protocol("thought_level choice without a string value"))?;
            if choices.len() == MAX_CHOICES {
                return Err(protocol(
                    "thought_level config option lists too many choices",
                ));
            }
            choices.push(value);
        }
    }
    Ok(Some(ThoughtLevel {
        id,
        current: option.get("currentValue").and_then(Value::as_str),
        choices,
    }))
}

/// Which of the model's catalogue levels the agent's `thought_level` option in
/// `result` also offers, where that option has the `id` a level is sent to:
/// none where the profile sends no level (`id` is `None`), the catalogue
/// records no levels for the model (or the binding offers no reasoning), the
/// agent advertises no option, or advertises it under another id.
///
/// # Errors
///
/// As [`thought_level`], and only where there are catalogue levels to narrow:
/// otherwise `result` is not read.
pub(crate) fn offered(
    result: &Value,
    capabilities: &EffectiveCapabilities,
    id: Option<&str>,
) -> Result<OfferedEffortLevels, AgentError> {
    // Nothing to narrow, or nothing that could be sent: the response is not read.
    let (Some(levels), Some(id)) = (capabilities.effort_levels(), id) else {
        return Ok(OfferedEffortLevels::default());
    };
    Ok(thought_level(result)?
        .filter(|option| option.id == id)
        .map(|option| levels.offered_by(option.choices))
        .unwrap_or_default())
}

/// Check that the agent's option `id` reads back `selected`, once a level has
/// been selected; with none selected the agent keeps its own default and
/// nothing is claimed.
///
/// # Errors
///
/// As [`thought_level`], and a protocol error when a level was selected but
/// the response has no `thought_level` option, has one under another id, or
/// reports another current value.
pub(crate) fn verify_selected(
    result: &Value,
    id: &str,
    selected: Option<&EffortLevel>,
) -> Result<(), AgentError> {
    let Some(selected) = selected else {
        return Ok(());
    };
    match thought_level(result)? {
        Some(option) if option.id == id && option.current == Some(selected.as_str()) => Ok(()),
        _ => Err(protocol("provider effort level did not match selection")),
    }
}

/// The `session/set_config_option` request that selects `level` through the
/// agent's option `id`.
pub(crate) fn selection(session_id: &str, id: &str, level: &EffortLevel) -> Value {
    serde_json::json!({"sessionId":session_id,"configId":id,"value":level.as_str()})
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/acp/sessions/thought_level.rs"]
mod tests;
