use crate::application::agent_binding::*;
use crate::domain::agent_execution::value_objects::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

pub(super) const FILE_TOOLS: &[&str] = &["Read", "Write", "Edit", "Glob", "Grep"];
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub(super) enum RpcId {
    Number(i64),
    Text(String),
}
impl RpcId {
    pub fn value(&self) -> Value {
        match self {
            Self::Number(id) => json!(id),
            Self::Text(id) => json!(id),
        }
    }
}
#[derive(Deserialize)]
pub(super) struct Envelope {
    pub jsonrpc: String,
    pub id: Option<RpcId>,
    pub method: Option<String>,
    pub params: Option<Value>,
    pub result: Option<Value>,
    pub error: Option<RpcError>,
}
#[derive(Deserialize)]
pub(super) struct RpcError {
    pub code: i64,
}
pub(super) fn parse(frame: &[u8]) -> Result<Envelope, BindingError> {
    let message: Envelope =
        serde_json::from_slice(frame).map_err(|_| protocol("invalid JSON-RPC frame"))?;
    if matches!(&message.id, Some(RpcId::Text(id)) if id.is_empty() || id.len() > 256) {
        return Err(protocol("invalid RPC identifier"));
    }
    if message.jsonrpc != "2.0"
        || (message.method.is_some() && (message.result.is_some() || message.error.is_some()))
        || (message.method.is_none()
            && (message.id.is_none() || message.result.is_some() == message.error.is_some()))
    {
        return Err(protocol("invalid JSON-RPC envelope"));
    }
    Ok(message)
}
pub(super) fn protocol(message: &str) -> BindingError {
    BindingError::Protocol(message.into())
}
pub(super) fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str, BindingError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| protocol(&format!("missing {field}")))
}
pub(super) fn identifier<'a>(value: &'a Value, field: &str) -> Result<&'a str, BindingError> {
    let value = string(value, field)?;
    if value.len() > 256 {
        return Err(protocol("identifier exceeds limit"));
    }
    Ok(value)
}
pub(super) fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}
pub(super) fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","method":method,"params":params})
}
pub(super) fn permission_cancel(id: &RpcId) -> Value {
    json!({"jsonrpc":"2.0","id":id.value(),"result":{"outcome":{"outcome":"cancelled"}}})
}
pub(super) fn selected(id: &RpcId, option: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id.value(),"result":{"outcome":{"outcome":"selected","optionId":option}}})
}
pub(super) fn unsupported(id: &RpcId) -> Value {
    json!({"jsonrpc":"2.0","id":id.value(),"error":{"code":-32601,"message":"Client operation is not enabled"}})
}

pub(super) fn verify_config(
    result: &Value,
    model: &str,
    require_mode: bool,
) -> Result<(), BindingError> {
    let options = result
        .get("configOptions")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol("missing session config options"))?;
    let current = |id| {
        options
            .iter()
            .find(|option| option.get("id").and_then(Value::as_str) == Some(id))
            .and_then(|option| option.get("currentValue"))
            .and_then(Value::as_str)
    };
    if current("model") != Some(model) {
        return Err(protocol(
            "provider did not select the exact configured model",
        ));
    }
    if require_mode && current("mode") != Some("default") {
        return Err(protocol("provider permission mode is not default"));
    }
    Ok(())
}
pub(super) fn outcome(value: &Value) -> Result<PromptOutcome, BindingError> {
    match string(value, "stopReason")? {
        "end_turn" => Ok(PromptOutcome::Completed),
        "max_tokens" => Ok(PromptOutcome::OutputLimit),
        "max_turn_requests" => Ok(PromptOutcome::RequestLimit),
        "refusal" => Ok(PromptOutcome::Refused),
        "cancelled" => Ok(PromptOutcome::Cancelled),
        _ => Err(protocol("unknown prompt stop reason")),
    }
}

