/** Conversation documents share the window's existing split arrangement (#1077). */
import {
  draftOf,
  without,
  paneIds,
  paneShowingSession,
  type WorkspaceState,
} from "../workspace-state"
import { openSession, closePane, createDraft } from "./panes"

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
    next = replacement
      ? openSession(state, { sessionId: replacement, pane: pane.key })
      : closePane(state, { pane: pane.key, draftId })
  }
  // The last unsent document also closes to a fresh home. closePane keeps a
  // lone home in place; a document close must release its text explicitly.
  if (pane && next === state) {
    next = createDraft(state, { draftId, target: pane.key })
    if (next === state) return state
  }
  const closed = {
    ...next,
    sessionTabs: next.sessionTabs.filter((id) => id !== sessionId),
  }
  return draftOf(closed, sessionId)
    ? {
        ...closed,
        drafts: without(closed.drafts, sessionId),
        composerText: without(closed.composerText, sessionId),
        accessChoices: without(closed.accessChoices, sessionId),
      }
    : closed
}
