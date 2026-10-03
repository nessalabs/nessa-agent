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
/// because they never change for that call and are what a reader is shown.
/// Nothing here is authenticated: an app speaks on the person's behalf, under
/// their credential, and this says which app it was.
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum MessageSender {
    /// The person wrote it.
    #[default]
    Person,
    /// An app sent it on the person's behalf, with their consent.
    App(McpAppSource),
}

/// What one MCP App gave the model to know (MCP Apps'
/// `ui/update-model-context`): its text, its structured content, or both.
///
/// It travels with a user message, ahead of what the message says, and is
/// not part of what the person said: a transcript shows the message, not
/// this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppContext {
    app: McpAppSource,
    text: Option<Box<str>>,
    structured_content: Option<Box<str>>,
}
impl AppContext {
    /// Most UTF-8 bytes of one app's context, its text and its structured
    /// content's JSON together.
    pub const MAX_BYTES: usize = 8 * 1024;

    /// `app`'s context: `text`, and `structured_content` as the JSON text of
    /// one object. An empty text is none.
    ///
    /// # Errors
    ///
    /// [`ExecutionError::EmptyValue`] when there is neither;
    /// [`ExecutionError::ValueTooLong`] past [`Self::MAX_BYTES`] together;
    /// [`ExecutionError::InvalidStructuredContent`] for structured content
    /// that is not the JSON text of one object.
    pub fn new(
        app: McpAppSource,
        text: Option<String>,
        structured_content: Option<String>,
    ) -> Result<Self, ExecutionError> {
        let text = text.filter(|text| !text.is_empty());
        if text.is_none() && structured_content.is_none() {
            return Err(ExecutionError::EmptyValue("app context"));
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
        Ok(Self {
            app,
            text: text.map(String::into_boxed_str),
            structured_content: structured_content.map(String::into_boxed_str),
        })
    }
    /// The app that gave it.
    pub fn app(&self) -> &McpAppSource {
        &self.app
    }
    /// Its text, when it gave any.
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }
    /// Its structured content, the JSON text of one object, when it gave any.
    pub fn structured_content(&self) -> Option<&str> {
        self.structured_content.as_deref()
    }
    /// Retained variable payload bytes: the app and both parts.
    pub fn payload_bytes(&self) -> usize {
        self.app
            .payload_bytes()
            .saturating_add(self.text.as_deref().map_or(0, str::len))
            .saturating_add(self.structured_content.as_deref().map_or(0, str::len))
    }
}
