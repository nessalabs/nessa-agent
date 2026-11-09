//! What one desktop window shows: its arrangement, which side columns are
//! drawn, what fills its content region, the focused chat's right panel, and
//! its panes, left to right and
//! each column top to bottom. The window is the one authority for this; the
//! gateway keeps the latest the window published (`WorkspaceWindows`) so the
//! person's agents can read the space they share with the person.
//!
//! ```text
//! WorkspaceWindow ─▶ arrangement, sidebar, sessionList, showing, panel
//!       ├─panel─▶ open?, tabs ─▶ [ WidgetRef ], front (one of tabs, or none)
//!       └─columns─▶ [ WindowColumn { width%, panes ─▶ [ WindowPane { height%, focused, content } ] } ]
//!                                                   content ─▶ Conversation(id) | Draft | Widget(ref)
//! ```
//!
//! A window names conversations by id only. Their titles and agents are the
//! gateway's to say, read when an agent asks, so a stale title can never be
//! published here. A pane's place is said in words a person uses ("left",
//! "top right") by [`WorkspaceWindow::places`], one rule for every reader.
use nessa_protocol::conversation::domain::ConversationId;

/// The most columns, and the most panes in one column, a window may publish:
/// past what any screen draws readably, so a larger one is not a window.
pub const MAX_WINDOW_COLUMNS: usize = 8;
/// See [`MAX_WINDOW_COLUMNS`].
pub const MAX_COLUMN_PANES: usize = 8;
/// The longest widget plugin or id, in bytes.
pub const MAX_WIDGET_ID_BYTES: usize = 256;
/// The most tabs one chat's panel may publish: past a readable strip.
/// `WorkspacePanel.tabs` `maxItems` in the product schema is this number.
pub const MAX_PANEL_TABS: usize = 16;

/// How the window lays out its workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowArrangement {
    /// Sidebar, a session list, and the panes.
    Columns,
    /// Sessions listed in the sidebar beside the panes.
    Sidebar,
}

impl WindowArrangement {
    pub fn name(self) -> &'static str {
        match self {
            Self::Columns => "columns",
            Self::Sidebar => "sidebar",
        }
    }
}

/// A widget, by the plugin that draws it and its id there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WidgetRef {
    plugin: String,
    id: String,
}

impl WidgetRef {
    /// # Errors
    /// Either part empty or longer than [`MAX_WIDGET_ID_BYTES`].
    pub fn new(plugin: String, id: String) -> Result<Self, WindowRefusal> {
        for part in [&plugin, &id] {
            if part.is_empty() || part.len() > MAX_WIDGET_ID_BYTES {
                return Err(WindowRefusal::Widget);
            }
        }
        Ok(Self { plugin, id })
    }
    pub fn plugin(&self) -> &str {
        &self.plugin
    }
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// What fills the window's content region.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowShowing {
    Panes,
    /// The Agents overview, drawn over the panes.
    Agents,
    /// A widget, drawn over the panes.
    Widget(WidgetRef),
}

impl WindowShowing {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Panes => "panes",
            Self::Agents => "agents",
            Self::Widget(_) => "widget",
        }
    }
}

/// What one pane shows. Another kind of pane (an open file, say) is another
/// variant here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneContent {
    Conversation(ConversationId),
    /// A new conversation the person has not started yet.
    Draft,
    Widget(WidgetRef),
}

impl PaneContent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Conversation(_) => "conversation",
            Self::Draft => "draft",
            Self::Widget(_) => "widget",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowPane {
    height_percent: u8,
    focused: bool,
    content: PaneContent,
}

impl WindowPane {
    /// # Errors
    /// A share over 100 percent.
    pub fn new(
        height_percent: u64,
        focused: bool,
        content: PaneContent,
    ) -> Result<Self, WindowRefusal> {
        Ok(Self {
            height_percent: percent(height_percent)?,
            focused,
            content,
        })
    }
    pub fn content(&self) -> &PaneContent {
        &self.content
    }
    pub fn focused(&self) -> bool {
        self.focused
    }
    pub fn height_percent(&self) -> u8 {
        self.height_percent
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowColumn {
    width_percent: u8,
    panes: Vec<WindowPane>,
}

impl WindowColumn {
    /// # Errors
    /// A share over 100 percent, or no panes, or more than [`MAX_COLUMN_PANES`].
    pub fn new(width_percent: u64, panes: Vec<WindowPane>) -> Result<Self, WindowRefusal> {
        if panes.is_empty() || panes.len() > MAX_COLUMN_PANES {
            return Err(WindowRefusal::Shape);
        }
        Ok(Self {
            width_percent: percent(width_percent)?,
            panes,
        })
    }
    pub fn width_percent(&self) -> u8 {
        self.width_percent
    }
    pub fn panes(&self) -> &[WindowPane] {
        &self.panes
    }
}

/// The focused chat's right panel, as the window published it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowPanel {
    open: bool,
    tabs: Vec<WidgetRef>,
    front: Option<WidgetRef>,
}

impl WindowPanel {
    /// # Errors
    /// More than [`MAX_PANEL_TABS`] tabs, a repeated tab, or a front that is
    /// not exactly one of `tabs` (absent when there are none).
    pub fn new(
        open: bool,
        tabs: Vec<WidgetRef>,
        front: Option<WidgetRef>,
    ) -> Result<Self, WindowRefusal> {
        if tabs.len() > MAX_PANEL_TABS {
            return Err(WindowRefusal::Panel);
        }
        // Each tab is compared with the ones before it. The length check above
        // keeps that scan within MAX_PANEL_TABS.
        if tabs
            .iter()
            .enumerate()
            .any(|(index, tab)| tabs[..index].contains(tab))
        {
            return Err(WindowRefusal::Panel);
        }
        let front_is_a_tab = match &front {
            None => tabs.is_empty(),
            Some(front) => tabs.contains(front),
        };
        if !front_is_a_tab {
            return Err(WindowRefusal::Panel);
        }
        Ok(Self { open, tabs, front })
    }

