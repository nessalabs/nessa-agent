use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::tools::{McpTool, ToolCallUpdate, ToolContent};
use crate::infrastructure::acp::fields::{identifier, string};
use crate::infrastructure::acp::tools::wire::{
    tool_call as acp_tool_call, UNSUPPORTED_TOOL_CONTENT,
};
use crate::infrastructure::json_rpc::protocol;
use crate::infrastructure::mcp::structured_result;
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{self, Write};

/// The most tool calls one execution keeps an identity for. Matches the Claude
/// profile's bound: with 256-byte identifiers and [`MAX_NAME_BYTES`] names, this
/// bounds the map's payload independently of incoming frame size.
const MAX_TOOLS: usize = 4096;

/// The longest provider tool name this profile will retain. Every Codex tool
/// name is far shorter; a longer one is a frame trying to choose how much memory
/// the adapter spends.
const MAX_NAME_BYTES: usize = 128;

/// The most streamed command output one tool call accumulates.
///
/// Codex sends terminal output as deltas, so between frames the transcript
/// exists nowhere but here — and [`MAX_TOOLS`] commands each printing without
/// stopping would otherwise grow this map until the process died. When a command
/// outruns the window the oldest bytes go, because what someone watching a
/// command run is reading is its most recent output, and because the completion
/// carries Codex's own aggregate and replaces this snapshot with it outright.
const MAX_STREAMED_OUTPUT_BYTES: usize = 1024 * 1024;

/// What one observed Codex tool call is known by.
///
/// Codex names some of its tool calls (`exec_command`, `view_image`, an MCP
/// tool) and identifies the rest only by ACP kind. Both are retained, because a
/// permission request arrives carrying neither reliably: a sparse MCP approval
/// carries a tool call identifier, a kind, and a status.
#[derive(Clone)]
pub(in crate::infrastructure::codex_acp) struct ObservedTool {
    /// The provider's own tool name, when it gave one.
    name: Option<String>,
    /// The first ACP kind this tool call was seen with, already validated by the
    /// shared mapper. Later frames may restate it; a kind is display state the
    /// observation carries itself, so a change is not an identity change.
    kind: Option<String>,
    /// The command output streamed for this tool call so far.
    ///
    /// Accumulated rather than forwarded a delta at a time because
    /// [`ToolObservation::with_update`] replaces an observation's content
    /// instead of appending to it: a frame carrying one delta would retain that
    /// delta and lose every delta before it. Bounded by
    /// [`MAX_STREAMED_OUTPUT_BYTES`], and cleared when the completion's own
    /// aggregate arrives to take its place.
    ///
    /// [`ToolObservation::with_update`]: crate::domain::agent_execution::tools::ToolObservation::with_update
    output: String,
    /// Original action input, needed by sparse MCP approval requests.
    input: Option<Box<str>>,
    /// Whether a frame for this call carried Codex's MCP marker. Only the
    /// announcement does; the completion that carries the result does not
    /// (codex-acp 1.12.0 `index.js:25123-25130`), so it is remembered here.
    mcp: bool,
}

/// Execution-scoped provider observations and their retained input accounting.
#[derive(Clone, Default)]
pub(in crate::infrastructure::codex_acp) struct ObservedTools {
    entries: HashMap<String, ObservedTool>,
    input_bytes: usize,
}
impl ObservedTools {
    pub(in crate::infrastructure::codex_acp) fn clear(&mut self) {
        self.entries.clear();
        self.input_bytes = 0;
    }
}

// This bounds the adapter's original-input cache, independently of the
// controller's permission and tool-observation retention budgets.
const MAX_RETAINED_INPUT_BYTES: usize = 1024 * 1024;

struct InputSize {
    remaining: usize,
}
impl Write for InputSize {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(io::Error::other("tool input retention limit exceeded"));
        }
        self.remaining -= bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn same_input(retained: &str, incoming: &Value) -> Result<(), AgentError> {
    let original: Value =
        serde_json::from_str(retained).map_err(|_| protocol("invalid retained tool input"))?;
    if original != *incoming {
        return Err(protocol("tool input changed"));
    }
    Ok(())
}

fn new_input(
    value: &Value,
    retained: Option<&str>,
    used: usize,
) -> Result<Option<Box<str>>, AgentError> {
    let input = match value.get("rawInput") {
        None | Some(Value::Null) => return Ok(None),
        Some(input) if input.is_object() => input,
        Some(_) => return Err(protocol("invalid tool input")),
    };
    if let Some(retained) = retained {
        same_input(retained, input)?;
        return Ok(None);
    }
    // Count encoded bytes before allocating the retained copy. Box<str> keeps
    // capacity equal to its charged length rather than retaining spare capacity.
    let mut size = InputSize {
        remaining: MAX_RETAINED_INPUT_BYTES - used,
    };
    serde_json::to_writer(&mut size, input)
        .map_err(|_| protocol("tool input retention limit exceeded"))?;
    let encoded = serde_json::to_string(input).map_err(|_| protocol("invalid tool input"))?;
    Ok(Some(encoded.into_boxed_str()))
}

