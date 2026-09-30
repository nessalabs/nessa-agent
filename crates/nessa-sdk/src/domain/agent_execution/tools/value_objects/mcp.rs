#![deny(missing_docs)]

use crate::domain::agent_execution::ExecutionError;

/// The longest MCP server or tool name retained, in UTF-8 bytes. The MCP
/// specification asks for tool names of at most 128 characters; a server's
/// name is the key it was configured under, held to the same bound.
pub const MAX_MCP_NAME_BYTES: usize = 128;

/// The MCP server and tool an observed call was made to, as the agent harness
/// named them.
///
/// Identity for display and correlation only: it grants nothing, reaches no
/// server, and does not say the server is still configured. Names are
/// non-empty, at most [`MAX_MCP_NAME_BYTES`] bytes, and hold no whitespace or
/// control characters, so each can be shown on one line and compared exactly.
///
/// What the tool declared about itself — its `_meta`, and the UI resource an
/// MCP Apps host draws — is not here: no harness passes a tool's declaration
/// through ACP, so it is read from the server itself, not from the call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpTool {
    server: Box<str>,
    tool: Box<str>,
}
impl McpTool {
    /// Name the `server` and `tool` of an MCP call.
    ///
    /// # Errors
    ///
    /// [`ExecutionError::EmptyValue`] for an empty name,
    /// [`ExecutionError::ValueTooLong`] past [`MAX_MCP_NAME_BYTES`], and
    /// [`ExecutionError::InvalidMcpToolName`] for whitespace or a control
    /// character; the field names which of the two was rejected.
    pub fn new(server: impl Into<String>, tool: impl Into<String>) -> Result<Self, ExecutionError> {
        Ok(Self {
            server: name(server.into(), "MCP server name")?,
            tool: name(tool.into(), "MCP tool name")?,
        })
    }
    /// The server's name, as the harness's configuration keyed it.
    pub fn server(&self) -> &str {
        &self.server
    }
    /// The tool's name on that server.
    pub fn tool(&self) -> &str {
        &self.tool
    }
    /// Retained variable payload bytes: both names.
    pub fn payload_bytes(&self) -> usize {
        self.server.len().saturating_add(self.tool.len())
    }
}

fn name(value: String, field: &'static str) -> Result<Box<str>, ExecutionError> {
    if value.is_empty() {
        return Err(ExecutionError::EmptyValue(field));
    }
    if value.len() > MAX_MCP_NAME_BYTES {
        return Err(ExecutionError::ValueTooLong {
            field,
            max_bytes: MAX_MCP_NAME_BYTES,
        });
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ExecutionError::InvalidMcpToolName(field));
    }
    Ok(value.into_boxed_str())
}
