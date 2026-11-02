/**
 * What the person (or an agent) does with a chat's right panel (ADR 326's
 * `panel` place): open a widget in it as a tab, bring a tab to the front,
 * close one. Each chat has its own panel (`model/chat-panel.ts`); the one
 * beside the panes is the focused chat's, so opening a widget for a chat
 * brings that chat to the front first.
 */
import type { WidgetRef } from "../../../widgets/model/widget-ref"
import { isAppPluginId } from "../../../widgets/app/model/app-ref"
import { closeInPanel, openInPanel, showInPanel } from "../../model/chat-panel"
import { paneItemKey, widgetItem } from "../../model/pane-item"
import { focusedPane } from "../../../split-panes/model/pane-layout"
import {
  entry,
  paneShowingSession,
  sessionIn,
  without,
  type WorkspaceState,
} from "../workspace-state"
import { focusPane, openSession } from "./panes"

/** The session the focused pane shows, if it shows one. */
function focusedSession(state: WorkspaceState): string | undefined {
  return state.panes ? sessionIn(focusedPane(state.panes)) : undefined
}

/**
 * The chat `widget` belongs to, in front: `origin` — the session the caller
 * says it is for — its pane focused, or shown in the focused pane's place
 * when no pane shows it; with none named, the focused chat. None when there
 * is no chat to hold it.
 */
function ownerInFront(
  state: WorkspaceState,
  origin: string | undefined,
): { state: WorkspaceState; owner: string } | null {
  if (origin === undefined) {
    const owner = focusedSession(state)
    return owner === undefined ? null : { state, owner }
  }
  const panes = state.panes
  const pane = panes && paneShowingSession(panes, origin)
  const shown = pane
    ? focusPane(state, pane.key)
    : openSession(state, { sessionId: origin })
  return focusedSession(shown) === origin ? { state: shown, owner: origin } : null
}

/**
 * Opens `widget` as a tab in its chat's panel, in front, and asks the window
 * to show the panel (`panelAsked`). A widget open there already comes to the
 * front where it is.
 */
export function openWidgetInPanel(
  state: WorkspaceState,
  { widget, origin }: { widget: WidgetRef; origin?: string },
): WorkspaceState {
  const front = ownerInFront(state, origin)
  if (!front) return state
  const { state: shown, owner } = front
  const app = isAppPluginId(widget.plugin)
  const key = paneItemKey(widgetItem(widget))
  return {
    ...shown,
    panels: {
      ...shown.panels,
      [owner]: openInPanel(entry(shown.panels, owner), widget, app),
    },
    panelled:
      app && !shown.panelled.includes(key) ? [...shown.panelled, key] : shown.panelled,
    panelAsked: shown.panelAsked + 1,
  }
}

/** Brings `widget`'s tab to the front of `sessionId`'s panel; nothing when it has none. */
export function showPanelTab(
  state: WorkspaceState,
  { sessionId, widget }: { sessionId: string; widget: WidgetRef },
): WorkspaceState {
  const panel = entry(state.panels, sessionId)
  if (!panel) return state
  const next = showInPanel(panel, widget)
  return next === panel
    ? state
    : { ...state, panels: { ...state.panels, [sessionId]: next } }
}

/** Closes `widget`'s tab in `sessionId`'s panel; the panel stays open, saying so once empty. */
export function closePanelTab(
  state: WorkspaceState,
  { sessionId, widget }: { sessionId: string; widget: WidgetRef },
): WorkspaceState {
  const panel = entry(state.panels, sessionId)
  if (!panel) return state
  const next = closeInPanel(panel, widget)
  if (next === panel) return state
  if (next) return { ...state, panels: { ...state.panels, [sessionId]: next } }
  return { ...state, panels: without(state.panels, sessionId) }
}
