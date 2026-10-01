//! What an MCP server says about its apps (MCP Apps, `io.modelcontextprotocol/ui`):
//! which of its tools have a UI, and the UI resource itself.
//!
//! ```text
//! ListedTool --> McpTool (agent_execution::tools)
//!     `-------> ToolUi --> UiResourceUri
//! UiResource --> UiResourceUri + UiCsp + UiPermissions
//! ```
//!
//! Arrows mean "holds". These are values read from a server, validated when
//! they are made: a URI is a `ui://` URI, the HTML and every CSP source are
//! bounded, and a CSP source can only hold characters that cannot end it or
//! start another directive. Nothing here reaches a server; the MCP client in
//! `infrastructure::mcp` reads them.
#![deny(missing_docs)]

mod error;
mod value_objects;

/// The identifier of the MCP Apps extension, the key a host and a server
/// declare it under in `capabilities.extensions`.
pub const EXTENSION: &str = "io.modelcontextprotocol/ui";
/// The MIME type of an MCP App's UI resource.
pub const APP_MIME_TYPE: &str = "text/html;profile=mcp-app";

pub use error::McpAppError;
pub use value_objects::{
    ListedTool, ToolUi, UiCsp, UiPermissions, UiResource, UiResourceUri, UiVisibility,
    MAX_CSP_SOURCES, MAX_CSP_SOURCE_BYTES, MAX_UI_HTML_BYTES, MAX_UI_URI_BYTES,
};