pub(super) fn tool_call(
    value: &Value,
    names: &mut HashMap<String, String>,
) -> Result<ToolCallUpdate, BindingError> {
    let id = identifier(value, "toolCallId")?.to_owned();
    if let Some(name) = value
        .pointer("/_meta/claudeCode/toolName")
        .and_then(Value::as_str)
    {
        if !FILE_TOOLS.contains(&name) {
            return Err(BindingError::Unsupported(format!(
                "tool {name} is outside the file-tool profile"
            )));
        }
        if names.get(&id).is_some_and(|old| old != name) {
            return Err(protocol("tool identity changed"));
        }
        if names.len() >= 4096 && !names.contains_key(&id) {
            return Err(protocol("tool count limit exceeded"));
        }
        names.insert(id.clone(), name.to_owned());
    }
    let kind = value
        .get("kind")
        .map(|kind| match kind.as_str() {
            Some("read") => Ok(ToolKind::Read),
            Some("edit") => Ok(ToolKind::Edit),
            Some("search") => Ok(ToolKind::Search),
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
                .collect::<Result<Vec<_>, BindingError>>()
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
                if item.get("type").and_then(Value::as_str) == Some("diff") {
                    let old = match item.get("oldText") {
                        None | Some(Value::Null) => None,
                        Some(Value::String(text)) => Some(text.clone()),
                        _ => return Err(protocol("invalid diff old text")),
                    };
                    result.push(ToolContent::Diff {
                        path: path(string(item, "path")?)?,
                        old,
                        new: item
                            .get("newText")
                            .and_then(Value::as_str)
                            .ok_or_else(|| protocol("missing diff new text"))?
                            .to_owned(),
                    });
                } else if item.get("type").and_then(Value::as_str) == Some("content") {
                    let block = item
                        .get("content")
                        .ok_or_else(|| protocol("missing tool content block"))?;
                    if block.get("type").and_then(Value::as_str) == Some("text") {
                        result.push(ToolContent::Text(
                            block
                                .get("text")
                                .and_then(Value::as_str)
                                .ok_or_else(|| protocol("invalid tool text"))?
                                .to_owned(),
                        ));
                    }
                }
            }
            Ok::<_, BindingError>(result)
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

/// Deserialize every admitted input field; an unfamiliar schema cannot be allowed.
fn path(value: impl Into<String>) -> Result<FilePath, BindingError> {
    FilePath::new(value).map_err(|error| protocol(&error.to_string()))
}

pub(super) fn file_input(name: &str, value: &Value) -> Result<FileToolInput, BindingError> {
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
    fn parse<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, BindingError> {
        serde_json::from_value(value.clone()).map_err(|_| protocol("unsupported file-tool input"))
    }
    match name {
        "Read" => {
            let v: Read = parse(value)?;
            Ok(FileToolInput::Read {
                path: path(v.file_path)?,
                offset: v.offset,
                limit: v.limit,
                pages: v.pages,
            })
        }
        "Write" => {
            let v: Write = parse(value)?;
            Ok(FileToolInput::Write {
                path: path(v.file_path)?,
                content: v.content,
            })
        }
        "Edit" => {
            let v: Edit = parse(value)?;
            Ok(FileToolInput::Edit {
                path: path(v.file_path)?,
                old: v.old_string,
                new: v.new_string,
                replace_all: v.replace_all,
            })
        }
        "Glob" => {
            let v: Glob = parse(value)?;
            Ok(FileToolInput::Glob {
                pattern: v.pattern,
                path: v.path.map(path).transpose()?,
            })
        }
        "Grep" => {
            let v: Grep = parse(value)?;
            Ok(FileToolInput::Grep {
                pattern: v.pattern,
                path: v.path.map(path).transpose()?,
                glob: v.glob,
                file_type: v.file_type,
                output_mode: v.output_mode,
                before: v.before,
                after: v.after,
                context: match (v.context, v.short_context) {
                    (Some(a), Some(b)) if a != b => {
                        return Err(protocol("conflicting search context fields"))
                    }
                    (Some(value), _) | (_, Some(value)) => Some(value),
                    _ => None,
                },
                line_numbers: v.line_numbers,
                ignore_case: v.ignore_case,
                only_matching: v.only_matching,
                multiline: v.multiline,
                head_limit: v.head_limit,
                offset: v.offset,
            })
        }
        _ => Err(protocol("permission for an unknown tool")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_file_inputs_have_one_typed_application_projection() {
        let cases = [
            (
                "Read",
                json!({"file_path":"/a","offset":2,"limit":5,"pages":"1-2"}),
                FileToolInput::Read {
                    path: FilePath::new("/a").unwrap(),
                    offset: Some(2),
                    limit: Some(5),
                    pages: Some("1-2".into()),
                },
            ),
            (
                "Write",
                json!({"file_path":"/a","content":"new"}),
                FileToolInput::Write {
                    path: FilePath::new("/a").unwrap(),
                    content: "new".into(),
                },
            ),
            (
                "Edit",
                json!({"file_path":"/a","old_string":"old","new_string":"new"}),
                FileToolInput::Edit {
                    path: FilePath::new("/a").unwrap(),
                    old: "old".into(),
                    new: "new".into(),
                    replace_all: false,
                },
            ),
            (
                "Glob",
                json!({"pattern":"*.rs"}),
                FileToolInput::Glob {
                    pattern: "*.rs".into(),
                    path: None,
                },
            ),
        ];
        for (name, input, expected) in cases {
            assert_eq!(file_input(name, &input).unwrap(), expected);
        }
        let input = json!({"pattern":"x","context":2,"-o":true});
        let FileToolInput::Grep {
            context,
            only_matching,
            ..
        } = file_input("Grep", &input).unwrap()
        else {
            panic!("expected search")
        };
        assert_eq!(context, Some(2));
        assert_eq!(only_matching, Some(true));
        // Both spellings are in the pinned SDK's current schema; conflicts cannot
        // be hidden from the host by choosing a projection precedence.
        assert!(file_input("Grep", &json!({"pattern":"x","context":1,"-C":2})).is_err());
        for (name, input) in [
            ("Bash", json!({"command":"true"})),
            (
                "Write",
                json!({"file_path":"/a","content":"x","command":"true"}),
            ),
            ("Read", json!({"file_path":"/a","offset":-1})),
            ("Write", json!({"file_path":"","content":"x"})),
            ("Read", json!({"file_path":"a\0b"})),
            ("Glob", json!({"pattern":"*","path":""})),
        ] {
            assert!(file_input(name, &input).is_err());
        }
    }

    #[test]
    fn sparse_tool_updates_keep_omission_distinct_from_empty_and_preserve_diffs() {
        let mut names = HashMap::new();
        let tool = tool_call(&json!({"toolCallId":"a","_meta":{"claudeCode":{"toolName":"Write"}},"content":[{"type":"diff","path":"/a","oldText":null,"newText":"new"}]}),&mut names).unwrap();
        assert_eq!(
            tool.content().clone(),
            Some(vec![ToolContent::Diff {
                path: FilePath::new("/a").unwrap(),
                old: None,
                new: "new".into()
            }])
        );
        let patch = tool_call(&json!({"toolCallId":"a","content":[]}), &mut names).unwrap();
        assert_eq!(patch.content().clone(), Some(vec![]));
        assert_eq!(patch.locations().clone(), None);
        assert!(tool_call(&json!({"toolCallId":" "}), &mut names).is_err());
        assert!(tool_call(
            &json!({"toolCallId":"a", "locations":[{"path":""}]}),
            &mut names
        )
        .is_err());
        assert!(tool_call(
            &json!({"toolCallId":"a","_meta":{"claudeCode":{"toolName":"Edit"}}}),
            &mut names
        )
        .is_err());
        assert!(tool_call(
            &json!({"toolCallId":"b","_meta":{"claudeCode":{"toolName":"Bash"}}}),
            &mut names
        )
        .is_err());
    }

    #[test]
    fn malformed_envelopes_and_oversized_identifiers_cannot_enter_dispatch() {
        for frame in [
            json!({"jsonrpc":"1.0","id":1,"result":{}}),
            json!({"jsonrpc":"2.0","result":{}}),
            json!({"jsonrpc":"2.0","id":1,"result":{},"error":{"code":0}}),
            json!({"jsonrpc":"2.0","id":1,"method":"x","result":{}}),
            json!({"jsonrpc":"2.0","id":"x".repeat(257),"result":{}}),
        ] {
            assert!(parse(&serde_json::to_vec(&frame).unwrap()).is_err());
        }
    }
}
