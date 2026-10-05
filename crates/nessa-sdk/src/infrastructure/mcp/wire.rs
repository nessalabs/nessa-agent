//! MCP's JSON, read into the domain's values: the `initialize` answer, a
//! `tools/list` page, a `resources/read` of an MCP App, and a tool result's
//! `structuredContent`.
use super::McpError;
use crate::domain::agent_execution::tools::{McpTool, ToolContent, MAX_STRUCTURED_RESULT_BYTES};
use crate::domain::agent_execution::ExecutionError;
use crate::domain::mcp_apps::{
    ListedTool, McpAppError, ToolHints, ToolUi, UiCsp, UiPermissions, UiResource, UiResourceUri,
    UiVisibility, APP_MIME_TYPE, EXTENSION,
};
use crate::infrastructure::json_rpc::json_fits;
use base64::Engine;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// The protocol revision this client asks for.
pub(crate) const PROTOCOL_VERSION: &str = "2025-06-18";
/// The revisions it accepts a server answering with. Each has `tools/list`
/// and `resources/read` as used here; none it does not know.
pub(crate) const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// What this client sends as `initialize`: the revision it speaks, the MCP
/// Apps extension with the one MIME type it draws, and no other capability.
pub(crate) fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "extensions": { (EXTENSION): { "mimeTypes": [APP_MIME_TYPE] } } },
        "clientInfo": { "name": "nessa", "version": env!("CARGO_PKG_VERSION") },
    })
}

/// Check a server's `initialize` answer: an object naming a revision this
/// client accepts.
pub(crate) fn initialized(result: &Value) -> Result<(), McpError> {
    let version = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::Handshake("no protocolVersion".into()))?;
    if !SUPPORTED_VERSIONS.contains(&version) {
        return Err(McpError::Handshake(format!(
            "unsupported protocol version {version}"
        )));
    }
    Ok(())
}

/// The text a structured result past the domain's bound
/// ([`MAX_STRUCTURED_RESULT_BYTES`]) is replaced by: said, not silently dropped.
pub(crate) const STRUCTURED_RESULT_OMITTED: &str = "[structured tool result omitted: too large]";

/// A tool result's `structuredContent`, as observed content: its JSON text,
/// or [`STRUCTURED_RESULT_OMITTED`] past [`MAX_STRUCTURED_RESULT_BYTES`] in
/// place of JSON cut short (at and past the bound:
/// `s2_a_structured_result_past_the_bound_is_kept_as_said_never_cut`).
/// Counted before it is written out, so a result of any size costs no more
/// than the bound.
///
/// # Errors
///
/// What [`ToolContent::structured`] refuses. The stand-in then keeps nothing;
/// Codex's adapter refuses the frame.
pub(crate) fn structured_result(structured: &Value) -> Result<ToolContent, ExecutionError> {
    if !json_fits(structured, MAX_STRUCTURED_RESULT_BYTES) {
        return Ok(ToolContent::text(STRUCTURED_RESULT_OMITTED));
    }
    ToolContent::structured(structured.to_string())
}

/// One `tools/list` page from `server`.
pub(crate) struct ToolsPage {
    /// Its tools. A tool whose name cannot be an [`McpTool`] is left out; one
    /// whose `_meta.ui.resourceUri` cannot be read is kept without a UI, and
    /// its `visibility` is read all the same ([`declared_ui`]).
    pub(crate) tools: Vec<ListedTool>,
    /// Each named tool on it, with whether the model may not see it
    /// ([`model_may_see`]).
    pub(crate) hidden: Vec<(String, bool)>,
    /// The cursor of the next page, when there is one.
    pub(crate) next: Option<String>,
}

pub(crate) fn tools_page(server: &str, result: &Value) -> Result<ToolsPage, McpError> {
    let tools = result
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| McpError::Malformed("tools/list without a tools array".into()))?;
    let next = match result.get("nextCursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(cursor)) => Some(cursor.clone()),
        Some(_) => {
            return Err(McpError::Malformed(
                "a nextCursor that is not a string".into(),
            ))
        }
    };
    let listed = tools
        .iter()
        .filter_map(|tool| {
            let name = tool.get("name")?.as_str()?;
            let identity = McpTool::new(server, name).ok()?;
            Some(
                ListedTool::new(identity, tool_ui(tool))
                    .with_hints(tool_hints(tool.get("annotations"))),
            )
        })
        .collect();
    let hidden = tools
        .iter()
        .filter_map(|tool| Some((tool.get("name")?.as_str()?.to_owned(), !model_may_see(tool))))
        .collect();
    Ok(ToolsPage {
        tools: listed,
        hidden,
        next,
    })
}

