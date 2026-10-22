/**
 * The pane commands: place, move, and close, and ask whether a placement
 * fits. A new draft and a channel opened into a pane measure the room, so
 * they live here. Re-exported by `commands.ts`, the one list.
 */
import type { WidgetRef } from "../../../widgets/model/widget-ref"
import type { Drop } from "../../../split-panes"
import {
  type Direction,
  type PaneKey,
  type Side,
  type Zone,
} from "../../../split-panes/model/pane-layout"
import type { PaneRoom } from "../../../split-panes/model/pane-sizing"
import { draftOf } from "../../application/workspace-state"
import * as panesUseCases from "../../application/usecases/panes"
import { paneItemOf } from "../../model/pane-item"
import type { WorkspaceCommand } from "./commands"
import { workspaceActions } from "./slice"

const {
  channelOpened,
  closeInFront: closedInFront,
  draftCreated,
  openedBeside,
  paneClosed,
  paneMoved,
  paneNudged,
  panesFitted,
  sessionDropped,
  showContent,
  widgetOpened,
} = workspaceActions

/**
 * Opens a session beside a pane — the focused one unless `target` is named —
 * on `side`, or, with none named, to the right, else below; only where the
 * room the page measures allows (`arrange`). Where it will not fit, the
 * session takes the target's place, unless `replace` is false. A session on
 * screen already is focused where it is.
 */
export function openBeside(options: {
  sessionId: string
  target?: PaneKey
  side?: Side
  replace?: boolean
}): WorkspaceCommand {
  return (dispatch, _getState, { measure }) => {
    dispatch(openedBeside({ ...options, room: measure() }))
  }
}

/**
 * Opens a session on a pane's zone, as a drop there would: the middle opens
 * it there, a side beside it there where the room the page measures now
 * allows, else nothing. An agent's way to do what a drag does.
 */
export function dropSession(options: {
  sessionId: string
  target: PaneKey
  zone: Zone
}): WorkspaceCommand {
  return (dispatch, _getState, { measure }) => {
    dispatch(sessionDropped({ ...options, room: measure() }))
  }
}

/**
 * Moves a pane to a zone of another: the middle swaps them; a side only where
 * the room the page measures now allows. An agent's way to do what a drag does.
 */
export function movePane(options: {
  pane: PaneKey
  target: PaneKey
  zone: Zone
}): WorkspaceCommand {
  return (dispatch, _getState, { measure }) => {
    dispatch(paneMoved({ ...options, room: measure() }))
  }
}

/**
 * The keyboard's move: a swap with the pane that way, or a column of its own
 * where the room allows.
 */
export function nudgePane(options: {
  pane: PaneKey
  direction: Direction
}): WorkspaceCommand {
  return (dispatch, _getState, { measure }) => {
    dispatch(paneNudged({ ...options, room: measure() }))
  }
}

/**
 * Holds every pane to the readable size in the room the page measures now —
 * after the window or a side column changed it — rebalancing where it must.
 */
export function fitPanes(): WorkspaceCommand {
  return (dispatch, _getState, { measure }) => {
    const room = measure()
    if (room) dispatch(panesFitted({ room }))
  }
}

/**
 * The panes' room as the page measures it now: what the split panes' drag
 * reads once, as its press begins (`split-panes-source.ts`).
 */
export function measureRoom(): WorkspaceCommand<PaneRoom | undefined> {
  return (_dispatch, _getState, { measure }) => measure()
}

/**
 * The drag's drop: commits what the drag previewed for the same zone
 * (`dropOutcome` of the same panes), in the same room — the one read as the
 * press began, so the drop reads nothing of the page. The drag's own,
 * through the split panes' source; an agent moves a pane with `movePane` or
 * opens a session with `dropSession`, which measure the room as they run.
 */
export function commitDrop({ carried, target, zone, room }: Drop): WorkspaceCommand {
  return (dispatch) => {
    if (carried.kind === "pane") {
      dispatch(paneMoved({ pane: carried.pane, target, zone, room }))
      return
    }
    // Carried from outside the grid: a session's row. Nothing carries a widget in.
    const item = paneItemOf(carried.item)
    if (item?.kind === "session")
      dispatch(sessionDropped({ sessionId: item.sessionId, target, zone, room }))
  }
}