/// The last `bytes` bytes of `text`, moved forward to the next character
/// boundary so what comes back is text Codex sent rather than the tail of a
/// character split down the middle.
fn tail(text: &str, bytes: usize) -> &str {
    if text.len() <= bytes {
        return text;
    }
    let mut start = text.len() - bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// `output` with `delta` added, within [`MAX_STREAMED_OUTPUT_BYTES`].
fn accumulate(mut output: String, delta: &str) -> String {
    output.push_str(delta);
    if output.len() > MAX_STREAMED_OUTPUT_BYTES {
        let keep = tail(&output, MAX_STREAMED_OUTPUT_BYTES).len();
        output.drain(..output.len() - keep);
    }
    output
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
/// - **Command output.** Streamed in `_meta` as deltas, and repeated whole in
///   the completion's `rawOutput`. An observation's content is replaced by each
///   update rather than added to, so what a delta frame carries is everything
///   streamed so far and not the delta alone; the completion's aggregate then
///   replaces that in turn. Output is shown once and never dropped.
///
/// The frame is borrowed unchanged when none of that applies.
///
/// `streamed` is what this tool call has accumulated up to now. The second
/// element of the result is what it should accumulate instead, or `None` when
/// this frame carried no output — and it is deliberately returned rather than
/// written, so a frame the shared mapper goes on to reject cannot leave the
/// accumulator holding output no observation ever received.
fn normalize<'a>(
    value: &'a Value,
    streamed: &str,
) -> Result<(Cow<'a, Value>, Option<String>), AgentError> {
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
    let mut accumulated = None;
    if let Some(delta) = streamed_output(value)? {
        let whole = accumulate(streamed.to_owned(), delta);
        content.push(text_content(&whole));
        accumulated = Some(whole);
        rewritten = true;
    } else if let Some(text) = optional_text(value, "rawOutput", "formatted_output")? {
        // Codex's own aggregate, which is the whole of what the command printed
        // and supersedes everything streamed before it. Carried as it stands
        // rather than truncated: an oversized tool payload is the execution
        // controller's to refuse, out loud, and not this adapter's to silently
        // cut down.
        content.push(text_content(text));
        accumulated = Some(String::new());
        rewritten = true;
    }
    if !rewritten {
        return Ok((Cow::Borrowed(value), accumulated));
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
    Ok((Cow::Owned(frame), accumulated))
}

/// Whether Codex marked this frame as an MCP tool call (`_meta.is_mcp_tool_call`).
/// A call is an MCP call when any of its frames was: see [`ObservedTool::mcp`].
fn is_mcp_call(value: &Value) -> bool {
    value.pointer("/_meta/is_mcp_tool_call") == Some(&Value::Bool(true))
}

/// The MCP server and tool an MCP call frame names, from `rawInput`.
///
/// Codex puts them there exactly (`{server, tool, arguments}`); its title
/// joins them with dots, which a server or tool name may also hold, so it is
/// never split. A frame without them, or with names the domain will not keep,
/// leaves the call without an identity rather than refusing it: the call and
/// its result are still shown.
fn mcp_tool(value: &Value, mcp: bool) -> Option<McpTool> {
    if !mcp {
        return None;
    }
    let input = value.get("rawInput")?;
    let server = input.get("server")?.as_str()?;
    let tool = input.get("tool")?.as_str()?;
    McpTool::new(server, tool).ok()
}

/// One MCP content block from a Codex MCP result, as observed content.
///
/// Text is kept as text; a resource link as its URI, which is the whole of
/// what it said; an embedded resource as its text. A block Nessa cannot show —
/// an image, audio, a binary resource, a type it does not know — becomes the
/// shared placeholder, so the result says something was there.
fn mcp_block(block: &Value) -> Result<ToolContent, AgentError> {
    let kind = block
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| protocol("invalid MCP result content type"))?;
    Ok(match kind {
        "text" => ToolContent::text(exact_text(block, "text")?),
        "resource_link" => ToolContent::text(string(block, "uri")?),
        "resource" => match block
            .get("resource")
            .ok_or_else(|| protocol("missing MCP resource"))?
            .get("text")
        {
            Some(Value::String(text)) => ToolContent::text(text.as_str()),
            None | Some(Value::Null) => ToolContent::text(UNSUPPORTED_TOOL_CONTENT),
            Some(_) => return Err(protocol("invalid MCP resource text")),
        },
        _ => ToolContent::text(UNSUPPORTED_TOOL_CONTENT),
    })
}

/// The string at `field`, exactly, empty included: an MCP tool may return
/// empty text, and that is a result, not a malformed frame.
fn exact_text<'a>(value: &'a Value, field: &str) -> Result<&'a str, AgentError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| protocol("invalid MCP result text"))
}

