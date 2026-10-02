/**
 * What the person (or an agent) does with panes: open a session or a widget
 * in one, open one beside another, drop a session on a pane, move, nudge,
 * resize, close. Each is a pure function of the workspace; the layout rules
 * themselves are the split panes' model (`split-panes/model/pane-layout.ts`,
 * `split-panes/model/pane-sizing.ts`, `split-panes/model/drop.ts`), which
 * holds each pane's item as its key (`model/pane-item.ts`). What a pane can
 * show, and what opening it does, is decided here per kind of item.
 */
import { fitted as fittedColumn } from "../../../model/side-column"
import { defaultModel, type ModelRef } from "../../model/workspace-index"
import {
  focusPane as focusLayoutPane,
  locate,
  nudgePane as nudgeLayoutPane,
  paneByKey,
  paneCount,
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
  paneItemKey,
  roomProbeKey,
  sessionItem,
  widgetItem,
  type PaneItem,
  type PaneItemKey,
} from "../../model/pane-item"
import type { WidgetRef } from "../../../widgets/model/widget-ref"
import {
  channelOf,
  draftOf,
  focusedChannel,
  itemIn,
  modelForNextTurn,
  paneShowingItem,
  paneShowingSession,
  sessionOf,
  toggled,
  withPanes,
  type WorkspaceState,
} from "../workspace-state"

/**
 * Whether a pane can show an item: a session the window has, listed or a
 * draft; any widget, which the widgets' host draws as it can — ready, not
 * read yet, gone, or not to be shown (ADR 326).
 */
