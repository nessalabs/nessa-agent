/** Conversation documents share the window's existing split arrangement (#1077). */
import {
  draftOf,
  letGoOfDrafts,
  paneIds,
  paneShowingSession,
  withPanes,
  type WorkspaceState,
} from "../workspace-state"
import { openSession, closePane, createDraft, draftHome, focusPane } from "./panes"

/** Close a document, reusing its pane for another hidden tab where possible. */
export function closeSessionTab(
  state: WorkspaceState,
  { sessionId, draftId }: { sessionId: string; draftId: string },
): WorkspaceState {
  if (!state.sessionTabs.includes(sessionId)) return state
  const pane = state.panes ? paneShowingSession(state.panes, sessionId) : undefined
  let next = state
  if (pane) {
    const shown = new Set(paneIds(state.panes))
    const replacement = state.sessionTabs.find((id) => id !== sessionId && !shown.has(id))
    if (replacement) {
      const replaced = openSession(state, { sessionId: replacement, pane: pane.key })
      const focused = state.panes?.focused
      next =
        focused !== undefined && focused !== pane.key
          ? focusPane(replaced, focused)
          : replaced
    } else {
      next = closePane(state, { pane: pane.key, draftId })
    }
  }
  // The last unsent document also closes to a fresh home. closePane keeps a
  // lone home in place; a document close must release its text explicitly.
  if (pane && next === state) {
    const home = draftHome(state, {})
    // With no channel or model left, there is no valid home for the last pane.
    if (!home) next = withPanes(state, null)
    else {
      next = createDraft(state, { draftId, target: pane.key, ...home })
      if (next === state) return state
    }
  }
  const closed = {
    ...next,
    sessionTabs: next.sessionTabs.filter((id) => id !== sessionId),
  }
  return draftOf(closed, sessionId) ? letGoOfDrafts(closed, [sessionId]) : closed
}
