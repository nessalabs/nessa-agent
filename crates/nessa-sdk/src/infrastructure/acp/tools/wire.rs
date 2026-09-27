use super::super::fields::{identifier, string};
use crate::application::agent_execution::agents::AgentError;
use crate::domain::agent_execution::tools::{
    FileLocation, FilePath, ToolCallId, ToolCallUpdate, ToolContent, ToolKind, ToolStatus,
};
use crate::infrastructure::json_rpc::protocol;
use serde_json::Value;

/// Bounded display text for a structurally valid tool result Nessa cannot render.
pub(crate) const UNSUPPORTED_TOOL_CONTENT: &str = "[unsupported tool result content]";

pub(crate) fn tool_call(value: &Value) -> Result<ToolCallUpdate, AgentError> {
    let id = identifier(value, "toolCallId")?.to_owned();
    let kind = value
        .get("kind")
        .map(|kind| match kind.as_str() {
            Some("read") => Ok(ToolKind::Read),
            Some("edit") => Ok(ToolKind::Edit),
            Some("search") => Ok(ToolKind::Search),
            Some("fetch") => Ok(ToolKind::Fetch),
            Some("execute") => Ok(ToolKind::Execute),
            Some("think") => Ok(ToolKind::Think),
            Some("delete") => Ok(ToolKind::Delete),
            Some("move") => Ok(ToolKind::Move),
            Some("switch_mode") => Ok(ToolKind::SwitchMode),
            Some("other") => Ok(ToolKind::Other),
            _ => Err(protocol("unsupported tool kind")),
        })
        .transpose()?;
    let status = value
        .get("status")
        .map(|status| match status.as_str() {
            Some("pending") => Ok(ToolStatus::Pending),
            Some("in_progress") => Ok(ToolStatus::Running),
            Some("completed") => Ok(ToolStatus::Completed),
            Some("failed") => Ok(ToolStatus::Failed),
            _ => Err(protocol("invalid tool status")),
        })
        .transpose()?;
    let title = value
        .get("title")
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| protocol("invalid tool title"))
        })
        .transpose()?;
    let locations = value
        .get("locations")
        .map(|v| {
            v.as_array()
                .ok_or_else(|| protocol("invalid tool locations"))?
                .iter()
                .map(|location| {
                    let line = location
                        .get("line")
                        .map(|line| {
                            line.as_u64()
                                .and_then(|n| n.try_into().ok())
                                .ok_or_else(|| protocol("invalid location line"))
                        })
                        .transpose()?;
                    Ok(FileLocation::new(path(string(location, "path")?)?, line))
                })
                .collect::<Result<Vec<_>, AgentError>>()
        })
        .transpose()?;
    let content = value
        .get("content")
        .map(|v| {
            let content = v
                .as_array()
                .ok_or_else(|| protocol("invalid tool content"))?;
            let mut result = Vec::new();
            for item in content {
                match item
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or_else(|| protocol("invalid tool content type"))?
                {
                    "diff" => {
                        let old = match item.get("oldText") {
                            None | Some(Value::Null) => None,
                            Some(Value::String(text)) => Some(text.clone()),
                            _ => return Err(protocol("invalid diff old text")),
                        };
                        result.push(ToolContent::diff(
                            path(string(item, "path")?)?,
                            old,
                            item.get("newText")
                                .and_then(Value::as_str)
                                .ok_or_else(|| protocol("missing diff new text"))?
                                .to_owned(),
                        ));
                    }
                    "content" => {
                        let block = item
                            .get("content")
                            .ok_or_else(|| protocol("missing tool content block"))?;
                        match block
                            .get("type")
                            .and_then(Value::as_str)
                            .ok_or_else(|| protocol("invalid nested tool content type"))?
                        {
                            "text" => result.push(ToolContent::text(
                                block
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .ok_or_else(|| protocol("invalid tool text"))?
                                    .to_owned(),
                            )),
                            _ => result.push(ToolContent::text(UNSUPPORTED_TOOL_CONTENT)),
                        }
                    }
                    _ => result.push(ToolContent::text(UNSUPPORTED_TOOL_CONTENT)),
                }
            }
            Ok::<_, AgentError>(result)
        })
        .transpose()?;
    Ok(ToolCallUpdate::new(
        ToolCallId::new(id).map_err(|error| protocol(&error.to_string()))?,
        title,
        kind,
        status,
        locations,
        content,
    ))
}

pub(crate) fn path(value: impl Into<String>) -> Result<FilePath, AgentError> {
    FilePath::new(value).map_err(|error| protocol(&error.to_string()))
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/acp/tools/wire.rs"]
mod tests;
