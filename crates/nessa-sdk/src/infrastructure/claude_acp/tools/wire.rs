use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::tools::{McpTool, ToolCallUpdate};
use crate::infrastructure::acp::fields::identifier;
use crate::infrastructure::acp::tools::wire::{path, tool_call as acp_tool_call};
use crate::infrastructure::json_rpc::protocol;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
/// What the pinned harness offers that Nessa must not let an agent reach.
///
/// Admission is otherwise open — see [`enabled_name`] — so this list is the
/// whole of that boundary, and it is read against one pinned harness version,
/// which startup verifies. Reviewing a tool is not the same as owning what it
/// does: an approval says "yes, do this", and these do it somewhere Nessa
/// cannot see, account for, or stop.
///
/// Grouped by what would escape, not by name:
///
/// 1. Execution outside Shepherd. `Monitor` takes a shell command or a
///    WebSocket and streams it back, which is `Bash` by another door.
/// 2. Permission-mode changes, which would rewrite the policy the rest of
///    this profile depends on.
/// 3. Work that outlives the execution that asked for it: scheduled prompts
///    that survive a restart, workflow scripts that spawn their own agents,
///    and worktree switches that move the checkout underneath one.
/// 4. Effects on services beyond this machine, which belong to the user's
///    account rather than to a local conversation.
///
/// These names go to the harness as both `disallowedTools` and permission
/// `deny`. The pinned Claude SDK canonicalizes historical names before matching:
/// `KillShell` becomes `TaskStop` and `BashOutput` becomes `TaskOutput`. This
/// list uses the canonical names so its text and its effective boundary agree.
/// A name added here must be checked against that normalization: this list and
/// the harness must deny the same set for admission to be open safely.
pub(in crate::infrastructure::claude_acp) const DISALLOWED_TOOLS: &[&str] = &[
    // Execution Nessa does not own.
    "Bash",
    "TaskOutput",
    "TaskStop",
    "Monitor",
    "REPL",
    // Permission mode.
    "EnterPlanMode",
    "ExitPlanMode",
    // Work that outlives or escapes this execution.
    "Workflow",
    "CronCreate",
    "CronDelete",
    "CronList",
    "ScheduleWakeup",
    "EnterWorktree",
    "ExitWorktree",
    // Effects beyond this machine.
    "Artifact",
    "PushNotification",
    "RemoteTrigger",
    "SendFeedback",
];
/// The namespace the harness gives every tool of a configured MCP server.
pub(in crate::infrastructure::claude_acp) const MCP_NAMESPACE: &str = "mcp__";
fn bounded_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
/// Whether Nessa's permission owner may review a call to this tool.
///
/// The harness's own built-in tools are all reviewable: the model reaches them
/// through deferred schema loading, and which ones exist is the pinned
/// harness's fact, not a list Nessa can keep current. Enumerating them refused
/// executions outright — a whole turn died on the first tool Nessa had not
/// heard of — so admission asks the two questions Nessa does own instead.
/// Tools Nessa denies outright stay denied, and an MCP name must belong to a
/// configured server rather than merely look like one.
fn enabled_name(name: &str, mcp_prefixes: &[String]) -> bool {
    if !bounded_name(name) || DISALLOWED_TOOLS.contains(&name) {
        return false;
    }
    if name.starts_with(MCP_NAMESPACE) {
        return mcp_prefixes
            .iter()
            .any(|prefix| names_tool_of(name, prefix));
    }
    true
}
/// The configured MCP server and tool a harness tool name names, or `None`.
///
/// The harness names an MCP tool `mcp__<server>__<tool>`. A configured server
/// name cannot hold `__` (`AcpConfig::validate`) but may end in `_`, so the name
/// alone does not always say where the server ends. The
/// configured servers do: a name is split at the one configured prefix it
/// starts with. Where two configured prefixes both fit (`a` and `a_` for
/// `mcp__a___c`), the call is left without an identity rather than given a
/// guessed one — its tool row and text result are unchanged. The tool part is
/// the harness's spelling, in which every character outside `[A-Za-z0-9_-]`
/// has become `_`.
fn mcp_tool(name: &str, mcp_prefixes: &[String]) -> Option<McpTool> {
    let mut fitting = mcp_prefixes
        .iter()
        .filter(|prefix| names_tool_of(name, prefix));
    let prefix = fitting.next()?;
    if fitting.next().is_some() {
        return None;
    }
    let server = prefix
        .strip_prefix(MCP_NAMESPACE)?
        .strip_suffix(MCP_SEPARATOR)?;
    McpTool::new(server, &name[prefix.len()..]).ok()
}
/// Whether `name` names a tool of the server whose harness prefix is `prefix`
/// (`mcp__<server>__`): it starts with the prefix and names something after it.
/// Admission and the MCP identity both ask this, so they cannot disagree.
fn names_tool_of(name: &str, prefix: &str) -> bool {
    name.starts_with(prefix) && name.len() > prefix.len()
}
/// What separates the server from the tool in a harness MCP tool name.
pub(in crate::infrastructure::claude_acp) const MCP_SEPARATOR: &str = "__";
/// What this binding knows about one tool call it has observed.
///
/// A call it will not put to a host is still a call the agent made, and the
/// person watching should see it happen. Refusing the frame outright hid the
/// tool *and* ended the turn; keeping the observation lets the tool row appear
/// and leaves the refusal where it can be answered — the permission request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::infrastructure::claude_acp) enum ObservedTool {
    /// A name this binding will put to a host, retained for the review.
    Reviewable(String),
    /// A name it will not. The name is deliberately not retained: an unbounded
    /// one would be retained here for the rest of the execution, and the frame
    /// that asks for the review carries the name again for the record.
    Declined,
}

