use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::tools::ToolCallUpdate;
use crate::infrastructure::acp::fields::{identifier, string};
use crate::infrastructure::acp::tools::wire::tool_call as acp_tool_call;
use crate::infrastructure::json_rpc::protocol;
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::HashMap;

/// The most tool calls one execution keeps an identity for. Matches the Claude
/// profile's bound: with 256-byte identifiers and [`MAX_NAME_BYTES`] names, this
/// bounds the map's payload independently of incoming frame size.
const MAX_TOOLS: usize = 4096;

/// The longest provider tool name this profile will retain. Every Codex tool
/// name is far shorter; a longer one is a frame trying to choose how much memory
/// the adapter spends.
const MAX_NAME_BYTES: usize = 128;

/// What one observed Codex tool call is known by.
///
/// Codex names some of its tool calls (`exec_command`, `view_image`, an MCP
/// tool) and identifies the rest only by ACP kind. Both are retained, because a
/// permission request arrives carrying neither reliably: a file-change approval
/// is asked with nothing but a tool call identifier, a kind, and a status.
#[derive(Clone)]
pub(in crate::infrastructure::codex_acp) struct ObservedTool {
    /// The provider's own tool name, when it gave one.
    name: Option<String>,
    /// The first ACP kind this tool call was seen with, already validated by the
    /// shared mapper. Later frames may restate it; a kind is display state the
    /// observation carries itself, so a change is not an identity change.
    kind: Option<String>,
    /// Whether this tool call's output has already been carried as content.
    /// Codex streams command output in `_meta` and then repeats the whole of it
    /// in the completion's `rawOutput`; carrying both would duplicate it.
    output_carried: bool,
}

/// Whether a provider name is small enough and plain enough to keep.
///
/// Graphic ASCII rather than an allowlist of characters: Codex tool names are
/// its own (`exec_command`), its MCP namespacing (`mcp.nessa.shell`), and names
/// the model's tool registry supplies. What matters here is that a name is a
/// name — bounded, printable, and on one line — not that it matches a shape this
/// repository invented.
fn bounded_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME_BYTES && name.bytes().all(|b| b.is_ascii_graphic())
}

/// The provider tool name on this frame, when it declared one.
fn declared_name(value: &Value) -> Result<Option<&str>, AgentError> {
    match value.get("name") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(name)) if bounded_name(name) => Ok(Some(name)),
        // A rejected name can occupy the entire frame. Do not copy it into an
        // error that teardown will retain and clone.
        Some(Value::String(_)) => Err(AgentError::Unsupported(
            "tool name is not a reviewable identity".into(),
        )),
        Some(_) => Err(protocol("invalid tool name")),
    }
}

/// A non-empty string at `field` inside `parent`, or nothing when the field is
/// absent. A present field of the wrong type is a protocol error rather than a
/// silent absence: dropping output because it arrived in an unexpected shape is
/// exactly the evidence loss this adapter exists to prevent.
fn optional_text<'a>(
    value: &'a Value,
    parent: &str,
    field: &str,
) -> Result<Option<&'a str>, AgentError> {
    let Some(parent) = value.get(parent) else {
        return Ok(None);
    };
    match parent.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if text.is_empty() => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(protocol("invalid tool output")),
    }
}

/// Output Codex streamed for this tool call, in whichever of the two shapes the
/// negotiated terminal output mode uses.
fn streamed_output(value: &Value) -> Result<Option<&str>, AgentError> {
    let Some(meta) = value.get("_meta") else {
        return Ok(None);
    };
    for source in ["terminal_output", "terminal_output_delta"] {
        if let Some(text) = optional_text(meta, source, "data")? {
            return Ok(Some(text));
        }
    }
    Ok(None)
}

/// A text content entry carrying `text`, in the shape the shared mapper reads.
fn text_content(text: &str) -> Value {
    json!({"type":"content","content":{"type":"text","text":text}})
}

