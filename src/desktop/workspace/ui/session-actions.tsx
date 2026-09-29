/**
 * What a session row offers, wherever the row is — the session list, a
 * channel's pinned sessions, a channel's branch in the sidebar: open it,
 * open it beside the focused pane (⌘-click), and its context menu.
 */
import { shallowEqual } from "react-redux"
import { paneShowing } from "../model/pane-layout"
import { knownToSource } from "../model/revision"
import {
  archiveSession,
  closePane,
  openBeside,
  openSession,
  pinSession,
} from "../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../adapters/store/hooks"
import { selectPaneCount, selectPanes, selectSession } from "../adapters/store/selectors"
import { useBesidePreference } from "../../adapters/window-preferences"
import { MenuItem, MenuSeparator, MenuShortcut } from "../../ui/menu"
import { isMac } from "../../adapters/platform"
import { commandKey, commandLabel } from "../../model/keyboard"

/**
 * Whether a click or ↩ on a row asks for beside: the command key held, while
 * Settings › Workspace › "⌘-click opens beside" is on. The one place that
 * says so, for every row and the channels that open like them.
 */
export function useBesideKey(): {
  on: boolean
  asks: (event: { metaKey: boolean; ctrlKey: boolean }) => boolean
} {
  const [preference] = useBesidePreference()
  const on = preference === "on"
  return { on, asks: (event) => on && commandKey(event, isMac) }
}

/** Opens a session in place, or beside the focused pane with the command key held. */
export function useOpenFromRow() {
  const dispatch = useWorkspaceDispatch()
  const besideKey = useBesideKey()
  const open = (sessionId: string) => dispatch(openSession({ sessionId }))
  const beside = (sessionId: string) => dispatch(openBeside({ sessionId }))
  return {
    open,
    beside,
    besideKey,
    /** A click, or ↩, on a row: ⌘ with it opens beside, where Settings says it does. */
    activate: (event: { metaKey: boolean; ctrlKey: boolean }, sessionId: string) =>
      besideKey.asks(event) ? beside(sessionId) : open(sessionId),
  }
}

/** A session row's context menu; its content mounts only while the menu is open. */
export function SessionMenuItems({ sessionId }: { sessionId: string }) {
  const dispatch = useWorkspaceDispatch()
  const { open, beside, besideKey } = useOpenFromRow()
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
      pane: panes ? paneShowing(panes, sessionId)?.key : undefined,
    }
  }, shallowEqual)
  const shown = pane !== undefined
  return (
    <>
      <MenuItem onSelect={() => open(sessionId)}>
        Open
        <MenuShortcut>↩</MenuShortcut>
      </MenuItem>
      {/* As ⌘-click does: beside when there is room, in the focused pane's place when not. */}
      <MenuItem onSelect={() => beside(sessionId)}>
        Open Beside
        {besideKey.on ? <MenuShortcut>{commandLabel(isMac)}Click</MenuShortcut> : null}
      </MenuItem>
      {shown && count > 1 ? (
        <MenuItem onSelect={() => dispatch(closePane({ pane }))}>Close Pane</MenuItem>
      ) : null}
      <MenuSeparator />
      <MenuItem
        disabled={!begun}
        onSelect={() =>
          void dispatch(pinSession({ sessionId, pinned: !pinned, initiator: "person" }))
        }
      >
        {pinned ? "Unpin from Sidebar" : "Pin to Sidebar"}
      </MenuItem>
      <MenuItem
        disabled={!begun}
        onSelect={() => void dispatch(archiveSession({ sessionId, initiator: "person" }))}
      >
        Archive
      </MenuItem>
    </>
  )
}
