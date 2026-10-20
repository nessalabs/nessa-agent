import { memo } from "react"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  listPaintFrame,
  usePaintedWithPanes,
} from "../../adapters/store/paint-with-panes"
import {
  selectFocusedSessionId,
  selectSession,
  selectShownSessionIds,
} from "../../adapters/store/selectors"
import { paneItemKey, sessionItem } from "../../model/pane-item"
import { useNow } from "../../../../clock"
import { isMac } from "../../../adapters/platform"
import { commandLabel } from "../../../model/keyboard"
import { sessionTime } from "../../model/time-labels"
import { ListRow } from "../../../ui/list-row"
import { AgentTile } from "../chrome/agent-tile"
import { StatusGlyph } from "../chrome/status-glyph"
import { SessionMenuItems, useOpenFromRow } from "../session-actions"
import { useSessionRowActions } from "../session-row-actions"
import { tooltip } from "../../../ui/tooltip"
import { ContextMenu, ContextMenuContent, ContextMenuTrigger } from "../../../ui/menu"
import { useOverviewQuiet } from "./overview-quiet"

/**
 * A session hanging beneath its channel in the sidebar. `pinned`: one of the
 * chosen channel's pinned sessions, marked by its agent. `branch`: one of the
 * sessions a channel discloses inline, with its status and time. A `ListRow`,
 * not the kit's sidebar row: its time is as wide as it says, and the kit lays
 * what ends a row over a fixed room the title gives up.
 */
export const ThreadRow = memo(function ThreadRow({
  sessionId,
  channelId,
  kind,
}: {
  sessionId: string
  channelId: string
  kind: "pinned" | "branch"
}) {
  const session = useWorkspaceSelector((state) => selectSession(state, sessionId))
  const quiet = useOverviewQuiet()
  // With the list's measure, the frame after the boxes, so opening a pane
  // does not render the row on the click or on the box paint.
  const open = usePaintedWithPanes(
    (state) => selectShownSessionIds(state).includes(sessionId),
    listPaintFrame,
  )
  // The focused mark waits with it, so opening a pane does not render the row.
  const focused = usePaintedWithPanes(
    (state) => selectFocusedSessionId(state) === sessionId,
    listPaintFrame,
  )
  const actions = useOpenFromRow()
  // Delete only: the row is a button, which holds no field to rename in.
  const row = useSessionRowActions(sessionId)
  if (!session) return null
  return (
    <li>
      <ContextMenu onOpenChange={row.onMenuOpen}>
        <ContextMenuTrigger asChild>
          <ListRow
            ref={row.ref}
            as="button"
            className="workspace-thread-row"
            data-row="session"
            data-session-row={session.id}
            data-parent={channelId}
            data-open={open || undefined}
            selected={kind === "branch" && focused}
            unread={kind === "branch" && session.unread}
            aria-current={focused && !quiet ? "page" : undefined}
            // Carried by the pointer to a pane (`split-panes/adapters/dom/drag.ts`).
            data-drag-item={paneItemKey(sessionItem(session.id))}
            onClick={(event) => actions.activate(event, session.id)}
            {...tooltip(
              kind === "branch" && actions.besideKey.on
                ? `${session.title} — ${commandLabel(isMac)}-click to open beside`
                : session.title,
            )}
            leading={
              kind === "pinned" ? (
                <AgentTile model={session.model} size={14} />
              ) : (
                <StatusGlyph status={session.status} idle />
              )
            }
            title={session.title}
            trailing={
              kind === "pinned" ? (
                session.status === "idle" ? null : (
                  <StatusGlyph status={session.status} />
                )
              ) : (
                <BranchTime at={session.updatedAt} />
              )
            }
          />
        </ContextMenuTrigger>
        <ContextMenuContent onCloseAutoFocus={row.onCloseAutoFocus}>
          <SessionMenuItems sessionId={session.id} onDelete={row.items.onDelete} />
        </ContextMenuContent>
      </ContextMenu>
    </li>
  )
})

function BranchTime({ at }: { at: number }) {
  const now = useNow(30_000)
  return <span className="workspace-time">{sessionTime(at, now)}</span>
}
