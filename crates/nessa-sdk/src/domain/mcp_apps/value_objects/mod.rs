//! Immutable values an MCP server sends about its apps, checked when made.
mod tool_ui;
mod ui_resource;
pub use tool_ui::{ListedTool, ToolHints, ToolUi, UiResourceUri, UiVisibility, MAX_UI_URI_BYTES};
pub use ui_resource::{
    UiCsp, UiPermissions, UiResource, MAX_CSP_SOURCES, MAX_CSP_SOURCE_BYTES, MAX_UI_HTML_BYTES,
};
