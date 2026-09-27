import { memo } from "react"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuTrigger,
} from "@nessa-ui/react/context-menu"
import { shallowEqual } from "react-redux"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectFocusedSessionId,
  selectSession,
  selectShownSessionIds,
} from "../../adapters/store/selectors"
import { useSessionDrag } from "../../adapters/dom/drag"
import { useNow } from "../../adapters/dom/clock"
import { commandLabel } from "../../adapters/dom/shortcuts"
import { sessionTime } from "../../model/time-labels"
import { AgentTile } from "../chrome/agent-tile"
import { StatusGlyph } from "../chrome/status-glyph"
import { SessionMenuItems, useOpenFromRow } from "../session-actions"

/**
 * A session hanging beneath its channel in the sidebar. `pinned`: one of the
 * chosen channel's pinned sessions, marked by its agent. `branch`: one of the
 * sessions a channel discloses inline, with its status and time.
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
      focused: selectFocusedSessionId(state) === sessionId,
    }),
    shallowEqual,
  )
  const actions = useOpenFromRow()
  const startDrag = useSessionDrag()
  if (!session) return null
  return (
    <li>
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <button
            type="button"
            className="workspace-row workspace-thread-row"
            data-row="session"
            data-session-row={session.id}
            data-parent={channelId}
            data-open={open || undefined}
            data-focused={(kind === "branch" && focused) || undefined}
            data-unread={(kind === "branch" && session.unread) || undefined}
            aria-current={focused ? "page" : undefined}
            draggable
            onDragStart={(event) => startDrag(event, session.id, session.title)}
            onClick={(event) => actions.click(event, session.id)}
            title={
              kind === "branch"
                ? `${session.title} — ${commandLabel}-click to open beside`
                : session.title
            }
          >
            {kind === "pinned" ? (
              <>
                <AgentTile model={session.model} size={14} />
                <span className="workspace-truncate">{session.title}</span>
                <StatusGlyph status={session.status} />
              </>
            ) : (
              <>
                <StatusGlyph status={session.status} idle />
                <span className="workspace-truncate">{session.title}</span>
                <BranchTime at={session.updatedAt} />
              </>
            )}
          </button>
        </ContextMenuTrigger>
        <ContextMenuContent className="desktop-popover workspace-menu">
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
