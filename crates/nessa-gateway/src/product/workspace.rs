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
//!
//! And the person's own changes to a window, for a CLI (`nessa call`) or an
//! MCP client (`nessa mcp`), through the one path agents' window tools take
//! (`ConversationService::control_window`):
//!
//! ```text
//! workspace.arrange / workspace.closePane ──▶ admitted as `conversation.write`
//!   ──▶ params ──▶ WindowAction (each kind with exactly its fields, else invalid_request)
//!   ──▶ control_window(caller, Session, window named or newest, action)
//!   ──▶ { windowId, revision, changed } | its code
//! the window's connection ◀── workspace.command { windowId, commandId, action }
//! workspace.answer ──▶ WindowSlot::answer(commandId, answer) ──▶ { settled }
//! ```
use super::socket::{failure, success};
use super::state::ProductRouteState;
use crate::conversation::application::{
    WindowAnswer, WindowCommand, WindowControlVia, WindowSlot, WindowTarget,
};
use crate::conversation::domain::{
    OpenPlace, PaneContent, WidgetRef, WindowAction, WindowArrangement, WindowColumn, WindowPane,
    WindowPanel, WindowRefusal, WindowShowing, WindowSide, WindowView, WorkspaceWindow,
};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::product::generated::{
    WorkspaceAction, WorkspaceActionKind, WorkspaceAnswerOutcome, WorkspaceAnswerParams,
    WorkspaceAnswerResult, WorkspaceArrangeAction, WorkspaceArrangeKind, WorkspaceArrangeParams,
    WorkspaceArrangement, WorkspaceClosePaneParams, WorkspaceCommand, WorkspaceControlResult,
    WorkspaceOpenPlace, WorkspacePaneItem, WorkspacePaneKind, WorkspacePanel,
    WorkspacePublishParams, WorkspacePublishResult, WorkspaceShowing, WorkspaceView,
    WorkspaceWidget, WorkspaceWindow as WireWindow,
};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};

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

/// A window's answer to a command sent to it on this connection.
pub(super) fn answer(slot: Option<&WindowSlot>, frame: RequestFrame) -> OutgoingMessage {
    let Some(slot) = slot else {
        return failure(&frame.id, "conversations_not_configured");
    };
    if holds_null(&frame.params) {
        return failure(&frame.id, "invalid_request");
    }
    let Ok(params) = serde_json::from_value::<WorkspaceAnswerParams>(frame.params) else {
        return failure(&frame.id, "invalid_request");
    };
    if !canonical(&params.window_id) || params.command_id > MAX_REVISION {
        return failure(&frame.id, "invalid_request");
    }
    let answer = match (params.outcome, params.revision, params.code) {
        (WorkspaceAnswerOutcome::Applied, Some(revision), None) if revision <= MAX_REVISION => {
            WindowAnswer::Applied { revision }
        }
        (WorkspaceAnswerOutcome::Unchanged, Some(revision), None) if revision <= MAX_REVISION => {
            WindowAnswer::Unchanged { revision }
        }
        (WorkspaceAnswerOutcome::Refused, None, Some(code)) => WindowAnswer::Refused {
            code: code.as_str().to_owned(),
        },
        _ => return failure(&frame.id, "invalid_request"),
    };
    let settled = slot.answer(&params.window_id, params.command_id, answer);
    success(&frame.id, &WorkspaceAnswerResult { settled })
}

/// `workspace.arrange` and `workspace.closePane`: the person's own change to
/// one of their windows.
pub(super) async fn control(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(service) = state.conversation.service.as_ref() else {
        return failure(&frame.id, "conversations_not_configured");
    };
    // The schema allows no null: an optional field is absent, never null.
    if holds_null(&frame.params) {
        return failure(&frame.id, "invalid_request");
    }
    let decoded = match frame.method.as_str() {
        "workspace.arrange" => serde_json::from_value::<WorkspaceArrangeParams>(frame.params)
            .ok()
            .and_then(|params| Some((params.window_id, arrange_action(params.action)?))),
        "workspace.closePane" => serde_json::from_value::<WorkspaceClosePaneParams>(frame.params)
            .ok()
            .and_then(|params| {
                let conversation = ConversationId::new(&params.conversation_id).ok()?;
                Some((params.window_id, WindowAction::Close(conversation)))
            }),
        _ => return failure(&frame.id, "unknown_method"),
    };
    let Some((window_id, action)) = decoded else {
        return failure(&frame.id, "invalid_request");
    };
    let target = match window_id.as_deref() {
        Some(id) if canonical(id) => WindowTarget::Named(id),
        Some(_) => return failure(&frame.id, "invalid_request"),
        None => WindowTarget::Newest,
    };
    let caller = super::conversation::caller(session, frame.id.clone());
    match service
        .control_window(&caller, WindowControlVia::Session, target, action)
        .await
    {
        Ok(done) => success(
            &frame.id,
            &WorkspaceControlResult {
                window_id: done.window_id,
                revision: done.revision,
                changed: done.changed,
            },
        ),
        Err(error) => failure(&frame.id, &error.code()),
    }
}