/// Rewrite one Codex tool frame into the content vocabulary the shared mapper
/// reads, without losing what it carried.
///
/// Three things are Codex's own and have no shared shape:
///
/// - **Terminal content.** Codex points at a terminal for every shell command it
///   runs. Nessa declares no terminal capability, so that identifier addresses
///   nothing it can read. The output itself arrives separately, and is carried
///   here as text rather than left behind with the pointer.
/// - **Resource links.** Carried as their URI, which is the whole of what the
///   link said.
/// - **Command output.** Streamed in `_meta` and repeated whole in the
///   completion's `rawOutput`. The stream is carried as it arrives; the repeat is
///   carried only for a tool call that never streamed, so output is never shown
///   twice and never dropped.
///
/// The frame is borrowed unchanged when none of that applies.
fn normalize<'a>(
    value: &'a Value,
    output_carried: bool,
) -> Result<(Cow<'a, Value>, bool), AgentError> {
    let existing = match value.get("content") {
        None | Some(Value::Null) => &[][..],
        Some(content) => content
            .as_array()
            .ok_or_else(|| protocol("invalid tool content"))?,
    };
    let mut content = Vec::with_capacity(existing.len());
    let mut rewritten = false;
    for item in existing {
        match item.get("type").and_then(Value::as_str) {
            Some("terminal") => {
                // Validated even though it is dropped: a malformed frame is a
                // malformed frame whether or not this adapter can use the field.
                identifier(item, "terminalId")?;
                rewritten = true;
            }
            Some("diff") => content.push(item.clone()),
            Some("content") => {
                let block = item
                    .get("content")
                    .ok_or_else(|| protocol("missing tool content block"))?;
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => content.push(item.clone()),
                    Some("resource_link") => {
                        content.push(text_content(string(block, "uri")?));
                        rewritten = true;
                    }
                    _ => return Err(protocol("unsupported nested tool content type")),
                }
            }
            _ => return Err(protocol("unsupported tool content type")),
        }
    }
    let mut carried = output_carried;
    if let Some(text) = streamed_output(value)? {
        content.push(text_content(text));
        carried = true;
        rewritten = true;
    } else if !carried {
        if let Some(text) = optional_text(value, "rawOutput", "formatted_output")? {
            content.push(text_content(text));
            carried = true;
            rewritten = true;
        }
    }
    if !rewritten {
        return Ok((Cow::Borrowed(value), carried));
    }
    let mut frame = value.clone();
    let object = frame
        .as_object_mut()
        .ok_or_else(|| protocol("invalid tool call"))?;
    if content.is_empty() {
        // Every entry this frame carried was a pointer this adapter cannot
        // follow. Absent content leaves the observation's content unchanged;
        // an empty list would read as content deliberately cleared.
        object.remove("content");
    } else {
        object.insert("content".into(), Value::Array(content));
    }
    Ok((Cow::Owned(frame), carried))
}

pub(in crate::infrastructure::codex_acp) fn tool_call(
    value: &Value,
    tools: &mut HashMap<String, ObservedTool>,
) -> Result<ToolCallUpdate, AgentError> {
    let id = identifier(value, "toolCallId")?.to_owned();
    let carried = tools.get(&id).is_some_and(|tool| tool.output_carried);
    let (frame, carried) = normalize(value, carried)?;
    // Validate the complete representation before retaining provider identity.
    let update = acp_tool_call(frame.as_ref())?;
    let name = declared_name(value)?;
    let kind = value.get("kind").and_then(Value::as_str);
    match tools.get_mut(&id) {
        Some(tool) => {
            match (&tool.name, name) {
                (Some(retained), Some(name)) if retained != name => {
                    return Err(protocol("tool identity changed"))
                }
                (None, Some(name)) => tool.name = Some(name.to_owned()),
                _ => {}
            }
            if tool.kind.is_none() {
                tool.kind = kind.map(str::to_owned);
            }
            tool.output_carried = carried;
        }
        None => {
            if tools.len() >= MAX_TOOLS {
                return Err(protocol("tool count limit exceeded"));
            }
            tools.insert(
                id,
                ObservedTool {
                    name: name.map(str::to_owned),
                    kind: kind.map(str::to_owned),
                    output_carried: carried,
                },
            );
        }
    }
    Ok(update)
}

/// What the host is being asked to authorize, named as precisely as Codex named
/// it: its own tool name when it gave one, and the kind of action otherwise.
fn review_name(tool: &Value, observed: Option<&ObservedTool>) -> Result<String, AgentError> {
    if let Some(name) = observed.and_then(|tool| tool.name.as_deref()) {
        return Ok(name.to_owned());
    }
    if let Some(name) = declared_name(tool)? {
        return Ok(name.to_owned());
    }
    match tool.get("kind") {
        Some(Value::String(kind)) if bounded_name(kind) => Ok(kind.clone()),
        None | Some(Value::Null) => observed
            .and_then(|tool| tool.kind.clone())
            .ok_or_else(|| protocol("permission request names no tool")),
        Some(_) => Err(protocol("invalid tool kind")),
    }
}

pub(in crate::infrastructure::codex_acp) fn permission_input(
    request: &Value,
    tools: &HashMap<String, ObservedTool>,
) -> Result<ToolReviewInput, AgentError> {
    let tool = request
        .get("toolCall")
        .ok_or_else(|| protocol("missing permission tool"))?;
    let id = identifier(tool, "toolCallId")?;
    let name = review_name(tool, tools.get(id))?;
    // `rawInput` is the action's own arguments and is preferred. Codex asks some
    // approvals — a file change is the common one — with no arguments on the
    // tool call at all, and puts what it is asking about in the request's own
    // metadata instead. Reviewing the second is not as good as reviewing the
    // first, but it is the provider's complete account of the action, and a
    // review with nothing in it would be worse than either.
    let arguments = match tool.get("rawInput") {
        Some(arguments) if arguments.is_object() => arguments,
        _ => request
            .pointer("/_meta/codex")
            .filter(|meta| meta.is_object())
            .ok_or_else(|| protocol("permission request carries no reviewable input"))?,
    };
    Ok(ToolReviewInput {
        name,
        arguments_json: arguments.to_string(),
    })
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/codex_acp/tools/wire.rs"]
mod tests;