    /// Whether the panel is drawn.
    pub fn open(&self) -> bool {
        self.open
    }

    /// The focused chat's tabs, in the order they were opened.
    pub fn tabs(&self) -> &[WidgetRef] {
        &self.tabs
    }

    /// The tab in front: one of [`Self::tabs`], or none when there are none.
    pub fn front(&self) -> Option<&WidgetRef> {
        self.front.as_ref()
    }
}

/// Why a published window is not one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowRefusal {
    /// No columns, an empty column, or more columns or panes than a window has.
    Shape,
    /// A share of the room over 100 percent.
    Share,
    /// More than one pane focused.
    Focus,
    /// A widget with an empty or oversized plugin or id.
    Widget,
    /// A panel with too many tabs, a repeated tab, or a front that is not one of them.
    Panel,
}

/// One window, as it published itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceWindow {
    arrangement: WindowArrangement,
    sidebar: bool,
    session_list: bool,
    showing: WindowShowing,
    panel: WindowPanel,
    columns: Vec<WindowColumn>,
}

/// Where one pane sits in its window, by position and in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanePlace<'a> {
    /// 1-based, in reading order: down each column, then across.
    pub number: usize,
    /// 1-based, left to right.
    pub column: usize,
    /// 1-based, top to bottom.
    pub row: usize,
    /// Where a person would say it is: "left", "right", "middle", "top left",
    /// "bottom right", or "full" for a window of one pane.
    pub position: String,
    pub column_width_percent: u8,
    pub pane: &'a WindowPane,
}

impl WorkspaceWindow {
    /// # Errors
    /// See [`WindowRefusal`].
    pub fn new(
        arrangement: WindowArrangement,
        sidebar: bool,
        session_list: bool,
        showing: WindowShowing,
        panel: WindowPanel,
        columns: Vec<WindowColumn>,
    ) -> Result<Self, WindowRefusal> {
        if columns.is_empty() || columns.len() > MAX_WINDOW_COLUMNS {
            return Err(WindowRefusal::Shape);
        }
        let focused = columns
            .iter()
            .flat_map(|column| &column.panes)
            .filter(|pane| pane.focused)
            .count();
        if focused > 1 {
            return Err(WindowRefusal::Focus);
        }
        Ok(Self {
            arrangement,
            sidebar,
            session_list,
            showing,
            panel,
            columns,
        })
    }
    pub fn arrangement(&self) -> WindowArrangement {
        self.arrangement
    }
    /// Whether the sidebar is drawn.
    pub fn sidebar(&self) -> bool {
        self.sidebar
    }
    /// Whether the session list column is drawn.
    pub fn session_list(&self) -> bool {
        self.session_list
    }
    pub fn showing(&self) -> &WindowShowing {
        &self.showing
    }
    /// The focused chat's right panel.
    pub fn panel(&self) -> &WindowPanel {
        &self.panel
    }
    pub fn columns(&self) -> &[WindowColumn] {
        &self.columns
    }

    /// Every pane with its place, in reading order: down each column, then
    /// across.
    pub fn places(&self) -> Vec<PanePlace<'_>> {
        let columns = self.columns.len();
        let mut number = 0;
        let mut places = Vec::new();
        for (column_index, column) in self.columns.iter().enumerate() {
            let rows = column.panes.len();
            for (row_index, pane) in column.panes.iter().enumerate() {
                number += 1;
                places.push(PanePlace {
                    number,
                    column: column_index + 1,
                    row: row_index + 1,
                    position: position(column_index, columns, row_index, rows),
                    column_width_percent: column.width_percent,
                    pane,
                });
            }
        }
        places
    }
}

/// A place in words: across, then down, joined as "top left" reads.
fn position(column: usize, columns: usize, row: usize, rows: usize) -> String {
    let across = side(column, columns, "left", "right");
    let down = side(row, rows, "top", "bottom");
    match (down, across) {
        (None, None) => "full".into(),
        (Some(down), None) => down.into(),
        (None, Some(across)) => across.into(),
        (Some(down), Some(across)) => format!("{down} {across}"),
    }
}

/// `index` of `count` as first, middle or last; `None` when it is the only one.
/// Past three, the inner ones are "middle" too: the number says which.
fn side(
    index: usize,
    count: usize,
    first: &'static str,
    last: &'static str,
) -> Option<&'static str> {
    if count <= 1 {
        None
    } else if index == 0 {
        Some(first)
    } else if index + 1 == count {
        Some(last)
    } else {
        Some("middle")
    }
}

fn percent(value: u64) -> Result<u8, WindowRefusal> {
    u8::try_from(value)
        .ok()
        .filter(|value| *value <= 100)
        .ok_or(WindowRefusal::Share)
}

#[cfg(test)]
#[path = "../../../../tests/conversation/workspace_window.rs"]
mod tests;
