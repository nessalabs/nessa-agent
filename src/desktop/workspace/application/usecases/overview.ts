/**
 * The Agents overview's own state: which session its peek shows, and what
 * it lists. Opening and leaving it is the content region's
 * (`navigation.ts`, `showContent`). Each is a named action an agent can
 * dispatch, as the person's clicks and keys do.
 */
import type { AgentsFilter } from "../../model/overview/filter"
import { keepShownFailures, sessionOf, type WorkspaceState } from "../workspace-state"

/** Chooses the session the overview's peek shows. A session not listed is not chosen. */
export function selectInOverview(
  state: WorkspaceState,
  { sessionId }: { sessionId: string },
): WorkspaceState {
  if (!sessionOf(state, sessionId) || state.overview.selected === sessionId) return state
  return keepShownFailures({
    ...state,
    overview: { ...state.overview, selected: sessionId },
  })
}

/** Chooses what the overview lists. */
export function filterOverview(
  state: WorkspaceState,
  { filter }: { filter: AgentsFilter },
): WorkspaceState {
  return keepShownFailures({ ...state, overview: { ...state.overview, filter } })
}