function showable(state: WorkspaceState, item: PaneItem): boolean {
  if (item.kind === "widget") return true
  return (
    sessionOf(state, item.sessionId) !== undefined ||
    draftOf(state, item.sessionId) !== undefined
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
 * An item opened. A session's channel is disclosed; it is marked read by the
 * effect that owns "shown means read" (`adapters/store/effects.ts`), whoever
 * showed it and however. A widget changes nothing beside its pane.
 */
function opened(state: WorkspaceState, item: PaneItem): WorkspaceState {
  return item.kind === "session" ? discloseChannel(state, item.sessionId) : state
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
 * Shows an item in `pane` — the focused pane when none is named — or, shown
 * already, focuses the pane showing it (`showInPane`, which never shows an
 * item twice).
 */
function openItem(
  state: WorkspaceState,
  { item, pane }: { item: PaneItem; pane?: PaneKey },
): WorkspaceState {
  const panes = state.panes
  if (!panes || !showable(state, item)) return state
  const next = showInPane(panes, pane ?? panes.focused, paneItemKey(item))
  if (next === panes && !paneShowingItem(panes, item)) return state
  return withPanes(opened(state, item), next)
}

/**
 * Opens a session: shows it in `pane` — the focused pane when none is named
 * — or, shown already, focuses the pane showing it.
 */
export function openSession(
  state: WorkspaceState,
  { sessionId, pane }: { sessionId: string; pane?: PaneKey },
): WorkspaceState {
  return openItem(state, { item: sessionItem(sessionId), pane })
}

/** Where a new pane showing `item` goes on `side` of `target`, if the room allows. */
function besideIn(
  panes: PaneLayout,
  target: PaneKey,
  side: Side,
  item: PaneItemKey,
  room: PaneRoom | undefined,
): Arranged | null {
  return arrange(panes, splitPane(panes, target, side, item), room)
}

/**
 * Opens an item beside a pane, by `openBeside`'s rules: on `side` of
 * `target`, or with none named to the right, else below; where it will not
 * fit, in the target's place unless `replace` is false; shown already,
 * focused where it is.
 */
function openItemBeside(
  state: WorkspaceState,
  {
    item,
    target,
    side,
    room,
    replace = true,
  }: {
    item: PaneItem
    target?: PaneKey
    side?: Side
    room: PaneRoom | undefined
    replace?: boolean
  },
): WorkspaceState {
  const panes = state.panes
  if (!panes || !showable(state, item)) return state
  const beside = target ?? panes.focused
  if (paneShowingItem(panes, item) || !locate(panes, beside))
    return openItem(state, { item })
  const key = paneItemKey(item)
  const placed = side
    ? besideIn(panes, beside, side, key, room)
    : (besideIn(panes, beside, "right", key, room) ??
      besideIn(panes, beside, "bottom", key, room))
  if (placed) return arranged(opened(state, item), placed)
  return replace ? openItem(state, { item, pane: beside }) : state
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
    replace,
  }: {
    sessionId: string
    target?: PaneKey
    side?: Side
    room: PaneRoom | undefined
    replace?: boolean
  },
): WorkspaceState {
  return openItemBeside(state, {
    item: sessionItem(sessionId),
    target,
    side,
    room,
    replace,
  })
}

/**
 * Opens a widget in a pane of its own (ADR 326's `pane` place): beside the
 * pane showing `origin` — the session the caller says the widget belongs
 * to — by `openBeside`'s rules, or in the focused pane's place when no pane
 * shows it or there is none. A widget on screen already is focused where it
 * is.
 */
export function openWidget(
  state: WorkspaceState,
  {
    widget,
    origin,
    room,
  }: { widget: WidgetRef; origin?: string; room: PaneRoom | undefined },
): WorkspaceState {
  const panes = state.panes
  if (!panes) return state
  const item = widgetItem(widget)
  const beside = origin === undefined ? undefined : paneShowingSession(panes, origin)
  return beside
    ? openItemBeside(state, { item, target: beside.key, room })
    : openItem(state, { item })
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
  // A key no pane can hold, so only the room decides.
  const sides: readonly Side[] = side ? [side] : ["right", "bottom"]
  return sides.some((each) => besideIn(panes, beside, each, roomProbeKey, room) !== null)
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
  const item = sessionItem(sessionId)
  if (!state.panes || !showable(state, item)) return state
  const outcome = dropOutcome(
    state.panes,
    { kind: "item", item: paneItemKey(item) },
    target,
    zone,
    room,
  )
  return outcome ? arranged(opened(state, item), outcome) : state
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
  if (!panes || showable(state, sessionItem(draftId))) return state
  const home = draftHome(state, { channelId, model })
  if (!home) return state
  const drafted: WorkspaceState = {
    ...state,
    drafts: {
      ...state.drafts,
      [draftId]: { id: draftId, ...home },
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
 * Where a new session starts and on what: `channelId` if the window has it,
 * else where a new session goes (`draftChannel`); `model`, else the default.
 * None when there is no channel or no model to start on — a new session
 * runs on a model, and with none, none starts.
 */
function draftHome(
  state: WorkspaceState,
  { channelId, model }: { channelId?: string; model?: ModelRef },
): { channelId: string; model: ModelRef } | undefined {
  const home = channelId && channelOf(state, channelId) ? channelId : draftChannel(state)
  const startsOn = model ?? defaultModel()
  return home && startsOn ? { channelId: home, model: startsOn } : undefined
}

/**
 * Where the last pane goes back to when it closes: a new session's home in
 * the channel of the listed session it shows, on the model its next message
 * would take; showing a widget, where a new session goes, on the default
 * model. None for a home already, or where no new session can start.
 */
function homeAfterLast(
  state: WorkspaceState,
  item: PaneItem | null,
): { channelId: string; model: ModelRef } | undefined {
  if (item?.kind === "widget") return draftHome(state, {})
  const session = item ? sessionOf(state, item.sessionId) : undefined
  if (!session) return undefined
  return draftHome(state, {
    channelId: session.channelId,
    model: modelForNextTurn(state, session.id) ?? session.model,
  })
}

/**
 * Whether a pane closes: any one of several; the last only when it goes
 * back to a new session's home (`homeAfterLast`). The one rule `closePane`
 * follows, and what a pane's close button and menu item ask before they
 * offer it.
 */
export function canClosePane(state: WorkspaceState, pane: PaneKey): boolean {
  const panes = state.panes
  const shown = panes && paneByKey(panes, pane)
  if (!panes || !shown) return false
  return paneCount(panes) > 1 || homeAfterLast(state, itemIn(shown)) !== undefined
}

/**
 * Closes a pane; its neighbour takes the room. The last pane cannot close:
 * it goes back to a new session's home under `draftId` (`homeAfterLast`) —
 * showing a conversation, in its channel; showing a widget, where a new
 * session goes (the channel being looked at, else the first), on the
 * default model. Showing a home already, nothing changes (`canClosePane`).
 */
export function closePane(
  state: WorkspaceState,
  { pane, draftId }: { pane: PaneKey; draftId?: string },
): WorkspaceState {
  const panes = state.panes
  const shown = panes && paneByKey(panes, pane)
  if (!panes || !shown) return state
  if (paneCount(panes) > 1) return withPanes(state, removePane(panes, pane))
  const home = homeAfterLast(state, itemIn(shown))
  if (!home || !draftId) return state
  return createDraft(state, { draftId, ...home, target: pane })
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
