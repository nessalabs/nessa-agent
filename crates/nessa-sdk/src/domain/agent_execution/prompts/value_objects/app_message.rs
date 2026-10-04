#![deny(missing_docs)]

use crate::domain::agent_execution::{
    executions::ExecutionId,
    tools::{value_objects::json::is_json, McpTool, ToolCallId},
    ExecutionError,
};

/// An MCP App, named by the tool call whose UI it is: the execution and tool
/// call that drew it, and the MCP server and tool that call was to.
///
/// The tool call is the app's identity; the server and tool are kept beside it
/// because a tool call names one MCP tool for good (an update naming another
/// is [`ExecutionError::DifferentMcpTool`]) and they are what a reader is
/// shown.
/// Nothing here is authenticated: an app speaks on the person's behalf, under
/// their credential, and this says which app it was.
///
/// Building one checks only its own fields. A session admits a message that
/// names it — as its writer, or as the giver of a context it carries — only
/// when its tool call is an MCP tool call an earlier turn of that session
/// observed, to this same server and tool; otherwise admission refuses the
/// message (the session's `AgentError::UnknownApp`) and saves nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpAppSource {
    execution_id: ExecutionId,
    tool_id: ToolCallId,
    tool: McpTool,
}
impl McpAppSource {
    /// Most UTF-8 bytes of the tool call's identity, as an execution's.
    pub const MAX_TOOL_ID_BYTES: usize = ExecutionId::MAX_BYTES;

    /// The app drawn for the tool call `tool_id` of `execution_id`, which
    /// called `tool`.
    ///
    /// # Errors
    ///
    /// [`ExecutionError::ValueTooLong`] for a tool call identity past
    /// [`Self::MAX_TOOL_ID_BYTES`]; the identities' own constructors refuse
    /// the rest.
    pub fn new(
        execution_id: ExecutionId,
        tool_id: ToolCallId,
        tool: McpTool,
    ) -> Result<Self, ExecutionError> {
        if tool_id.as_str().len() > Self::MAX_TOOL_ID_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "app tool call ID",
                max_bytes: Self::MAX_TOOL_ID_BYTES,
            });
        }
        Ok(Self {
            execution_id,
            tool_id,
            tool,
        })
    }
    /// The execution the app's tool call belongs to.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    /// The app's tool call.
    pub fn tool_id(&self) -> &ToolCallId {
        &self.tool_id
    }
    /// The MCP server and tool the app's tool call was to.
    pub fn tool(&self) -> &McpTool {
        &self.tool
    }
    /// Retained variable payload bytes: both identities and both names.
    pub fn payload_bytes(&self) -> usize {
        self.execution_id
            .as_str()
            .len()
            .saturating_add(self.tool_id.as_str().len())
            .saturating_add(self.tool.payload_bytes())
    }
}

/// Who wrote a user message: the person, or an MCP App on their behalf
/// (MCP Apps' `ui/message`). Either way it is the person's turn, under their
/// credential; this says whose words they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageSender {
    /// The person wrote it.
    Person,
    /// An app sent it on the person's behalf. Whether the person allowed it
    /// is the host's to decide before it says so.
    App(McpAppSource),
}

/// What one MCP App gave the model to know (MCP Apps'
/// `ui/update-model-context`): its text, its structured content, or both.
///
/// It travels with a user message, ahead of what the message says, and is
/// not part of what the person said: a transcript shows the message, not
/// this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppModelContext {
    app: McpAppSource,
    update: Box<str>,
    text: Option<Box<str>>,
    structured_content: Option<Box<str>>,
}
impl AppModelContext {
    /// Most UTF-8 bytes of one app's context, its text and its structured
    /// content's JSON together.
    pub const MAX_BYTES: usize = 8 * 1024;
    /// Most UTF-8 bytes of the identity of the update that gave it.
    pub const MAX_UPDATE_BYTES: usize = ExecutionId::MAX_BYTES;

