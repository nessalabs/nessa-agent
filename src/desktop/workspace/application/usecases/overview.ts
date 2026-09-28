/**
 * The Agents overview's own state: which session its peek shows, and what
 * it lists. Opening and leaving it is the content region's
 * (`navigation.ts`, `showContent`). Each is a named action an agent can
 * dispatch, as the person's clicks and keys do.
 */
import { agentsGlance, readingOrder } from "../../model/overview/agents-glance"
import type { AgentsFilter } from "../../model/overview/filter"
import {
  keepShownFailures,
  listedSessions,
  sessionOf,
  type WorkspaceState,
} from "../workspace-state"

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

/**
 * With nothing chosen that it lists — on opening, or once the chosen session
 * is gone — the open overview chooses the first it lists at `now`. The one
 * owner of which session its peek shows: the window reads that session's
 * conversation because this chose it (`overviewShownIds`), and the view
 * shows what is chosen. Nothing listed, nothing is chosen.
 */
export function keepOverviewChoice(
  state: WorkspaceState,
  { now }: { now: number },
): WorkspaceState {
  if (state.content !== "agents") return state
  const { selected, filter } = state.overview
  const order = readingOrder(
    agentsGlance(listedSessions(state), [], { filter, now, looking: selected }),
  )
  if (selected !== null && order.includes(selected)) return state
  const first = order[0] ?? null
  if (first === selected) return state
  return keepShownFailures({ ...state, overview: { ...state.overview, selected: first } })
}
