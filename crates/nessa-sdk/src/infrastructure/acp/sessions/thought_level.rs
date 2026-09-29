//! The agent's reasoning effort option, as ACP advertises it.
//!
//! ACP names the kind of a session config option by its `category`, and
//! `thought_level` is the one for reasoning effort. Agents call the option
//! itself what they like — `effort` for Claude's adapter, `reasoning_effort`
//! for Codex's — so it is found here by that category and the id the profile
//! sends a level to. Options under any other id are not this runtime's, and are
//! not read.
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

/// One agent's `thought_level` option: its current value, and the values it
/// will take.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ThoughtLevel<'a> {
    pub current: Option<&'a str>,
    pub choices: Vec<&'a str>,
}

/// The `thought_level` option under `id` in a session or configuration
/// response, or `None` where the agent advertises none there (Claude's adapter
/// on Haiku, say).
///
/// # Errors
///
/// A protocol error when the response has no config options, lists that option
/// twice, or lists it with choices that are not a bounded list of strings — a
/// flat list, or ACP's groups of them.
pub(crate) fn thought_level<'a>(
    result: &'a Value,
    id: &'a str,
) -> Result<Option<ThoughtLevel<'a>>, AgentError> {
    let options = result
        .get("configOptions")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol("missing session config options"))?;
    let mut found = None;
    for option in options {
        if option.get("category").and_then(Value::as_str) != Some(CATEGORY)
            || option.get("id").and_then(Value::as_str) != Some(id)
        {
            continue;
        }
        if found.replace(option).is_some() {
            return Err(protocol("duplicate thought_level config option"));
        }
    }
    let Some(option) = found else {
        return Ok(None);
    };
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
    Ok(thought_level(result, id)?
        .map(|option| levels.offered_by(option.choices))
        .unwrap_or_default())
}

/// Check that the agent's option `id` reads back `selected`, once
/// `configured` — before that, the request that sets it may still be in
/// flight — and a level has been selected; with none selected the agent keeps
/// its own default and nothing is claimed.
///
/// # Errors
///
/// As [`thought_level`], and a protocol error when a level was selected but
/// the response has no such option or reports another current value.
pub(crate) fn verify_selected(
    result: &Value,
    id: &str,
    selected: Option<&EffortLevel>,
    configured: bool,
) -> Result<(), AgentError> {
    let (Some(selected), true) = (selected, configured) else {
        return Ok(());
    };
    match thought_level(result, id)? {
        Some(option) if option.current == Some(selected.as_str()) => Ok(()),
        _ => Err(protocol("provider effort level did not match selection")),
    }
}

/// Whether a binding may select `level` for every session it opens: one of
/// the model's catalogue levels its binding offers
/// ([`EffectiveCapabilities::effort_levels`]).
///
/// # Errors
///
/// [`AgentError::Unsupported`] for any other level.
pub(crate) fn selectable(
    capabilities: &EffectiveCapabilities,
    level: &EffortLevel,
) -> Result<(), AgentError> {
    if capabilities
        .effort_levels()
        .is_some_and(|levels| levels.contains(level))
    {
        Ok(())
    } else {
        Err(AgentError::Unsupported(
            "effort level is unavailable for this model".into(),
        ))
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
