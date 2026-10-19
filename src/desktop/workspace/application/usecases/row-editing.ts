/**
 * What a session row edits in place, past the menu that asked for it: its
 * title, renamed where it stands, and its deletion, asked first. One of each
 * at a time, held by the window so a row drawn afresh (its session moving
 * group) keeps the edit, and the question stays while its row moves.
 */
import { sessionOf, type WorkspaceState } from "../workspace-state"

/** Rename began, from the title the session has. A session not held has none to edit. */
export function renameStarted(
  state: WorkspaceState,
  { sessionId }: { sessionId: string },
): WorkspaceState {
  const session = sessionOf(state, sessionId)
  return session ? { ...state, renaming: { sessionId, draft: session.title } } : state
}

/** What is typed so far, while that session's title is the one being renamed. */
export function renameDrafted(
  state: WorkspaceState,
  { sessionId, draft }: { sessionId: string; draft: string },
): WorkspaceState {
  return state.renaming?.sessionId === sessionId
    ? { ...state, renaming: { sessionId, draft } }
    : state
}

/** The edit ended, kept or not; the command that sends it is the caller's. */
export function renameEnded(
  state: WorkspaceState,
  { sessionId }: { sessionId: string },
): WorkspaceState {
  return state.renaming?.sessionId === sessionId ? { ...state, renaming: null } : state
}

/** Ask whether to delete a session; one question at a time. */
export function deleteAsked(
  state: WorkspaceState,
  { sessionId }: { sessionId: string },
): WorkspaceState {
  return sessionOf(state, sessionId) ? { ...state, deleteAsked: sessionId } : state
}

/** The question closed, answered or not. */
export function deleteClosed(state: WorkspaceState): WorkspaceState {
  return state.deleteAsked === null ? state : { ...state, deleteAsked: null }
}
