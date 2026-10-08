import { memo } from "react"
import { shallowEqual } from "react-redux"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectFocusedSessionId,
  selectOverviewOpen,
  selectSession,
  selectShownSessionIds,
} from "../../adapters/store/selectors"
import { paneItemKey, sessionItem } from "../../model/pane-item"
import { useNow } from "../../adapters/dom/clock"
import { isMac } from "../../../adapters/platform"
import { commandLabel } from "../../../model/keyboard"
import { sessionTime } from "../../model/time-labels"
import { ListRow } from "../../../ui/list-row"
import { AgentTile } from "../chrome/agent-tile"
import { StatusGlyph } from "../chrome/status-glyph"
import { SessionMenuItems, useOpenFromRow } from "../session-actions"
import { tooltip } from "../../../ui/tooltip"
import { ContextMenu, ContextMenuContent, ContextMenuTrigger } from "../../../ui/menu"

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
  const { open, focused } = useWorkspaceSelector(
    (state) => ({
      open: selectShownSessionIds(state).includes(sessionId),
      // The focused pane's, beside the panes or a widget over them: only the
      // Agents overview takes the sidebar's choice for itself.
      focused: selectFocusedSessionId(state) === sessionId && !selectOverviewOpen(state),
    }),
    shallowEqual,
  )
  const actions = useOpenFromRow()
  if (!session) return null
  return (
    <li>
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <ListRow
            as="button"
            className="workspace-thread-row"
            data-row="session"
            data-session-row={session.id}
            data-parent={channelId}
            data-open={open || undefined}
            selected={kind === "branch" && focused}
            unread={kind === "branch" && session.unread}
            aria-current={focused ? "page" : undefined}
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
        <ContextMenuContent>
          <SessionMenuItems sessionId={session.id} />
        </ContextMenuContent>
      </ContextMenu>
    </li>
  )
})

function BranchTime({ at }: { at: number }) {
  const now = useNow(30_000)
  return <span className="workspace-time">{sessionTime(at, now)}</span>
}
