/**
 * What the person (or an agent) does with panes: open a session in one, open
 * one beside another, drop a session on a pane, move, nudge, resize, close.
 * Each is a pure function of the workspace; the layout rules themselves are
 * the model's (`model/pane-layout.ts`, `model/pane-sizing.ts`).
 */
import { defaultModel, type ModelRef } from "../../model/organisation"
import {
  focusPane as focusLayoutPane,
  isHorizontal,
  locate,
  movePane as moveLayoutPane,
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
  type Side,
  type Zone,
} from "../../model/pane-layout"
import {
  canPlace,
  needsSidebarRoom,
  placeBeside,
  resizeEdge,
  type PaneEdge,
  type PaneRoom,
} from "../../model/pane-sizing"
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

function withSidebar(state: WorkspaceState, open: boolean): WorkspaceState {
  if (state.chrome.sidebarOpen === open) return state
  return { ...state, chrome: { ...state.chrome, sidebarOpen: open } }
}

export function focusPane(state: WorkspaceState, pane: PaneKey): WorkspaceState {
  if (!state.panes) return state
  return withPanes(state, focusLayoutPane(state.panes, pane))
}

/**
 * Opens a session: focuses the pane already showing it, or shows it in
 * `pane` — the focused pane when none is named.
 */
export function openSession(
  state: WorkspaceState,
  { sessionId, pane }: { sessionId: string; pane?: PaneKey },
): WorkspaceState {
  const panes = state.panes
  if (!panes || !showable(state, sessionId)) return state
  const existing = paneShowing(panes, sessionId)
  const target = existing?.key ?? pane ?? panes.focused
  if (!locate(panes, target)) return state
  const next = existing
    ? focusLayoutPane(panes, existing.key)
    : focusLayoutPane(showInPane(panes, target, sessionId), target)
  return withPanes(opened(state, sessionId), next)
}

/**
 * Opens a session beside a pane: on `side` of `target` (the focused pane, to
 * the right, by default), stacked when a column will not fit, the sidebar
 * stepping aside when that is what a column needs. Past the limits the
 * session takes the target's place — unless `replace` is false, when nothing
 * happens. A session already on screen is focused where it is.
 */
export function openBeside(
  state: WorkspaceState,
  {
    sessionId,
    target,
    side = "right",
    room,
    replace = true,
  }: {
    sessionId: string
    target?: PaneKey
    side?: Side
    room?: PaneRoom
    replace?: boolean
  },
): WorkspaceState {
  const panes = state.panes
  if (!panes || !showable(state, sessionId)) return state
  const existing = paneShowing(panes, sessionId)
  if (existing)
    return withPanes(opened(state, sessionId), focusLayoutPane(panes, existing.key))
  const beside = target ?? panes.focused
  if (!locate(panes, beside)) return state
  const place = placeBeside(panes, beside, side, room)
  if (!place) {
    if (!replace) return state
    return withPanes(
      opened(state, sessionId),
      focusLayoutPane(showInPane(panes, beside, sessionId), beside),
    )
  }
  const roomed = needsSidebarRoom(place, room) ? withSidebar(state, false) : state
  return withPanes(opened(roomed, sessionId), splitPane(panes, beside, place, sessionId))
}

/**
 * A session dropped on a pane: its middle opens it there, a side opens it
 * beside. A session already on screen is focused where it is.
 */
export function dropSession(
  state: WorkspaceState,
  {
    sessionId,
    target,
    zone,
    room,
  }: { sessionId: string; target: PaneKey; zone: Zone; room?: PaneRoom },
): WorkspaceState {
  if (zone === "center") return openSession(state, { sessionId, pane: target })
  return openBeside(state, { sessionId, target, side: zone, room })
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
  if (!home) return state
  const drafted: WorkspaceState = {
    ...state,
    drafts: {
      ...state.drafts,
      [draftId]: { id: draftId, channelId: home, model: model ?? defaultModel() },
    },
    view:
      state.view.kind === "channel" ? state.view : { kind: "channel", channelId: home },
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
  if (state.view.kind === "channel" && channelOf(state, state.view.channelId))
    return state.view.channelId
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
  const shown = paneByKey(panes, pane)?.sessionId
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
 * Moves a pane to a zone of another. A side it would not be readable on is
 * refused; a new column may ask the sidebar to step aside.
 */
export function movePane(
  state: WorkspaceState,
  {
    pane,
    target,
    zone,
    room,
  }: { pane: PaneKey; target: PaneKey; zone: Zone; room?: PaneRoom },
): WorkspaceState {
  if (!state.panes) return state
  // A side must leave both panes readable, as it must for any new pane.
  if (zone !== "center" && !canPlace(state.panes, zone, target, room, pane)) return state
  const moved = moveLayoutPane(state.panes, pane, target, zone)
  if (moved === state.panes) return state
  const roomed =
    zone !== "center" && isHorizontal(zone) && needsSidebarRoom(zone, room)
      ? withSidebar(state, false)
      : state
  return withPanes(roomed, moved)
}

export function nudgePane(
  state: WorkspaceState,
  { pane, direction }: { pane: PaneKey; direction: Direction },
): WorkspaceState {
  if (!state.panes) return state
  return withPanes(state, nudgeLayoutPane(state.panes, pane, direction))
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
