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
import { isOverviewTerminalAgent, terminalOverviewGroup } from "../../model/terminal"
import {
  entry,
  keepShownFailures,
  listedSessions,
  sessionOf,
  type WorkspaceState,
} from "../workspace-state"

/** Chooses the session or terminal agent the overview's peek shows. */
export function selectInOverview(
  state: WorkspaceState,
  { sessionId }: { sessionId: string },
): WorkspaceState {
  const listed =
    sessionOf(state, sessionId) !== undefined ||
    (() => {
      const terminal = entry(state.terminals, sessionId)
      return terminal !== undefined && isOverviewTerminalAgent(terminal)
    })()
  if (!listed || state.overview.selected === sessionId) return state
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
 * Shows one group alone, or — `null` — every group. The chosen session or
 * terminal stays chosen when the group holds it; otherwise nothing is, and
 * the open overview chooses the first the group lists (`keepOverviewChoice`),
 * so its peek never shows an item the group leaves out. Choosing the group
 * already shown changes nothing: letting it go is choosing `null`.
 */
export function showOverviewGroup(
  state: WorkspaceState,
  { group }: { group: AgentsGroup | null },
): WorkspaceState {
  if (state.overview.group === group) return state
  const { selected } = state.overview
  const chosenSession = selected === null ? undefined : sessionOf(state, selected)
  const chosenTerminal =
    selected === null || chosenSession !== undefined
      ? undefined
      : entry(state.terminals, selected)
  const kept =
    (chosenSession !== undefined && inGroup(chosenSession, group)) ||
    (chosenTerminal !== undefined &&
      isOverviewTerminalAgent(chosenTerminal) &&
      (group === null || terminalOverviewGroup(chosenTerminal) === group))
  return keepShownFailures({
    ...state,
    overview: {
      ...state.overview,
      group,
      selected: kept ? selected : null,
    },
  })
}

/**
 * With nothing chosen that it lists — on opening, or once the chosen item
 * is gone — the open overview chooses the first it lists at `now`. The one
 * owner of which item its peek shows: the window reads that session's
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
    agentsGlance(listedSessions(state), [], {
      filter,
      group,
      now,
      looking: selected,
      terminals: Object.values(state.terminals),
    }),
  )
  if (selected !== null && order.includes(selected)) return state
  const first = order[0] ?? null
  if (first === selected) return state
  return keepShownFailures({ ...state, overview: { ...state.overview, selected: first } })
}
