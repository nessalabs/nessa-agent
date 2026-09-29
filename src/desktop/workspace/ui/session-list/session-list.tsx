import { memo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react"
import { shallowEqual } from "react-redux"
import { useRunningFirstPreference } from "../../../adapters/window-preferences"
import { DesktopIcon } from "../../../ui/icons"
import { newSession, openSession } from "../../adapters/store/commands"
import {
  useWorkspaceDispatch,
  useWorkspaceSelector,
  useWorkspaceStore,
} from "../../adapters/store/hooks"
import {
  sameListGroups,
  selectChannel,
  selectFocusedSessionId,
  selectListGroups,
  selectSession,
  selectSessionListOpen,
  selectShownSessionIds,
  selectView,
} from "../../adapters/store/selectors"
import { contextMenuFromKey } from "../../adapters/dom/context-menu-key"
import { useNow } from "../../adapters/dom/clock"
import { sessionTime } from "../../model/time-labels"
import { sameWords } from "../../model/transcript"
import { AgentTile } from "../chrome/agent-tile"
import { ColumnHeader } from "../../../ui/column-header"
import { IconButton } from "../chrome/icon-button"
import { SessionMenuItems, useOpenFromRow } from "../session-actions"
import { useWorkspaceFrame } from "../workspace-frame"
import "./session-list.css"
import { ContextMenu, ContextMenuContent, ContextMenuTrigger } from "../../../ui/menu"

/**
 * The middle column: what the sidebar chose — a channel's sessions, or every
 * session in one state — grouped "Needs you", "Running", "Earlier", with a
 * search over them. Arrow keys walk the list and open as they go, like Mail's
 * message list. A new session appears here with its first message.
 */
export const SessionList = memo(function SessionList() {
  const dispatch = useWorkspaceDispatch()
  const store = useWorkspaceStore()
  const frame = useWorkspaceFrame()
  const open = useWorkspaceSelector(selectSessionListOpen)
  const view = useWorkspaceSelector(selectView)
  const channel = useWorkspaceSelector((state) => selectChannel(state, view.channelId))
  // The search is the list's own; choosing another channel starts it afresh.
  const [search, setSearch] = useState({ view, query: "" })
  const query = search.view === view ? search.query : ""
  const setQuery = (next: string) => setSearch({ view, query: next })
  const [runningFirst] = useRunningFirstPreference()
  const groups = useWorkspaceSelector(
    (state) => selectListGroups(state, query, runningFirst === "on"),
    sameListGroups,
  )
  const listRef = useRef<HTMLDivElement>(null)
  const title = channel?.name ?? ""
  const ordered = groups.flatMap((group) => group.ids)
  // One row takes Tab: the open one, or the first when none here is open.
  const focusedId = useWorkspaceSelector(selectFocusedSessionId)
  const tabStop = focusedId && ordered.includes(focusedId) ? focusedId : ordered[0]

  // ↑ and ↓ walk the list, Home and End go to its ends, each opening as it
  // goes; the context-menu key opens the focused row's menu.
  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (contextMenuFromKey(event)) return
    const walk: Record<string, (at: number) => number> = {
      ArrowDown: (at) => at + 1,
      ArrowUp: (at) => at - 1,
      Home: () => 0,
      End: () => ordered.length - 1,
    }
    if (!Object.hasOwn(walk, event.key)) return
    event.preventDefault()
    const focused = selectFocusedSessionId(store.getState())
    const at = ordered.findIndex((id) => id === focused)
    const next = ordered[Math.min(Math.max(walk[event.key](at), 0), ordered.length - 1)]
    if (!next) return
    dispatch(openSession({ sessionId: next }))
    listRef.current
      ?.querySelector<HTMLElement>(`[data-session-row="${CSS.escape(next)}"]`)
      ?.focus()
  }

  const shortcut = frame.shortcut("search")
  return (
    <section
      className="workspace-list"
      aria-label={title}
      inert={!open}
      aria-hidden={!open || undefined}
    >
      <div className="workspace-list-inner" data-flip="slide" data-flip-id="list">
        <ColumnHeader
          title={title}
          flipId="column-title"
          icon={<DesktopIcon name={channel?.private ? "privateChannel" : "channel"} />}
          action={
            <IconButton
              icon="newSession"
              label="New Session"
              shortcut={frame.shortcut("newSession")}
              onClick={() => dispatch(newSession())}
            />
          }
        >
          <label className="workspace-search">
            <DesktopIcon name="search" />
            <input
              type="search"
              placeholder="Search sessions"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Escape") setQuery("")
              }}
            />
            {query || !shortcut ? null : <kbd>{shortcut}</kbd>}
          </label>
        </ColumnHeader>
        <div
          className="workspace-list-scroll"
          ref={listRef}
          role="listbox"
          aria-label="Sessions"
          onKeyDown={onKeyDown}
        >
          {ordered.length === 0 ? (
            <div className="workspace-list-empty">
              <p>{query ? "No sessions match." : "No sessions here yet."}</p>
              {query ? null : (
                <button
                  type="button"
                  className="workspace-button"
                  onClick={() => dispatch(newSession())}
                >
                  New Session
                </button>
              )}
            </div>
          ) : null}
          {groups.map((group, index) => (
            <div key={group.id} role="group" aria-label={group.label}>
              <h3 className="workspace-group-label" data-first={index === 0 || undefined}>
                {group.label}
                <span>{group.ids.length}</span>
              </h3>
              {group.ids.map((sessionId) => (
                <SessionRow
                  key={sessionId}
                  sessionId={sessionId}
                  tabStop={sessionId === tabStop}
                />
              ))}
            </div>
          ))}
        </div>
      </div>
    </section>
  )
})

const SessionRow = memo(function SessionRow({
  sessionId,
  tabStop,
}: {
  sessionId: string
  /** Whether the row is the list's one Tab stop. */
  tabStop: boolean
}) {
  const session = useWorkspaceSelector((state) => selectSession(state, sessionId))
  const { selected, open } = useWorkspaceSelector(
    (state) => ({
      selected: selectFocusedSessionId(state) === sessionId,
      open: selectShownSessionIds(state).includes(sessionId),
    }),
    shallowEqual,
  )
  const actions = useOpenFromRow()
  if (!session) return null
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          role="option"
          tabIndex={tabStop ? 0 : -1}
          aria-selected={selected}
          data-session-row={session.id}
          className="workspace-session"
          data-selected={selected || undefined}
          data-open={open || undefined}
          data-unread={session.unread || undefined}
          // Carried by the pointer to a pane (`split-panes/adapters/dom/drag.ts`).
          data-drag-item={session.id}
          onClick={(event) => actions.activate(event, session.id)}
          onKeyDown={(event) => {
            if (event.key !== "Enter") return
            event.preventDefault()
            actions.activate(event, session.id)
          }}
        >
          <AgentTile model={session.model} size={22} />
          <div className="workspace-session-main">
            <div className="workspace-session-top">
              {session.unread ? (
                <span className="workspace-unread" aria-label="Unread" />
              ) : null}
              <span className="workspace-session-title workspace-truncate">
                {session.title}
              </span>
              <SessionTime at={session.updatedAt} />
            </div>
            {/* A short first message is the title too; it is said once. */}
            {sameWords(session.title, session.preview) ? null : (
              <p className="workspace-session-preview">{session.preview}</p>
            )}
          </div>
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <SessionMenuItems sessionId={session.id} />
      </ContextMenuContent>
    </ContextMenu>
  )
})

function SessionTime({ at }: { at: number }) {
  const now = useNow(30_000)
  return <time>{sessionTime(at, now)}</time>
}
