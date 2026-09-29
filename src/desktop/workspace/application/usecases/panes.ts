/**
 * What the person (or an agent) does with panes: open a session in one, open
 * one beside another, drop a session on a pane, move, nudge, resize, close.
 * Each is a pure function of the workspace; the layout rules themselves are
 * the split panes' model (`split-panes/model/pane-layout.ts`,
 * `split-panes/model/pane-sizing.ts`, `split-panes/model/drop.ts`).
 */
import { fitted as fittedColumn } from "../../../model/side-column"
import { defaultModel, type ModelRef } from "../../model/workspace-index"
import {
  focusPane as focusLayoutPane,
  locate,
  nudgePane as nudgeLayoutPane,
  paneByKey,
  paneCount,
  paneShowing,
  removePane,
  showInPane,
  splitPane,
  equalizePanes as equalizeLayout,
  type Direction,
  type PaneKey,
  type PaneLayout,
  type Side,
  type Zone,
} from "../../../split-panes/model/pane-layout"
import { dropOutcome } from "../../../split-panes/model/drop"
import {
  arrange,
  fitted,
  resizeEdge,
  type Arranged,
  type PaneEdge,
  type PaneRoom,
} from "../../../split-panes/model/pane-sizing"
import {
  channelOf,
  draftOf,
  focusedChannel,
  modelForNextTurn,
  sessionOf,
  toggled,
  withPanes,
  type WorkspaceState,
} from "../workspace-state"

/** Whether a session id names something a pane can show: a listed session or a draft. */
function showable(state: WorkspaceState, sessionId: string): boolean {
  return (
    sessionOf(state, sessionId) !== undefined || draftOf(state, sessionId) !== undefined
  )
}

/** Discloses the channel a session is in, in the sidebar that lists sessions inline. */
function discloseChannel(state: WorkspaceState, sessionId: string): WorkspaceState {
  const channelId = sessionOf(state, sessionId)?.channelId
  if (!channelId) return state
  const expandedChannels = toggled(state.tree.expandedChannels, channelId, true)
  if (expandedChannels === state.tree.expandedChannels) return state
  return { ...state, tree: { ...state.tree, expandedChannels } }
}

/**
 * A session opened: its channel is disclosed. It is marked read by the
 * effect that owns "shown means read" (`adapters/store/effects.ts`), whoever
 * showed it and however.
 */
function opened(state: WorkspaceState, sessionId: string): WorkspaceState {
  return discloseChannel(state, sessionId)
}

/**
 * A change of layout the room allowed (`arrange`): applied, the sidebar
 * folded for room when taking the spare room it gives up (`PaneRoom.spare`)
 * is what made it fit — a fold the window lifts once there is room again,
 * not the person's choice.
 */
function arranged(
  state: WorkspaceState,
  { layout, takesSpare }: Arranged,
): WorkspaceState {
  const sidebar = takesSpare
    ? fittedColumn(state.chrome.sidebar, false)
    : state.chrome.sidebar
  const folded =
    sidebar === state.chrome.sidebar
      ? state
      : { ...state, chrome: { ...state.chrome, sidebar } }
  return withPanes(folded, layout)
}

export function focusPane(state: WorkspaceState, pane: PaneKey): WorkspaceState {
  if (!state.panes) return state
  return withPanes(state, focusLayoutPane(state.panes, pane))
}

/**
 * Opens a session: shows it in `pane` — the focused pane when none is named
 * — or, shown already, focuses the pane showing it (`showInPane`, which
 * never shows a session twice).
 */
export function openSession(
  state: WorkspaceState,
  { sessionId, pane }: { sessionId: string; pane?: PaneKey },
): WorkspaceState {
  const panes = state.panes
  if (!panes || !showable(state, sessionId)) return state
  const next = showInPane(panes, pane ?? panes.focused, sessionId)
  if (next === panes && !paneShowing(panes, sessionId)) return state
  return withPanes(opened(state, sessionId), next)
}