/// The wire's `workspace.command` for `command`.
pub(super) fn command(command: &WindowCommand) -> WorkspaceCommand {
    WorkspaceCommand {
        window_id: command.window_id.clone(),
        command_id: command.id,
        action: wire_action(&command.action),
    }
}

/// Whether `value` holds a JSON null anywhere, which serde would read as an
/// absent optional field.
fn holds_null(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::Array(items) => items.iter().any(holds_null),
        serde_json::Value::Object(fields) => fields.values().any(holds_null),
        _ => false,
    }
}

fn canonical(id: &str) -> bool {
    uuid::Uuid::try_parse(id).is_ok_and(|parsed| parsed.to_string() == id)
}

/// The change `workspace.arrange` names: each kind with exactly its fields.
fn arrange_action(wire: WorkspaceArrangeAction) -> Option<WindowAction> {
    let conversation = |id: Option<String>| id.and_then(|id| ConversationId::new(&id).ok());
    let side = |side| WindowAction::Show { side, open: false };
    match (
        wire.kind,
        wire.open,
        wire.view,
        wire.widget,
        wire.conversation_id,
        wire.place,
    ) {
        (WorkspaceArrangeKind::Sidebar, Some(open), None, None, None, None) => {
            Some(shown(side(WindowSide::Sidebar), open))
        }
        (WorkspaceArrangeKind::SessionList, Some(open), None, None, None, None) => {
            Some(shown(side(WindowSide::SessionList), open))
        }
        (WorkspaceArrangeKind::Panel, Some(open), None, None, None, None) => {
            Some(shown(side(WindowSide::Panel), open))
        }
        (WorkspaceArrangeKind::PanelTab, None, None, Some(widget), None, None) => {
            widget_ref(widget).ok().map(WindowAction::PanelTab)
        }
        (WorkspaceArrangeKind::Show, None, Some(view), None, None, None) => {
            Some(WindowAction::View(match view {
                WorkspaceView::Panes => WindowView::Panes,
                WorkspaceView::Agents => WindowView::Agents,
            }))
        }
        (WorkspaceArrangeKind::OpenConversation, None, None, None, id @ Some(_), place) => {
            Some(WindowAction::Open {
                conversation: conversation(id)?,
                place: match place {
                    None | Some(WorkspaceOpenPlace::Beside) => OpenPlace::Beside,
                    Some(WorkspaceOpenPlace::Right) => OpenPlace::Right,
                    Some(WorkspaceOpenPlace::Below) => OpenPlace::Below,
                    Some(WorkspaceOpenPlace::Here) => OpenPlace::Here,
                },
            })
        }
        (WorkspaceArrangeKind::FocusPane, None, None, None, id @ Some(_), None) => {
            Some(WindowAction::Focus(conversation(id)?))
        }
        _ => None,
    }
}

fn shown(action: WindowAction, open: bool) -> WindowAction {
    match action {
        WindowAction::Show { side, .. } => WindowAction::Show { side, open },
        other => other,
    }
}

/// `action` in the protocol's words, as the window is sent it.
fn wire_action(action: &WindowAction) -> WorkspaceAction {
    let mut wire = WorkspaceAction {
        kind: WorkspaceActionKind::Sidebar,
        open: None,
        view: None,
        widget: None,
        conversation_id: None,
        place: None,
    };
    match action {
        WindowAction::Show { side, open } => {
            wire.kind = match side {
                WindowSide::Sidebar => WorkspaceActionKind::Sidebar,
                WindowSide::SessionList => WorkspaceActionKind::SessionList,
                WindowSide::Panel => WorkspaceActionKind::Panel,
            };
            wire.open = Some(*open);
        }
        WindowAction::PanelTab(widget) => {
            wire.kind = WorkspaceActionKind::PanelTab;
            wire.widget = Some(WorkspaceWidget {
                plugin: widget.plugin().to_owned(),
                id: widget.id().to_owned(),
            });
        }
        WindowAction::View(view) => {
            wire.kind = WorkspaceActionKind::Show;
            wire.view = Some(match view {
                WindowView::Panes => WorkspaceView::Panes,
                WindowView::Agents => WorkspaceView::Agents,
            });
        }
        WindowAction::Open {
            conversation,
            place,
        } => {
            wire.kind = WorkspaceActionKind::OpenConversation;
            wire.conversation_id = Some(conversation.to_string());
            wire.place = Some(match place {
                OpenPlace::Beside => WorkspaceOpenPlace::Beside,
                OpenPlace::Right => WorkspaceOpenPlace::Right,
                OpenPlace::Below => WorkspaceOpenPlace::Below,
                OpenPlace::Here => WorkspaceOpenPlace::Here,
            });
        }
        WindowAction::Focus(conversation) => {
            wire.kind = WorkspaceActionKind::FocusPane;
            wire.conversation_id = Some(conversation.to_string());
        }
        WindowAction::Close(conversation) => {
            wire.kind = WorkspaceActionKind::ClosePane;
            wire.conversation_id = Some(conversation.to_string());
        }
    }
    wire
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
        (WorkspacePaneKind::Terminal, None, None) => Ok(PaneContent::Terminal),
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