/// Whether the model may see and call `tool`, a tool as `tools/list` gives it
/// ([`declared_ui`]).
pub(crate) fn model_may_see(tool: &Value) -> bool {
    tool_visibility(tool).model()
}

/// Who may see `tool`, one entry as `tools/list` gives it ([`declared_ui`]).
pub(crate) fn tool_visibility(tool: &Value) -> UiVisibility {
    declared_ui(tool).1
}

/// One visibility per name, in the order each name is first seen. A side is
/// included only when every entry for that name includes it
/// ([`UiVisibility::every`]).
pub(crate) fn one_visibility_per_name(
    entries: impl IntoIterator<Item = (String, UiVisibility)>,
) -> Vec<(String, UiVisibility)> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut folded: Vec<(String, UiVisibility)> = Vec::new();
    for (name, who) in entries {
        if let Some(&at) = index.get(&name) {
            folded[at].1 = folded[at].1.every(who);
        } else {
            index.insert(name.clone(), folded.len());
            folded.push((name, who));
        }
    }
    folded
}

/// The model record for a kept list: one bool per name, hidden when any
/// entry excludes the model. Names that could not be a [`ListedTool`] are
/// taken from `hidden`, which is the per-entry record [`tools_page`] built.
pub(crate) fn hidden_for_model(
    tools: &[ListedTool],
    hidden: &[(String, bool)],
) -> Vec<(String, bool)> {
    let known: HashSet<String> = tools
        .iter()
        .map(|tool| tool.tool().tool().to_owned())
        .collect();
    let mut entries: Vec<(String, UiVisibility)> = tools
        .iter()
        .map(|tool| (tool.tool().tool().to_owned(), tool.ui().visibility()))
        .collect();
    for (name, is_hidden) in hidden {
        if known.contains(name) {
            continue;
        }
        entries.push((name.clone(), UiVisibility::new(!is_hidden, true)));
    }
    one_visibility_per_name(entries)
        .into_iter()
        .map(|(name, who)| (name, !who.model()))
        .collect()
}

/// A listed tool's `_meta.ui`, and who may see it. One reading for the model
/// and for an app: no `_meta.ui` is both; a `_meta.ui` that is present and
/// not an object cannot be read, and is no one's (#424), the same as a
/// `visibility` that is not an array of strings.
fn declared_ui(tool: &Value) -> (Option<&Value>, UiVisibility) {
    match tool.pointer("/_meta/ui") {
        Some(ui) if ui.is_object() => (Some(ui), visibility(ui.get("visibility"))),
        Some(_) => (None, UiVisibility::new(false, false)),
        None => (None, visibility(None)),
    }
}

/// `_meta.ui.visibility`: absent is both; an array of strings names who.
/// Anything else cannot be read, and is no one's: it cannot be read to
/// include `model` or `app`, so the model is not shown the tool and an app's
/// call to it is refused (#412).
fn visibility(declared: Option<&Value>) -> UiVisibility {
    match declared {
        None => UiVisibility::BOTH,
        Some(Value::Array(who)) if who.iter().all(Value::is_string) => {
            let says = |name: &str| who.iter().any(|each| each.as_str() == Some(name));
            UiVisibility::new(says("model"), says("app"))
        }
        Some(_) => UiVisibility::new(false, false),
    }
}

/// A tool's `annotations`: each hint only when it is a boolean; anything else
/// is as if unsaid, which the rule reads as the riskier answer.
fn tool_hints(annotations: Option<&Value>) -> ToolHints {
    let hint = |name: &str| {
        annotations
            .and_then(|each| each.get(name))
            .and_then(Value::as_bool)
    };
    ToolHints::new(hint("readOnlyHint"), hint("destructiveHint"))
}

