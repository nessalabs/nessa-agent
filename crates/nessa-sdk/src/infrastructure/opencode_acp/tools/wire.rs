use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::tools::ToolCallUpdate;
use crate::infrastructure::acp::fields::identifier;
use crate::infrastructure::acp::tools::wire::tool_call as acp_tool_call;
use crate::infrastructure::json_rpc::protocol;
use serde_json::Value;
use std::collections::HashMap;

/// The most tool calls one execution keeps an identity for.
///
/// The same bound the other profiles use. With 256-byte identifiers and
/// [`MAX_NAME_BYTES`] names it bounds this map's payload independently of how
/// large the frames arriving are, so a provider that keeps announcing tool calls
/// cannot decide how much memory the adapter spends.
const MAX_TOOLS: usize = 4096;

/// The longest provider tool name this profile will retain. Every Opencode tool
/// name is far shorter; a longer one is a frame choosing how much memory to use.
const MAX_NAME_BYTES: usize = 128;

/// What one observed Opencode tool call is known by.
///
/// Only the title, because that is the one thing Opencode gives a tool call that
/// a person reading a permission request would recognise. Retained rather than
/// read from the request, because a permission request can arrive naming nothing
/// but the tool call it belongs to.
#[derive(Clone)]
pub(in crate::infrastructure::opencode_acp) struct ObservedTool {
    title: Option<String>,
}

/// Read a tool call frame, remembering what it was.
///
/// The reading itself is the shared one: Opencode reports tool calls in the
/// protocol's own shape, with no extension of its own to interpret. What is
/// added here is the retention, which is what makes a later permission request
/// answerable.
pub(in crate::infrastructure::opencode_acp) fn tool_call(
    value: &Value,
    tools: &mut HashMap<String, ObservedTool>,
) -> Result<ToolCallUpdate, AgentError> {
    let update = acp_tool_call(value)?;
    let id = identifier(value, "toolCallId")?;
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| title.len() <= MAX_NAME_BYTES)
        .map(str::to_owned);
    // A tool call is announced once and then updated, and an update carries only
    // what changed. A title that is absent now is one the announcement already
    // gave, so it is kept rather than forgotten.
    if let Some(observed) = tools.get_mut(id) {
        if title.is_some() {
            observed.title = title;
        }
        return Ok(update);
    }
    // Bounded, and by refusing rather than by evicting: a review that named the
    // wrong tool call would be worse than one saying it cannot name it, and this
    // many tool calls in one execution is not a real one.
    if tools.len() >= MAX_TOOLS {
        return Err(protocol("too many tool calls in one execution"));
    }
    tools.insert(id.to_owned(), ObservedTool { title });
    Ok(update)
}

/// What a permission request is asking to be allowed.
///
/// Opencode asks with the tool call itself, so its arguments are read from
/// `rawInput`. A request carrying no arguments is refused rather than reviewed
/// on its title alone: a decision recorded against "Edit" with nothing saying
/// what would be edited is not a reviewed decision, and the audit would carry it
/// as though it were.
pub(in crate::infrastructure::opencode_acp) fn permission_input(
    request: &Value,
    tools: &HashMap<String, ObservedTool>,
) -> Result<ToolReviewInput, AgentError> {
    let tool = request
        .get("toolCall")
        .ok_or_else(|| protocol("missing permission tool"))?;
    let id = identifier(tool, "toolCallId")?;
    let arguments = tool
        .get("rawInput")
        .filter(|arguments| arguments.is_object())
        .ok_or_else(|| protocol("permission request carries no reviewable input"))?;
    // The request's own title first: a request that says what it is asking about
    // is the better answer. The retained one is for the request that does not,
    // and the identifier is the last resort — never a name invented here, which
    // would put a tool Opencode never ran into the audit record.
    let name = tool
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| title.len() <= MAX_NAME_BYTES)
        .map(str::to_owned)
        .or_else(|| tools.get(id).and_then(|observed| observed.title.clone()))
        .unwrap_or_else(|| id.to_owned());
    Ok(ToolReviewInput {
        name,
        arguments_json: arguments.to_string(),
    })
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/opencode_acp/tools/wire.rs"]
mod tests;
