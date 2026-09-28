import { memo } from "react"
import { shallowEqual } from "react-redux"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectContentView,
  selectFocusedSessionId,
  selectSession,
  selectShownSessionIds,
} from "../../adapters/store/selectors"
import { useNow } from "../../adapters/dom/clock"
import { commandLabel } from "../../adapters/dom/shortcuts"
import { sessionTime } from "../../model/time-labels"
import { AgentTile } from "../chrome/agent-tile"
import { StatusGlyph } from "../chrome/status-glyph"
import { SessionMenuItems, useOpenFromRow } from "../session-actions"
import { tooltip } from "../../../ui/tooltip"
import { ContextMenu, ContextMenuContent, ContextMenuTrigger } from "../../../ui/menu"

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
      // The focused pane's, while the panes are shown rather than the Agents overview.
      focused:
        selectFocusedSessionId(state) === sessionId &&
        selectContentView(state) === "panes",
    }),
    shallowEqual,
  )
  const actions = useOpenFromRow()
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
            // Carried by the pointer to a pane (`adapters/dom/drag.ts`).
            data-drag-session={session.id}
            onClick={(event) => actions.activate(event, session.id)}
            {...tooltip(
              kind === "branch" && actions.besideKey.on
                ? `${session.title} — ${commandLabel}-click to open beside`
                : session.title,
            )}
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