    /// `app`'s context, as its update `update` gave it — the host's own
    /// identity for that update, which its records of it carry, so the turn
    /// that carried a context and the record of its update are one join —
    /// with `text`, and `structured_content` as the JSON text of one object.
    /// An empty text is none. Neither part is no context: `Ok(None)`, which
    /// a host reads as the app giving the model nothing.
    ///
    /// # Errors
    ///
    /// [`ExecutionError::EmptyValue`] for a blank `update`;
    /// [`ExecutionError::ValueTooLong`] for an `update` past
    /// [`Self::MAX_UPDATE_BYTES`];
    /// [`ExecutionError::ValueTooLong`] past [`Self::MAX_BYTES`] together;
    /// [`ExecutionError::InvalidStructuredContent`] for structured content
    /// that is not the JSON text of one object.
    ///
    /// # Examples
    ///
    /// ```
    /// use nessa_sdk::domain::agent_execution::executions::ExecutionId;
    /// use nessa_sdk::domain::agent_execution::prompts::{AppModelContext, McpAppSource};
    /// use nessa_sdk::domain::agent_execution::tools::{McpTool, ToolCallId};
    ///
    /// let app = McpAppSource::new(
    ///     ExecutionId::new("turn-1")?,
    ///     ToolCallId::new("call-1")?,
    ///     McpTool::new("charts", "plot")?,
    /// )?;
    /// let context = AppModelContext::new(
    ///     app.clone(),
    ///     "update-1",
    ///     Some("Showing April".into()),
    ///     Some(r#"{"month":4}"#.into()),
    /// )?
    /// .expect("it gave something");
    /// assert_eq!(context.structured_content(), Some(r#"{"month":4}"#));
    /// // Neither part: the app gives the model nothing.
    /// assert!(AppModelContext::new(app.clone(), "update-2", None, None)?.is_none());
    /// // Structure that is not one JSON object is refused.
    /// assert!(AppModelContext::new(app, "update-3", None, Some("[1]".into())).is_err());
    /// # Ok::<(), nessa_sdk::domain::agent_execution::ExecutionError>(())
    /// ```
    pub fn new(
        app: McpAppSource,
        update: &str,
        text: Option<String>,
        structured_content: Option<String>,
    ) -> Result<Option<Self>, ExecutionError> {
        if update.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("app context update"));
        }
        if update.len() > Self::MAX_UPDATE_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "app context update",
                max_bytes: Self::MAX_UPDATE_BYTES,
            });
        }
        let text = text.filter(|text| !text.is_empty());
        if text.is_none() && structured_content.is_none() {
            return Ok(None);
        }
        let bytes = text
            .as_ref()
            .map_or(0, String::len)
            .saturating_add(structured_content.as_ref().map_or(0, String::len));
        if bytes > Self::MAX_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "app context",
                max_bytes: Self::MAX_BYTES,
            });
        }
        // One JSON value whose first byte past any whitespace opens an
        // object is one object.
        if let Some(structured) = &structured_content {
            if !is_json(structured) || !structured.trim_start().starts_with('{') {
                return Err(ExecutionError::InvalidStructuredContent);
            }
        }
        Ok(Some(Self {
            app,
            update: update.into(),
            text: text.map(String::into_boxed_str),
            structured_content: structured_content.map(String::into_boxed_str),
        }))
    }
    /// The app that gave it.
    pub fn app(&self) -> &McpAppSource {
        &self.app
    }
    /// The identity of the update that gave it.
    pub fn update_id(&self) -> &str {
        &self.update
    }
    /// Its text, when it gave any.
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }
    /// Its structured content, the JSON text of one object, when it gave any.
    pub fn structured_content(&self) -> Option<&str> {
        self.structured_content.as_deref()
    }
    /// The bytes of what it says: its text and its structured content,
    /// together — what [`Self::MAX_BYTES`] bounds.
    pub fn content_bytes(&self) -> usize {
        self.text
            .as_deref()
            .map_or(0, str::len)
            .saturating_add(self.structured_content.as_deref().map_or(0, str::len))
    }
    /// Retained variable payload bytes: the app and both parts.
    pub fn payload_bytes(&self) -> usize {
        self.app
            .payload_bytes()
            .saturating_add(self.update.len())
            .saturating_add(self.content_bytes())
    }
}