/** Where a new pane showing `sessionId` goes on `side` of `target`, if the room allows. */
function besideIn(
  panes: PaneLayout,
  target: PaneKey,
  side: Side,
  sessionId: string,
  room: PaneRoom | undefined,
): Arranged | null {
  return arrange(panes, splitPane(panes, target, side, sessionId), room)
}

/**
 * Opens a session beside a pane: on `side` of `target` (the focused pane
 * when none is named), if the room allows (`arrange`). A side named is that
 * side or nothing; with none named — "beside", as ⌘-click asks — to the
 * right, else below. Where it will not fit, the session takes the target's
 * place — unless `replace` is false, when nothing happens. A session already
 * on screen is focused where it is.
 */
export function openBeside(
  state: WorkspaceState,
  {
    sessionId,
    target,
    side,
    room,
    replace = true,
  }: {
    sessionId: string
    target?: PaneKey
    side?: Side
    room: PaneRoom | undefined
    replace?: boolean
  },
): WorkspaceState {
  const panes = state.panes
  if (!panes || !showable(state, sessionId)) return state
  const beside = target ?? panes.focused
  if (paneShowing(panes, sessionId) || !locate(panes, beside))
    return openSession(state, { sessionId })
  const placed = side
    ? besideIn(panes, beside, side, sessionId, room)
    : (besideIn(panes, beside, "right", sessionId, room) ??
      besideIn(panes, beside, "bottom", sessionId, room))
  if (placed) return arranged(opened(state, sessionId), placed)
  return replace ? openSession(state, { sessionId, pane: beside }) : state
}

/**
 * Whether a session could open beside `target` without taking its place: on
 * `side`, or on either when none is named. What the menus and the switcher
 * ask before they offer it, so they never promise what will not happen.
 */
export function canOpenBeside(
  state: WorkspaceState,
  { target, side, room }: { target?: PaneKey; side?: Side; room: PaneRoom | undefined },
): boolean {
  const panes = state.panes
  if (!panes) return false
  const beside = target ?? panes.focused
  // A stand-in id no pane shows, so only the room decides.
  const probe = "\u0000beside"
  const sides: readonly Side[] = side ? [side] : ["right", "bottom"]
  return sides.some((each) => besideIn(panes, beside, each, probe, room) !== null)
}

/**
 * A session dropped on a pane (`dropOutcome`, the layout the drag previewed):
 * its middle shows it there, a side beside it there if the room allows, or
 * not at all. A session already on screen is focused where it is.
 */
export function dropSession(
  state: WorkspaceState,
  {
    sessionId,
    target,
    zone,
    room,
  }: { sessionId: string; target: PaneKey; zone: Zone; room: PaneRoom | undefined },
): WorkspaceState {
  if (!state.panes || !showable(state, sessionId)) return state
  const outcome = dropOutcome(
    state.panes,
    { kind: "item", item: sessionId },
    target,
    zone,
    room,
  )
  return outcome ? arranged(opened(state, sessionId), outcome) : state
}

/**
 * Starts a new session as a draft: in `target` (the focused pane), or beside
 * it when `beside` names a side. A split asked for when the workspace is full
 * does nothing, rather than putting a blank session over a conversation.
 */
export function createDraft(
  state: WorkspaceState,
  {
    draftId,
    channelId,
    model,
    beside,
    target,
    room,
  }: {
    draftId: string
    channelId?: string
    model?: ModelRef
    beside?: Side
    target?: PaneKey
    room?: PaneRoom
  },
): WorkspaceState {
  const panes = state.panes
  if (!panes || showable(state, draftId)) return state
  const home = channelId && channelOf(state, channelId) ? channelId : draftChannel(state)
  // A new session runs on a model; with none to start on, none starts.
  const startsOn = model ?? defaultModel()
  if (!home || !startsOn) return state
  const drafted: WorkspaceState = {
    ...state,
    drafts: {
      ...state.drafts,
      [draftId]: { id: draftId, channelId: home, model: startsOn },
    },
  }
  const placed = beside
    ? openBeside(drafted, {
        sessionId: draftId,
        target,
        side: beside,
        room,
        replace: false,
      })
    : openSession(drafted, { sessionId: draftId, pane: target })
  // A draft no pane took is not kept.
  return placed === drafted ? state : placed
}

