/**
 * The Agents overview's own state: which session its peek shows, what it
 * lists, and the one group it shows alone. Opening and leaving it is the content region's
 * (`navigation.ts`, `showContent`). Each is a named action an agent can
 * dispatch, as the person's clicks and keys do.
 */
import {
  agentsGlance,
  inGroup,
  readingOrder,
  type AgentsGroup,
} from "../../model/overview/agents-glance"
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
 * Shows one group alone, or — `null` — every group. The chosen session
 * stays chosen when the group holds it; otherwise nothing is, and the open
 * overview chooses the first the group lists (`keepOverviewChoice`), so its
 * peek never shows a session the group leaves out. Choosing the group
 * already shown changes nothing: letting it go is choosing `null`.
 */
export function showOverviewGroup(
  state: WorkspaceState,
  { group }: { group: AgentsGroup | null },
): WorkspaceState {
  if (state.overview.group === group) return state
  const { selected } = state.overview
  const chosen = selected === null ? undefined : sessionOf(state, selected)
  return keepShownFailures({
    ...state,
    overview: {
      ...state.overview,
      group,
      selected: chosen && inGroup(chosen, group) ? selected : null,
    },
  })
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
  const { selected, filter, group } = state.overview
  const order = readingOrder(
    agentsGlance(listedSessions(state), [], { filter, group, now, looking: selected }),
  )
  if (selected !== null && order.includes(selected)) return state
  const first = order[0] ?? null
  if (first === selected) return state
  return keepShownFailures({ ...state, overview: { ...state.overview, selected: first } })
}
