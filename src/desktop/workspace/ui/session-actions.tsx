/**
 * What a session row offers, wherever the row is — the session list, a
 * channel's pinned sessions, a channel's branch in the sidebar: open it,
 * open it beside the focused pane (⌘-click), and its context menu.
 */
import { shallowEqual } from "react-redux"
import { paneShowingSession } from "../application/workspace-state"
import { knownToSource } from "../model/revision"
import {
  archiveSession,
  closePane,
  forkSession,
  openBeside,
  openSession,
  openShare,
  pinSession,
} from "../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../adapters/store/hooks"
import {
  selectPaneCount,
  selectPanes,
  selectSession,
  selectShareable,
} from "../adapters/store/selectors"
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

/**
 * A session row's menu — its context menu, or what its "More" button opens —
 * whose content mounts only while the menu is open. `navigation` adds the
 * ways to open it, which a row's own menu button leaves to the row. Rename
 * and Delete are the row's to carry out (`session-row-actions.tsx`): the
 * field and the confirmation outlive the menu.
 */
export function SessionMenuItems({
  sessionId,
  navigation = true,
  onRename,
  onDelete,
}: {
  sessionId: string
  navigation?: boolean
  onRename?: () => void
  onDelete?: () => void
}) {
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
      pane: panes ? paneShowingSession(panes, sessionId)?.key : undefined,
    }
  }, shallowEqual)
  const shareable = useWorkspaceSelector(selectShareable)
  const shown = pane !== undefined
  return (
    <>
      {navigation ? (
        <>
          <MenuItem onSelect={() => open(sessionId)}>
            Open
            <MenuShortcut>↩</MenuShortcut>
          </MenuItem>
          {/* As ⌘-click does: beside when there is room, in the focused pane's place when not. */}
          <MenuItem onSelect={() => beside(sessionId)}>
            Open Beside
            {besideKey.on ? (
              <MenuShortcut>{commandLabel(isMac)}Click</MenuShortcut>
            ) : null}
          </MenuItem>
          {/* Only among several: from a session's row, closing its pane is taking the
              session away, never putting a new session's home in its place, which
              closing the last pane does (`canClosePane`). */}
          {shown && count > 1 ? (
            <MenuItem onSelect={() => dispatch(closePane({ pane }))}>Close Pane</MenuItem>
          ) : null}
          <MenuSeparator />
        </>
      ) : null}
      {/* A title is the source's, as a pin is: none to change before it has spoken. */}
      {onRename ? (
        <MenuItem disabled={!begun} onSelect={onRename}>
          Rename
        </MenuItem>
      ) : null}
      <MenuItem
        data-close-immediately=""
        disabled={!begun}
        onSelect={() =>
          void dispatch(pinSession({ sessionId, pinned: !pinned, initiator: "person" }))
        }
      >
        {pinned ? "Unpin from Sidebar" : "Pin to Sidebar"}
      </MenuItem>
      {/* A copy of what was said, so none before it has spoken. */}
      <MenuItem
        disabled={!begun}
        onSelect={() => void dispatch(forkSession({ sessionId, initiator: "person" }))}
      >
        Fork
      </MenuItem>
      {/* Sharing is the gateway's: a window without one offers none, and a
          session it has not spoken of has no conversation to share yet. */}
      {shareable ? (
        <MenuItem
          disabled={!begun}
          onSelect={() => void dispatch(openShare({ sessionId }))}
        >
          Share…
        </MenuItem>
      ) : null}
      <MenuSeparator />
      <MenuItem
        data-close-immediately=""
        disabled={!begun}
        onSelect={() => void dispatch(archiveSession({ sessionId, initiator: "person" }))}
      >
        Archive
      </MenuItem>
      {onDelete ? (
        <MenuItem disabled={!begun} onSelect={onDelete}>
          Delete…
        </MenuItem>
      ) : null}
    </>
  )
}