/** The channel a new session starts in: the one being looked at, else the focused session's. */
function draftChannel(state: WorkspaceState): string | undefined {
  if (channelOf(state, state.view.channelId)) return state.view.channelId
  return focusedChannel(state) ?? state.channels[0]?.id
}

/**
 * Closes a pane; its neighbour takes the room. The last pane cannot close:
 * showing a conversation, it goes back to a new session's home in the same
 * channel, under `draftId`; showing a home already, nothing changes.
 */
export function closePane(
  state: WorkspaceState,
  { pane, draftId }: { pane: PaneKey; draftId?: string },
): WorkspaceState {
  const panes = state.panes
  if (!panes || !locate(panes, pane)) return state
  if (paneCount(panes) > 1) return withPanes(state, removePane(panes, pane))
  const shown = paneByKey(panes, pane)?.item
  const session = shown ? sessionOf(state, shown) : undefined
  if (!session || !draftId) return state
  return createDraft(state, {
    draftId,
    channelId: session.channelId,
    model: modelForNextTurn(state, session.id) ?? session.model,
    target: pane,
  })
}

/**
 * Moves a pane to a zone of another (`dropOutcome`, the layout the drag
 * previewed): the middle swaps the two; a side takes it there if the room
 * allows (`arrange`), and not at all if not.
 */
export function movePane(
  state: WorkspaceState,
  {
    pane,
    target,
    zone,
    room,
  }: { pane: PaneKey; target: PaneKey; zone: Zone; room: PaneRoom | undefined },
): WorkspaceState {
  if (!state.panes) return state
  const outcome = dropOutcome(state.panes, { kind: "pane", pane }, target, zone, room)
  return outcome ? arranged(state, outcome) : state
}

/**
 * The keyboard's move (`nudgePane` in the model), held to the same rule: a
 * swap is always taken; a pane stepping out into a column of its own only
 * where the room allows.
 */
export function nudgePane(
  state: WorkspaceState,
  {
    pane,
    direction,
    room,
  }: { pane: PaneKey; direction: Direction; room: PaneRoom | undefined },
): WorkspaceState {
  if (!state.panes) return state
  const placed = arrange(state.panes, nudgeLayoutPane(state.panes, pane, direction), room)
  return placed ? arranged(state, placed) : state
}

/** Whether a pane can be moved that way: what the Move items ask before they offer it. */
export function canNudge(
  state: WorkspaceState,
  {
    pane,
    direction,
    room,
  }: { pane: PaneKey; direction: Direction; room: PaneRoom | undefined },
): boolean {
  if (!state.panes) return false
  return (
    arrange(state.panes, nudgeLayoutPane(state.panes, pane, direction), room) !== null
  )
}

export function resizePanes(
  state: WorkspaceState,
  { edge, fraction, pair }: { edge: PaneEdge; fraction: number; pair: number },
): WorkspaceState {
  if (!state.panes) return state
  return withPanes(state, resizeEdge(state.panes, edge, fraction, pair))
}

export function equalizePanes(state: WorkspaceState): WorkspaceState {
  if (!state.panes) return state
  return withPanes(state, equalizeLayout(state.panes))
}

/**
 * The panes' room changed — the window resized, a side column opened or
 * folded: every pane is held to the readable size again, its share
 * rebalanced where it must be (`fitted`). Where even that cannot fit them,
 * the layout stays as it is, and each pane's composer takes its compact form
 * rather than be clipped (`panes.css`).
 */
export function fitPanes(
  state: WorkspaceState,
  { room }: { room: { width: number; height: number } },
): WorkspaceState {
  if (!state.panes) return state
  const fit = fitted(state.panes, room)
  return fit ? withPanes(state, fit) : state
}
