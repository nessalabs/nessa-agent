use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::tools::ToolCallUpdate;
use crate::infrastructure::acp::fields::identifier;
use crate::infrastructure::acp::tools::wire::{path, tool_call as acp_tool_call};
use crate::infrastructure::json_rpc::protocol;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
// Native shell and mode changes must not bypass Nessa's execution and permission owners.
pub(in crate::infrastructure::claude_acp) const DISALLOWED_TOOLS: &[&str] = &[
    "Bash",
    "BashOutput",
    "KillShell",
    "EnterPlanMode",
    "ExitPlanMode",
];
pub(in crate::infrastructure::claude_acp) const REVIEW_TOOLS: &[&str] = &[
    "Read",
    "Write",
    "Edit",
    "Glob",
    "Grep",
    "NotebookEdit",
    "WebSearch",
    "WebFetch",
    "Agent",
    "Task",
    "TodoWrite",
    "TaskCreate",
    "TaskUpdate",
    "TaskList",
    "TaskGet",
    "TaskOutput",
    "TaskStop",
    "Skill",
];
fn allowed_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        && !DISALLOWED_TOOLS.contains(&name)
}
pub(in crate::infrastructure::claude_acp) fn tool_call(
    value: &Value,
    names: &mut HashMap<String, String>,
) -> Result<ToolCallUpdate, AgentError> {
    // Validate the complete representation before retaining provider name state.
    let update = acp_tool_call(value)?;
    let id = identifier(value, "toolCallId")?.to_owned();
    if let Some(name) = value
        .pointer("/_meta/claudeCode/toolName")
        .and_then(Value::as_str)
    {
        // Names are bounded before retention, including future native tool names.
        // Together with 256-byte IDs and 4,096 entries, this bounds the map's
        // string payload independently of incoming frame size.
        if !allowed_name(name) {
            // A rejected name can occupy the entire frame. Do not copy it into
            // an error that teardown will retain and clone.
            return Err(AgentError::Unsupported(
                "tool is outside the configured tool profile".into(),
            ));
        }
        if names.get(&id).is_some_and(|old| old != name) {
            return Err(protocol("tool identity changed"));
        }
        if names.len() >= 4096 && !names.contains_key(&id) {
            return Err(protocol("tool count limit exceeded"));
        }
        names.insert(id.clone(), name.to_owned());
    }
    Ok(update)
}
#[allow(dead_code)] // These structs validate the pinned harness schemas; preserve original arguments below.
pub(in crate::infrastructure::claude_acp) fn tool_input(
    name: &str,
    value: &Value,
) -> Result<ToolReviewInput, AgentError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Read {
        file_path: String,
        offset: Option<u64>,
        limit: Option<u64>,
        pages: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Write {
        file_path: String,
        content: String,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Edit {
        file_path: String,
        old_string: String,
        new_string: String,
        #[serde(default)]
        replace_all: bool,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Glob {
        pattern: String,
        path: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
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
        _ if allowed_name(name) && value.is_object() => {}
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
