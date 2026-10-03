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
        Self::checked(uri.into())
    }
    fn checked(uri: String) -> Result<Self, McpAppError> {
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

/// Who may see and call a tool (`_meta.ui.visibility`): the model, the app,
/// or both. A tool that does not say, with or without a UI, is visible to
/// both.
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

/// What a tool declares in `_meta.ui`: the UI resource its result is drawn
/// in, when it has one, and who may see and call it. Every listed tool has
/// one; a tool that declares nothing has no UI and is visible to both
/// ([`ToolUi::default`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolUi {
    resource_uri: Option<UiResourceUri>,
    visibility: UiVisibility,
}
impl ToolUi {
    /// A tool drawn in `resource_uri`, when it names one, seen by whom
    /// `visibility` says.
    pub fn new(resource_uri: Option<UiResourceUri>, visibility: UiVisibility) -> Self {
        Self {
            resource_uri,
            visibility,
        }
    }
    /// The UI resource the tool's result is drawn in, or `None` when it has
    /// no UI.
    pub fn resource_uri(&self) -> Option<&UiResourceUri> {
        self.resource_uri.as_ref()
    }
    /// Who may see and call the tool.
    pub fn visibility(&self) -> UiVisibility {
        self.visibility
    }
}
impl Default for ToolUi {
    /// What a tool with no `_meta.ui` declares: no UI, visible to both.
    fn default() -> Self {
        Self::new(None, UiVisibility::BOTH)
    }
}

/// What a tool says of its effects (`annotations.readOnlyHint`,
/// `annotations.destructiveHint`), each `None` when it said nothing. These
/// are the server's hints, not guarantees; Nessa reads them only to decide
/// when to ask the person first, and so reads silence as the riskier answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ToolHints {
    read_only: Option<bool>,
    destructive: Option<bool>,
}
impl ToolHints {
    /// The hints as the tool gave them.
    pub fn new(read_only: Option<bool>, destructive: Option<bool>) -> Self {
        Self {
            read_only,
            destructive,
        }
    }
    /// Whether calling the tool may destroy something: MCP's own defaults,
    /// under which a tool is destructive unless it says it only reads
    /// (`readOnlyHint: true`) or says it destroys nothing
    /// (`destructiveHint: false`). A tool that says nothing is destructive.
    pub fn destructive(self) -> bool {
        self.read_only != Some(true) && self.destructive != Some(false)
    }
}

/// One tool as its server listed it (`tools/list`): the server and the tool's
/// own name, exactly as the server spelled it, what it declared in
/// `_meta.ui`, and what it said of its effects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedTool {
    tool: McpTool,
    ui: ToolUi,
    hints: ToolHints,
}
impl ListedTool {
    /// `tool` as listed, with what it declared in `_meta.ui` and no hints.
    pub fn new(tool: McpTool, ui: ToolUi) -> Self {
        Self {
            tool,
            ui,
            hints: ToolHints::default(),
        }
    }
    /// This tool, with the hints it gave.
    pub fn with_hints(self, hints: ToolHints) -> Self {
        Self { hints, ..self }
    }
    /// What the tool said of its effects.
    pub fn hints(&self) -> ToolHints {
        self.hints
    }
    /// The server and the tool's name on it.
    pub fn tool(&self) -> &McpTool {
        &self.tool
    }
    /// What the tool declared in `_meta.ui`.
    pub fn ui(&self) -> &ToolUi {
        &self.ui
    }
    /// What the one tool in `listed` that an observed call `call` names
    /// ([`McpTool::names`]) declared in `_meta.ui`, or `None` when no listed
    /// tool is named, or when more than one could be: a guess would draw one
    /// tool's UI for another's call.
    pub fn ui_for<'a>(listed: &'a [ListedTool], call: &McpTool) -> Option<&'a ToolUi> {
        let mut named = listed.iter().filter(|each| call.names(&each.tool));
        let only = named.next()?;
        if named.next().is_some() {
            return None;
        }
        Some(&only.ui)
    }
}