/// A tool's `_meta.ui`, each part read on its own: a `resourceUri` that is
/// absent or cannot be read is no UI, and does not take the tool's
/// `visibility` with it. Who may see it is [`declared_ui`].
fn tool_ui(tool: &Value) -> ToolUi {
    let (declared, who) = declared_ui(tool);
    let uri = declared
        .and_then(|ui| ui.get("resourceUri"))
        .and_then(Value::as_str)
        .and_then(|uri| UiResourceUri::new(uri).ok());
    ToolUi::new(uri, who)
}

/// The MCP App at `requested`, from a `resources/read` answer: the one content
/// for that URI, of MIME `text/html;profile=mcp-app`, holding `text` or a
/// UTF-8 `blob`, and its `_meta.ui`.
pub(crate) fn ui_resource(
    requested: &UiResourceUri,
    result: &Value,
) -> Result<UiResource, McpError> {
    let contents = result
        .get("contents")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("resources/read without a contents array"))?;
    let content = contents
        .iter()
        .find(|content| content.get("uri").and_then(Value::as_str) == Some(requested.as_str()))
        .ok_or_else(|| malformed("no content for the requested URI"))?;
    if content.get("mimeType").and_then(Value::as_str) != Some(APP_MIME_TYPE) {
        return Err(McpError::NotAnApp);
    }
    let html = match (content.get("text"), content.get("blob")) {
        (Some(Value::String(text)), None) => text.clone(),
        (None, Some(Value::String(blob))) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(blob)
                .map_err(|_| malformed("a blob that is not base64"))?;
            String::from_utf8(bytes).map_err(|_| malformed("a blob that is not UTF-8"))?
        }
        _ => return Err(malformed("a content without one text or blob")),
    };
    let ui = content.pointer("/_meta/ui");
    let csp = match ui.and_then(|ui| ui.get("csp")) {
        None => UiCsp::default(),
        Some(csp) => UiCsp::new(
            domains(csp, "connectDomains")?,
            domains(csp, "resourceDomains")?,
            domains(csp, "frameDomains")?,
            domains(csp, "baseUriDomains")?,
        )
        .map_err(refused)?,
    };
    let permissions = match ui.and_then(|ui| ui.get("permissions")) {
        None => UiPermissions::default(),
        // Asked for as the extension writes it, `{}`, or as `true`; `false`,
        // `null` or anything else asks for nothing.
        Some(Value::Object(asked)) => {
            let asks = |name: &str| {
                asked
                    .get(name)
                    .is_some_and(|value| value.is_object() || *value == Value::Bool(true))
            };
            UiPermissions {
                camera: asks("camera"),
                microphone: asks("microphone"),
                geolocation: asks("geolocation"),
                clipboard_write: asks("clipboardWrite"),
            }
        }
        Some(_) => return Err(malformed("permissions that are not an object")),
    };
    let domain = match ui.and_then(|ui| ui.get("domain")) {
        None => None,
        Some(Value::String(domain)) => Some(domain.clone()),
        Some(_) => return Err(malformed("a domain that is not a string")),
    };
    let prefers_border = match ui.and_then(|ui| ui.get("prefersBorder")) {
        None => None,
        Some(Value::Bool(prefers)) => Some(*prefers),
        Some(_) => return Err(malformed("prefersBorder that is not a boolean")),
    };
    UiResource::new(
        requested.clone(),
        html,
        csp,
        permissions,
        domain,
        prefers_border,
    )
    .map_err(refused)
}

fn domains(csp: &Value, key: &str) -> Result<Vec<String>, McpError> {
    match csp.get(key) {
        None => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| malformed("a CSP domain that is not a string"))
            })
            .collect(),
        Some(_) => Err(malformed("a CSP list that is not an array")),
    }
}

fn malformed(reason: &str) -> McpError {
    McpError::Malformed(reason.into())
}

/// A value the domain refused: past a bound is [`McpError::TooLarge`],
/// anything else [`McpError::Malformed`].
fn refused(error: McpAppError) -> McpError {
    match error {
        McpAppError::ValueTooLong { field, .. } | McpAppError::TooManyValues { field, .. } => {
            McpError::TooLarge(field)
        }
        McpAppError::NotUiUri | McpAppError::InvalidValue(_) => {
            McpError::Malformed(error.to_string())
        }
    }
}
