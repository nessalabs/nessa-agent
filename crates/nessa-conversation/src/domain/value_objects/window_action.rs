//! One change a person's agent, or a person's CLI, asks a desktop window to
//! make to what it shows (`arrange_window`, `close_pane`,
//! `workspace.arrange`, `workspace.closePane`).
//!
//! The window is the authority for its layout, so an action is a request the
//! window carries out by its own rules, the ones a click or a shortcut
//! follows there: opening a conversation already on screen focuses it,
//! opening one beside where the room will not hold it shows it in the focused
//! pane, and so on. Nothing here decides a layout.
//!
//! Each action has a severity, which decides the tool it belongs to and the
//! hints that tool carries, so an agent's own approval mode treats it the way
//! it treats any tool of that kind.
use super::WidgetRef;
use nessa_protocol::conversation::domain::ConversationId;

/// A side column or the right panel, opened or closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowSide {
    Sidebar,
    SessionList,
    /// The focused chat's right panel.
    Panel,
}

/// What fills the content region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowView {
    Panes,
    /// The Agents overview, over the panes.
    Agents,
}

/// Where a conversation opens, relative to the focused pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenPlace {
    /// In the focused pane, in place of what it shows.
    Here,
    /// Beside the focused pane: to the right, else below, as ⌘-click asks.
    Beside,
    Right,
    Below,
}

/// How much an action changes, for the tool and hints it is offered under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowActionSeverity {
    /// Changes what is on screen; nothing is lost and it is undone the same way.
    Arrange,
    /// Takes something off the screen the person may have been using.
    Destructive,
}

/// One change to a window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowAction {
    Show {
        side: WindowSide,
        open: bool,
    },
    /// Bring this tab of the focused chat's panel to the front, opening the panel.
    PanelTab(WidgetRef),
    View(WindowView),
    Open {
        conversation: ConversationId,
        place: OpenPlace,
    },
    Focus(ConversationId),
    Close(ConversationId),
}

impl WindowAction {
    pub fn severity(&self) -> WindowActionSeverity {
        match self {
            Self::Close(_) => WindowActionSeverity::Destructive,
            Self::Show { .. }
            | Self::PanelTab(_)
            | Self::View(_)
            | Self::Open { .. }
            | Self::Focus(_) => WindowActionSeverity::Arrange,
        }
    }

    /// The conversation the action names, which the person must be able to read.
    pub fn conversation(&self) -> Option<&ConversationId> {
        match self {
            Self::Open { conversation, .. }
            | Self::Focus(conversation)
            | Self::Close(conversation) => Some(conversation),
            Self::Show { .. } | Self::PanelTab(_) | Self::View(_) => None,
        }
    }

    /// The action's name, as audits and the protocol say it.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Show {
                side: WindowSide::Sidebar,
                ..
            } => "sidebar",
            Self::Show {
                side: WindowSide::SessionList,
                ..
            } => "session_list",
            Self::Show {
                side: WindowSide::Panel,
                ..
            } => "panel",
            Self::PanelTab(_) => "panel_tab",
            Self::View(_) => "show",
            Self::Open { .. } => "open_conversation",
            Self::Focus(_) => "focus_pane",
            Self::Close(_) => "close_pane",
        }
    }
}

impl WindowView {
    pub fn name(self) -> &'static str {
        match self {
            Self::Panes => "panes",
            Self::Agents => "agents",
        }
    }
}

impl OpenPlace {
    pub fn name(self) -> &'static str {
        match self {
            Self::Here => "here",
            Self::Beside => "beside",
            Self::Right => "right",
            Self::Below => "below",
        }
    }
}
