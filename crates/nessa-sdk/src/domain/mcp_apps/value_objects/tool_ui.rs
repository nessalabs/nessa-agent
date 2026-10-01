use super::super::McpAppError;
use crate::domain::agent_execution::tools::McpTool;

/// The longest `ui://` URI kept, in UTF-8 bytes.
pub const MAX_UI_URI_BYTES: usize = 2048;

/// The URI of an MCP App's UI resource: `ui://` and then at least one
/// character, at most [`MAX_UI_URI_BYTES`] bytes, with no whitespace or
/// control character, so it can be compared exactly and shown on one line.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UiResourceUri(Box<str>);
impl UiResourceUri {
    /// The scheme every MCP Apps resource URI starts with.
    pub const SCHEME: &'static str = "ui://";

    /// Read `uri` as an MCP App resource URI.
    ///
    /// # Errors
    ///
    /// [`McpAppError::NotUiUri`] when it does not start with `ui://` or names
    /// nothing after it, [`McpAppError::ValueTooLong`] past
    /// [`MAX_UI_URI_BYTES`], and [`McpAppError::InvalidValue`] for whitespace
    /// or a control character.
    pub fn new(uri: impl Into<String>) -> Result<Self, McpAppError> {
        let uri = uri.into();
        if uri.len() > MAX_UI_URI_BYTES {
            return Err(McpAppError::ValueTooLong {
                field: "UI resource URI",
                max_bytes: MAX_UI_URI_BYTES,
            });
        }
        if !uri.starts_with(Self::SCHEME) || uri.len() == Self::SCHEME.len() {
            return Err(McpAppError::NotUiUri);
        }
        if uri.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(McpAppError::InvalidValue("UI resource URI"));
        }
        Ok(Self(uri.into_boxed_str()))
    }
    /// The URI as it was given.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Who may see and call a tool with a UI (`_meta.ui.visibility`): the model,
/// the app, or both. A tool that does not say is visible to both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiVisibility {
    model: bool,
    app: bool,
}
impl UiVisibility {
    /// What a tool that does not say is: visible to the model and the app.
    pub const BOTH: Self = Self {
        model: true,
        app: true,
    };
    /// Visible to the model when `model`, to the app when `app`.
    pub fn new(model: bool, app: bool) -> Self {
        Self { model, app }
    }
    /// Whether the model may see and call the tool.
    pub fn model(self) -> bool {
        self.model
    }
    /// Whether the app may call the tool.
    pub fn app(self) -> bool {
        self.app
    }
}

/// What a tool declares about its UI (`_meta.ui`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolUi {
    resource_uri: UiResourceUri,
    visibility: UiVisibility,
}
impl ToolUi {
    /// A tool whose UI is `resource_uri`, seen by whom `visibility` says.
    pub fn new(resource_uri: UiResourceUri, visibility: UiVisibility) -> Self {
        Self {
            resource_uri,
            visibility,
        }
    }
    /// The UI resource the tool's result is drawn in.
    pub fn resource_uri(&self) -> &UiResourceUri {
        &self.resource_uri
    }
    /// Who may see and call the tool.
    pub fn visibility(&self) -> UiVisibility {
        self.visibility
    }
}

/// One tool as its server listed it (`tools/list`): the server and the tool's
/// own name, exactly as the server spelled it, and its UI when it declared one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedTool {
    tool: McpTool,
    ui: Option<ToolUi>,
}
impl ListedTool {
    /// `tool` as listed, with the UI it declared.
    pub fn new(tool: McpTool, ui: Option<ToolUi>) -> Self {
        Self { tool, ui }
    }
    /// The server and the tool's name on it.
    pub fn tool(&self) -> &McpTool {
        &self.tool
    }
    /// The UI the tool declared, if any.
    pub fn ui(&self) -> Option<&ToolUi> {
        self.ui.as_ref()
    }
    /// The UI of the one tool in `listed` that an observed call `call` names
    /// ([`McpTool::names`]), or `None` when no listed tool is named, when the
    /// one named declared no UI, or when more than one could be: a guess
    /// would draw one tool's UI for another's call.
    pub fn ui_for<'a>(listed: &'a [ListedTool], call: &McpTool) -> Option<&'a ToolUi> {
        let mut named = listed.iter().filter(|each| call.names(&each.tool));
        let only = named.next()?;
        if named.next().is_some() {
            return None;
        }
        only.ui.as_ref()
    }
}