pub(in crate::infrastructure::claude_acp) fn tool_call(
    value: &Value,
    names: &mut HashMap<String, ObservedTool>,
    mcp_prefixes: &[String],
) -> Result<ToolCallUpdate, AgentError> {
    // Validate the complete representation before retaining provider name state.
    let mut update = acp_tool_call(value)?;
    let id = identifier(value, "toolCallId")?.to_owned();
    if let Some(name) = value
        .pointer("/_meta/claudeCode/toolName")
        .and_then(Value::as_str)
    {
        // Names are bounded before retention; together with 256-byte IDs and
        // 4,096 entries, this bounds the map's string payload independently of
        // incoming frame size. Which names are admitted at all is
        // `enabled_name`'s account, not a second one here.
        let observed = if enabled_name(name, mcp_prefixes) {
            if let Some(tool) = mcp_tool(name, mcp_prefixes) {
                update = update.with_mcp_tool(tool);
            }
            ObservedTool::Reviewable(name.to_owned())
        } else {
            ObservedTool::Declined
        };
        if names.get(&id).is_some_and(|old| *old != observed) {
            return Err(protocol("tool identity changed"));
        }
        if names.len() >= 4096 && !names.contains_key(&id) {
            // A reviewable call that cannot be retained cannot be reviewed, so
            // that frame is refused. A declined one has nothing to retain: it
            // is going to be refused anyway, and forgetting it costs only the
            // precision of the refusal's reason. Refusing the frame instead
            // would let a provider end an execution purely by calling denied
            // tools often enough — the failure this path exists to remove.
            if observed == ObservedTool::Declined {
                return Ok(update);
            }
            return Err(protocol("tool count limit exceeded"));
        }
        names.insert(id.clone(), observed);
    }
    Ok(update)
}
#[allow(dead_code)] // These structs validate the pinned harness schemas; preserve original arguments below.
pub(in crate::infrastructure::claude_acp) fn tool_input(
    name: &str,
    value: &Value,
    mcp_prefixes: &[String],
) -> Result<ToolReviewInput, AgentError> {
    #[derive(Deserialize)]
    struct Read {
        file_path: String,
        offset: Option<u64>,
        limit: Option<u64>,
        pages: Option<String>,
    }
    #[derive(Deserialize)]
    struct Write {
        file_path: String,
        content: String,
    }
    #[derive(Deserialize)]
    struct Edit {
        file_path: String,
        old_string: String,
        new_string: String,
        #[serde(default)]
        replace_all: bool,
    }
    #[derive(Deserialize)]
    struct Glob {
        pattern: String,
        path: Option<String>,
    }
    #[derive(Deserialize)]
    struct Grep {
        pattern: String,
        path: Option<String>,
        glob: Option<String>,
        #[serde(rename = "type")]
        file_type: Option<String>,
        output_mode: Option<String>,
        #[serde(rename = "-B")]
        before: Option<u64>,
        #[serde(rename = "-A")]
        after: Option<u64>,
        #[serde(rename = "-C")]
        short_context: Option<u64>,
        context: Option<u64>,
        #[serde(rename = "-n")]
        line_numbers: Option<bool>,
        #[serde(rename = "-i")]
        ignore_case: Option<bool>,
        #[serde(rename = "-o")]
        only_matching: Option<bool>,
        multiline: Option<bool>,
        head_limit: Option<u64>,
        offset: Option<u64>,
    }
    fn parse<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, AgentError> {
        serde_json::from_value(value.clone()).map_err(|_| protocol("unsupported file-tool input"))
    }
    match name {
        "Read" => {
            let v: Read = parse(value)?;
            path(v.file_path)?;
        }
        "Write" => {
            let v: Write = parse(value)?;
            path(v.file_path)?;
        }
        "Edit" => {
            let v: Edit = parse(value)?;
            path(v.file_path)?;
        }
        "Glob" => {
            let v: Glob = parse(value)?;
            v.path.map(path).transpose()?;
        }
        "Grep" => {
            let v: Grep = parse(value)?;
            v.path.map(path).transpose()?;
            if matches!((v.context, v.short_context), (Some(a), Some(b)) if a != b) {
                return Err(protocol("conflicting search context fields"));
            }
        }
        _ if enabled_name(name, mcp_prefixes) && value.is_object() => {}
        _ => return Err(protocol("permission for an invalid or disabled tool")),
    }
    Ok(ToolReviewInput {
        name: name.into(),
        arguments_json: value.to_string(),
    })
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/claude_acp/tools/wire.rs"]
mod tests;