/**
 * Whether a session could open beside `target` (the focused pane) on `side`,
 * or on either when none is named, without taking its place. What a menu or
 * the switcher asks before it offers "beside", so it never promises what the
 * command will not do.
 */
export function canOpenBeside(
  options: { target?: PaneKey; side?: Side } = {},
): WorkspaceCommand<boolean> {
  return (_dispatch, getState, { measure }) =>
    panesUseCases.canOpenBeside(getState().workspace, { ...options, room: measure() })
}

/** Whether a pane can be moved that way: what the Move items ask before they offer it. */
export function canNudge(options: {
  pane: PaneKey
  direction: Direction
}): WorkspaceCommand<boolean> {
  return (_dispatch, getState, { measure }) =>
    panesUseCases.canNudge(getState().workspace, { ...options, room: measure() })
}

/**
 * Starts a new session as a draft: in the focused pane (or `target`), or
 * beside it on `beside` where the room allows, in the channel being looked
 * at unless one is named. Returns the draft's id, or nothing when no pane
 * took it.
 */
export function newSession(
  options: {
    beside?: Side
    target?: PaneKey
    channelId?: string
  } = {},
): WorkspaceCommand<string | undefined> {
  return (dispatch, getState, { newId, measure }) => {
    const draftId = newId()
    dispatch(
      draftCreated({ draftId, ...options, room: options.beside ? measure() : undefined }),
    )
    return draftOf(getState().workspace, draftId) ? draftId : undefined
  }
}

/**
 * Closes a pane — the focused one unless `pane` is named. The last pane goes
 * back to a new session's home instead.
 */
export function closePane({ pane }: { pane?: PaneKey } = {}): WorkspaceCommand {
  return (dispatch, getState, { newId }) => {
    const panes = getState().workspace.panes
    if (!panes) return
    dispatch(paneClosed({ pane: pane ?? panes.focused, draftId: newId() }))
  }
}

/**
 * Closes what is in front (⌘W): a widget over the panes, back to the panes,
 * never a pane beneath it; else the focused pane, which, the last, goes
 * back to a new session's home.
 */
export function closeInFront(): WorkspaceCommand {
  return (dispatch, _getState, { newId }) => {
    dispatch(closedInFront({ draftId: newId() }))
  }
}

/**
 * Opens a widget (ADR 326): in a pane of its own — beside the pane showing
 * `origin`, the session the caller says it belongs to, where the room the
 * page measures allows, else in the focused pane's place — or over the
 * panes, in the window, replacing any widget there.
 */
export function openWidget({
  widget,
  place,
  origin,
}: {
  widget: WidgetRef
  place: "pane" | "window"
  origin?: string
}): WorkspaceCommand {
  return (dispatch, _getState, { measure, paintedRoom }) => {
    dispatch(
      place === "window"
        ? showContent({ content: { widget } })
        : widgetOpened({ widget, origin, room: paintedRoom?.() ?? measure() }),
    )
  }
}

/**
 * Opens a channel's newest session, in place or beside, or a new session
 * when it has none.
 */
export function openChannel({
  channelId,
  beside = false,
}: {
  channelId: string
  beside?: boolean
}): WorkspaceCommand {
  return (dispatch, _getState, { newId, measure }) => {
    dispatch(
      channelOpened({
        channelId,
        beside,
        draftId: newId(),
        room: beside ? measure() : undefined,
      }),
    )
  }
}

/**
 * Closes one chat from the tab strip: its pane out of a merged tab, or its
 * tab; the last tab's last chat goes back to a new session's home.
 */
export function closeSessionTab({ sessionId }: { sessionId: string }): WorkspaceCommand {
  return (dispatch, _getState, { newId }) => {
    dispatch(workspaceActions.tabClosed({ sessionId, draftId: newId() }))
  }
}
