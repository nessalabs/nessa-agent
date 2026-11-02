//! `workspace.publish`: a desktop window telling the gateway what it shows
//! now, held for the connection's life in the conversation service's
//! `WorkspaceWindows`, where the person's agents read it (`read_window`).
//!
//! ```text
//! socket opens ──▶ WindowSlot for the session's principal
//! workspace.publish ──▶ admitted as `conversation.write` (a paired session is refused)
//!   ──▶ window id a UUID, revision within 2^53 - 1
//!   ──▶ wire window ──▶ WorkspaceWindow (refused `invalid_request` when not one)
//!   ──▶ WindowSlot::publish(window id, revision, window) ──▶ { revision, applied }
//! socket closes ──▶ slot dropped ──▶ window withdrawn
//! ```
use super::socket::{failure, success};
use crate::conversation::application::WindowSlot;
use crate::conversation::domain::{
    PaneContent, WidgetRef, WindowArrangement, WindowColumn, WindowPane, WindowPanel,
    WindowRefusal, WindowShowing, WorkspaceWindow,
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::product::generated::{
    WorkspaceArrangement, WorkspacePaneItem, WorkspacePaneKind, WorkspacePanel,
    WorkspacePublishParams, WorkspacePublishResult, WorkspaceShowing, WorkspaceWidget,
    WorkspaceWindow as WireWindow,
};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};

pub(super) const METHOD: &str = "workspace.publish";
/// The schema's own bound on `revision`: the largest whole number a
/// JavaScript client holds exactly.
const MAX_REVISION: u64 = (1 << 53) - 1;

/// Holds the window `frame` publishes as this connection's. `slot` is absent
/// where the gateway runs no conversations, and so no agent could read it.
pub(super) fn publish(slot: Option<&WindowSlot>, frame: RequestFrame) -> OutgoingMessage {
    let Some(slot) = slot else {
        return failure(&frame.id, "conversations_not_configured");
    };
    let Ok(params) = serde_json::from_value::<WorkspacePublishParams>(frame.params) else {
        return failure(&frame.id, "invalid_request");
    };
    // The id is a UUID in its one spelling, so one window is one key, and
    // the revision one a client can count to: past 2^53 a JavaScript number
    // cannot hold it exactly.
    let canonical =
        uuid::Uuid::try_parse(&params.window_id).is_ok_and(|id| id.to_string() == params.window_id);
    if !canonical || params.revision > MAX_REVISION {
        return failure(&frame.id, "invalid_request");
    }
    let window = match params.window.map(window).transpose() {
        Ok(window) => window,
        Err(_) => return failure(&frame.id, "invalid_request"),
    };
    let published = slot.publish(&params.window_id, params.revision, window);
    success(
        &frame.id,
        &WorkspacePublishResult {
            revision: published.revision,
            applied: published.applied,
        },
    )
}

fn window(wire: WireWindow) -> Result<WorkspaceWindow, WindowRefusal> {
    let showing = match (wire.showing, wire.widget) {
        (WorkspaceShowing::Panes, None) => WindowShowing::Panes,
        (WorkspaceShowing::Agents, None) => WindowShowing::Agents,
        (WorkspaceShowing::Widget, Some(widget)) => WindowShowing::Widget(widget_ref(widget)?),
        _ => return Err(WindowRefusal::Widget),
    };
    let columns = wire
        .columns
        .into_iter()
        .map(|column| {
            let panes = column
                .panes
                .into_iter()
                .map(|pane| WindowPane::new(pane.height_percent, pane.focused, content(pane.item)?))
                .collect::<Result<Vec<_>, _>>()?;
            WindowColumn::new(column.width_percent, panes)
        })
        .collect::<Result<Vec<_>, _>>()?;
    WorkspaceWindow::new(
        match wire.arrangement {
            WorkspaceArrangement::Columns => WindowArrangement::Columns,
            WorkspaceArrangement::Sidebar => WindowArrangement::Sidebar,
        },
        wire.sidebar,
        wire.session_list,
        showing,
        panel(wire.panel)?,
        columns,
    )
}

/// The focused chat's panel: its front is one of its tabs, or neither.
fn panel(wire: WorkspacePanel) -> Result<WindowPanel, WindowRefusal> {
    let tabs = wire
        .tabs
        .into_iter()
        .map(widget_ref)
        .collect::<Result<Vec<_>, _>>()?;
    let front = wire.front.map(widget_ref).transpose()?;
    WindowPanel::new(wire.open, tabs, front)
}

/// A pane's item names exactly what its kind says it shows.
fn content(item: WorkspacePaneItem) -> Result<PaneContent, WindowRefusal> {
    match (item.kind, item.conversation_id, item.widget) {
        (WorkspacePaneKind::Conversation, Some(id), None) => ConversationId::new(&id)
            .map(PaneContent::Conversation)
            .map_err(|_| WindowRefusal::Shape),
        (WorkspacePaneKind::Draft, None, None) => Ok(PaneContent::Draft),
        (WorkspacePaneKind::Widget, None, Some(widget)) => {
            Ok(PaneContent::Widget(widget_ref(widget)?))
        }
        _ => Err(WindowRefusal::Shape),
    }
}

fn widget_ref(widget: WorkspaceWidget) -> Result<WidgetRef, WindowRefusal> {
    WidgetRef::new(widget.plugin, widget.id)
}

#[cfg(test)]
#[path = "../../tests/product/workspace.rs"]
mod tests;