/// What a completed Codex MCP call returned, as observed content: its content
/// blocks, then its structured result, or its error.
///
/// Codex sends an MCP result nowhere but `rawOutput` —
/// `{result: {content, structuredContent, _meta} | null, error: {message} |
/// null}` — and no ACP content at all, so without this a finished MCP call
/// showed nothing. `None` for a frame that is not an MCP call or carries no
/// output yet, which leaves the observed content as it was. The result's
/// `_meta` is not kept: nothing reads it yet.
fn mcp_result(value: &Value, mcp: bool) -> Result<Option<Vec<ToolContent>>, AgentError> {
    if !mcp {
        return Ok(None);
    }
    let output = match value.get("rawOutput") {
        None | Some(Value::Null) => return Ok(None),
        Some(output) if output.is_object() => output,
        Some(_) => return Err(protocol("invalid MCP tool output")),
    };
    let mut content = Vec::new();
    match output.get("result") {
        None | Some(Value::Null) => {}
        Some(result) if result.is_object() => {
            match result.get("content") {
                None | Some(Value::Null) => {}
                Some(Value::Array(blocks)) => {
                    for block in blocks {
                        content.push(mcp_block(block)?);
                    }
                }
                Some(_) => return Err(protocol("invalid MCP result content")),
            }
            match result.get("structuredContent") {
                None | Some(Value::Null) => {}
                // Past the domain's bound it is said, not kept.
                Some(structured) => content.push(
                    structured_result(structured).map_err(|error| protocol(&error.to_string()))?,
                ),
            }
        }
        Some(_) => return Err(protocol("invalid MCP result")),
    }
    match output.get("error") {
        None | Some(Value::Null) => {}
        Some(error) => content.push(ToolContent::text(exact_text(error, "message")?)),
    }
    Ok(Some(content))
}

pub(in crate::infrastructure::codex_acp) fn tool_call(
    value: &Value,
    tools: &mut ObservedTools,
) -> Result<ToolCallUpdate, AgentError> {
    let id = identifier(value, "toolCallId")?.to_owned();
    if !tools.entries.contains_key(&id) && tools.entries.len() >= MAX_TOOLS {
        return Err(protocol("tool count limit exceeded"));
    }
    let retained = tools
        .entries
        .get(&id)
        .and_then(|tool| tool.input.as_deref());
    let input = new_input(value, retained, tools.input_bytes)?;
    let added_bytes = input.as_ref().map_or(0, |input| input.len());
    let streamed = tools
        .entries
        .get(&id)
        .map_or("", |tool| tool.output.as_str());
    let (frame, accumulated) = normalize(value, streamed)?;
    // Validate the complete representation before retaining provider identity.
    let mut update = acp_tool_call(frame.as_ref())?;
    let mcp = is_mcp_call(value) || tools.entries.get(&id).is_some_and(|tool| tool.mcp);
    if let Some(result) = mcp_result(value, mcp)? {
        // After whatever ACP content the frame carried, which Codex leaves empty
        // for an MCP call today.
        let mut content = update.content().clone().unwrap_or_default();
        content.extend(result);
        update = update.with_content(content);
    }
    if let Some(tool) = mcp_tool(value, mcp) {
        update = update.with_mcp_tool(tool);
    }
    let name = declared_name(value)?;
    let kind = value.get("kind").and_then(Value::as_str);
    match tools.entries.get_mut(&id) {
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
            tool.mcp = mcp;
            if let Some(input) = input {
                tool.input = Some(input);
            }
            if let Some(accumulated) = accumulated {
                tool.output = accumulated;
            }
        }
        None => {
            tools.entries.insert(
                id,
                ObservedTool {
                    name: name.map(str::to_owned),
                    kind: kind.map(str::to_owned),
                    output: accumulated.unwrap_or_default(),
                    input,
                    mcp,
                },
            );
        }
    }
    tools.input_bytes += added_bytes;
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
    tools: &ObservedTools,
) -> Result<ToolReviewInput, AgentError> {
    let tool = request
        .get("toolCall")
        .ok_or_else(|| protocol("missing permission tool"))?;
    let id = identifier(tool, "toolCallId")?;
    let name = review_name(tool, tools.entries.get(id))?;
    // File-edit approvals in the pinned adapter carry locations on toolCall,
    // not rawInput. Preserve that request as evidence; the execution's existing
    // observation retains the previously streamed diff for the same tool ID.
    let retained = tools.entries.get(id).and_then(|tool| tool.input.as_deref());
    let arguments_json = match tool.get("rawInput") {
        Some(arguments) if arguments.is_object() => {
            if let Some(retained) = retained {
                same_input(retained, arguments)?;
            }
            arguments.to_string()
        }
        None | Some(Value::Null) => {
            if let Some(retained) = retained {
                retained.to_owned()
            } else if tool.get("kind").and_then(Value::as_str) == Some("edit") {
                let update = acp_tool_call(tool)?;
                if update.locations().as_ref().is_none_or(Vec::is_empty) {
                    return Err(protocol("file approval carries no locations"));
                }
                json!({"toolCall":tool,"metadata":request.get("_meta")}).to_string()
            } else {
                return Err(protocol("permission request carries no reviewable input"));
            }
        }
        _ => return Err(protocol("invalid tool input")),
    };
    Ok(ToolReviewInput {
        name,
        arguments_json,
    })
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/codex_acp/tools/wire.rs"]
mod tests;
