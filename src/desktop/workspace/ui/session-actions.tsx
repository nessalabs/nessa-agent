/**
 * What a session row offers, wherever the row is — the session list, a
 * channel's pinned sessions, a channel's branch in the sidebar: open it,
 * open it beside the focused pane (⌘-click), and its context menu.
 */
import { shallowEqual } from "react-redux"
import {
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
} from "@nessa-ui/react/context-menu"
import { panesOf } from "../model/pane-layout"
import { knownToSource } from "../model/revision"
import {
  archiveSession,
  closePane,
  openBeside,
  openSession,
  pinSession,
} from "../adapters/store/commands"
import {
  useWorkspaceDispatch,
  useWorkspaceSelector,
  useWorkspaceStore,
} from "../adapters/store/hooks"
import {
  selectFocusedPaneKey,
  selectPaneCount,
  selectPanes,
  selectSession,
} from "../adapters/store/selectors"
import { commandKey, commandLabel } from "../adapters/dom/shortcuts"
import type { PaneRoom } from "../model/pane-sizing"
import { useWorkspaceFrame } from "./workspace-frame"

/**
 * The focused pane's room, measured when asked — for anything that opens
 * beside it. Read when clicked, so a row never renders again because focus
 * moved.
 */
export function useFocusedRoom(): () => PaneRoom | undefined {
  const store = useWorkspaceStore()
  const frame = useWorkspaceFrame()
  return () => {
    const focused = selectFocusedPaneKey(store.getState())
    return focused === null ? undefined : frame.roomOf(focused)
  }
}

/** Opens a session in place, or beside the focused pane with the command key held. */
export function useOpenFromRow() {
  const dispatch = useWorkspaceDispatch()
  const room = useFocusedRoom()
  const open = (sessionId: string) => dispatch(openSession({ sessionId }))
  const beside = (sessionId: string) => dispatch(openBeside({ sessionId, room: room() }))
  return {
    open,
    beside,
    /** A click on a row: ⌘-click opens beside. */
    click: (event: { metaKey: boolean; ctrlKey: boolean }, sessionId: string) =>
      commandKey(event) ? beside(sessionId) : open(sessionId),
  }
}

/** A session row's context menu; its content mounts only while the menu is open. */
export function SessionMenuItems({ sessionId }: { sessionId: string }) {
  const dispatch = useWorkspaceDispatch()
  const { open, beside } = useOpenFromRow()
  const { pinned, begun } = useWorkspaceSelector((state) => {
    const session = selectSession(state, sessionId)
    return {
      pinned: session?.pinned === true,
      // Pinning and archiving are the source's: a session it has not spoken of has neither yet.
      begun: session !== undefined && knownToSource(session),
    }
  }, shallowEqual)
  const { count, pane } = useWorkspaceSelector((state) => {
    const panes = selectPanes(state)
    return {
      count: selectPaneCount(state),
      pane: panes
        ? panesOf(panes).find((each) => each.sessionId === sessionId)?.key
        : undefined,
    }
  }, shallowEqual)
  const shown = pane !== undefined
  return (
    <>
      <ContextMenuItem onSelect={() => open(sessionId)}>
        Open
        <ContextMenuShortcut>↩</ContextMenuShortcut>
      </ContextMenuItem>
      {/* As ⌘-click does: beside when there is room, in the focused pane's place when not. */}
      <ContextMenuItem onSelect={() => beside(sessionId)}>
        Open Beside
        <ContextMenuShortcut>{commandLabel}Click</ContextMenuShortcut>
      </ContextMenuItem>
      {shown && count > 1 ? (
        <ContextMenuItem onSelect={() => dispatch(closePane({ pane }))}>
          Close Pane
        </ContextMenuItem>
      ) : null}
      <ContextMenuSeparator />
      <ContextMenuItem
        disabled={!begun}
        onSelect={() =>
          void dispatch(pinSession({ sessionId, pinned: !pinned, initiator: "person" }))
        }
      >
        {pinned ? "Unpin from Sidebar" : "Pin to Sidebar"}
      </ContextMenuItem>
      <ContextMenuItem
        disabled={!begun}
        onSelect={() => void dispatch(archiveSession({ sessionId, initiator: "person" }))}
      >
        Archive
      </ContextMenuItem>
    </>
  )
}
