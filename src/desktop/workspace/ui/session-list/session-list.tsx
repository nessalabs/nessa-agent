import { memo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react"
import { shallowEqual } from "react-redux"
import { Button } from "@nessa-ui/react/button"
import { EmptyState } from "@nessa-ui/react/empty-state"
import { GroupHeader } from "@nessa-ui/react/group-header"
import { SearchField } from "@nessa-ui/react/search-field"
import { SwipeRow, SwipeRowGroup, type SwipeRowAction } from "@nessa-ui/react/swipe-row"
import { useRunningFirstPreference } from "../../../adapters/window-preferences"
import { DesktopIcon } from "../../../ui/icons"
import {
  archiveSession,
  deleteSession,
  newSession,
  openSession,
} from "../../adapters/store/commands"
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
import { useNow } from "../../../../clock"
import { sessionTime } from "../../model/time-labels"
import { sameWords } from "../../model/transcript"
import { paneItemKey, sessionItem } from "../../model/pane-item"
import { AgentTile } from "../chrome/agent-tile"
import { StatusGlyph } from "../chrome/status-glyph"
import { ColumnHeader } from "../../../ui/column-header"
import { IconButton } from "../../../ui/icon-button"
import { ListRow } from "../../../ui/list-row"
import { SessionMenuItems, useOpenFromRow } from "../session-actions"
import { useWorkspaceFrame } from "../workspace-frame"
import "./session-list.css"
import "../../../ui/pill-ink.css"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuTrigger,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "../../../ui/menu"
import { knownToSource } from "../../model/revision"
import { useSessionRowActions, type SessionRowActions } from "../session-row-actions"

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
    // A row's menu is drawn elsewhere and React carries its keys up here
    // anyway: only keys pressed in the list itself walk it.
    if (!event.currentTarget.contains(event.target as Node)) return
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
          <SearchField
            className="workspace-search"
            size="md"
            aria-label="Search sessions"
            placeholder="Search sessions"
            value={query}
            onValueChange={setQuery}
            icon={<DesktopIcon name="search" />}
            shortcut={shortcut}
            onKeyDown={(event) => {
              // With a query, Escape clears it and goes no further: a widget
              // in the window is left only by the next one (ADR 326). The
              // field sees this preventDefault and leaves the clear to us.
              if (event.key !== "Escape" || !query) return
              event.preventDefault()
              setQuery("")
            }}
          />
        </ColumnHeader>
        {/* One row's swipe actions open at a time; a scroll that moves it closes it. */}
        <SwipeRowGroup asChild data-slot="workspace-swipe">
          <div
            className="workspace-list-scroll"
            ref={listRef}
            role="listbox"
            aria-label="Sessions"
            onKeyDown={onKeyDown}
          >
            {ordered.length === 0 ? (
              <EmptyState
                className="workspace-list-empty desktop-pill-ink"
                variant="compact"
                title={query ? "No sessions match." : "No sessions here yet."}
                action={
                  query ? undefined : (
                    <Button
                      variant="tinted"
                      size="28"
                      shape="pill"
                      press="scale"
                      onClick={() => dispatch(newSession())}
                    >
                      New Session
                    </Button>
                  )
                }
              />
            ) : null}
            {groups.map((group, index) => (
              <div
                key={group.id}
                role="group"
                aria-label={`${group.label} ${group.ids.length}`}
              >
                {/* The kit caption is a heading. Hidden here, so the listbox
                  exposes the group and its options; the group's name keeps
                  the count the heading would have said. */}
                <GroupHeader
                  aria-hidden="true"
                  className="workspace-group-label"
                  data-first={index === 0 || undefined}
                  level={3}
                  size="dense"
                  tone="quiet"
                  label={group.label}
                  count={group.ids.length}
                />
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
        </SwipeRowGroup>
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
  const row = useSessionRowActions(sessionId)
  const dispatch = useWorkspaceDispatch()
  const begun = session !== undefined && knownToSource(session)
  // The row's "More" menu: while it is open its buttons stay, as on hover.
  const [more, setMore] = useState(false)
  if (!session) return null
  return (
    <ContextMenu>
      {/* A two-finger swipe left uncovers Archive and Delete, as Mail's rows
          do. The pointer's drag stays the drag to a pane, and right-click
          stays the row's menu, which holds the same two. */}
      <SwipeRow
        value={session.id}
        name={session.title}
        data-slot="workspace-swipe-row"
        actions={begun && !row.renaming ? swipeActions : noSwipe}
        mouseDrag={false}
        openOnContextMenu={false}
        onAction={(id, details) => {
          if (details.hadFocus) focusNeighbour(session.id)
          const change = { sessionId: session.id, initiator: "person" } as const
          void dispatch(id === "delete" ? deleteSession(change) : archiveSession(change))
        }}
      >
        <ContextMenuTrigger asChild>
          <ListRow
            role="option"
            tabIndex={tabStop ? 0 : -1}
            aria-selected={selected}
            data-session-row={session.id}
            className="workspace-session"
            selected={selected}
            unread={session.unread}
            data-open={open || undefined}
            data-renaming={row.renaming || undefined}
            data-actions={more || undefined}
            // Carried by the pointer to a pane (`split-panes/adapters/dom/drag.ts`).
            data-drag-item={paneItemKey(sessionItem(session.id))}
            onClick={(event) => {
              if (!row.renaming) actions.activate(event, session.id)
            }}
            onKeyDown={(event) => {
              if (event.key !== "Enter" || row.renaming) return
              event.preventDefault()
              actions.activate(event, session.id)
            }}
            leading={<AgentTile model={session.model} size={22} />}
            marker={session.unread ? <StatusGlyph status="unread" flush /> : null}
            title={row.title(session.title)}
            trailing={
              <>
                <SessionTime at={session.updatedAt} />
                {row.renaming ? null : (
                  <SessionRowButtons
                    sessionId={session.id}
                    title={session.title}
                    more={more}
                    onMore={setMore}
                    items={row.items}
                    onCloseAutoFocus={row.onCloseAutoFocus}
                  />
                )}
              </>
            }
            // A short first message is the title too; it is said once.
            description={
              sameWords(session.title, session.preview) ? null : session.preview
            }
          />
        </ContextMenuTrigger>
      </SwipeRow>
      <ContextMenuContent onCloseAutoFocus={row.onCloseAutoFocus}>
        <SessionMenuItems sessionId={session.id} {...row.items} />
      </ContextMenuContent>
    </ContextMenu>
  )
})

/**
 * What a swipe uncovers, primary first: a full swipe archives. Delete is
 * destructive, so it never commits from a swipe and asks once more in place.
 */
const swipeActions: readonly SwipeRowAction[] = [
  { id: "archive", label: "Archive", icon: <DesktopIcon name="archive" /> },
  {
    id: "delete",
    label: "Delete",
    icon: <DesktopIcon name="delete" />,
    tone: "destructive",
  },
]
const noSwipe: readonly SwipeRowAction[] = []

/**
 * Focus was in a row that is about to go: it moves to the row after it, or
 * the one before when it was the last, so the keys keep their place.
 */
function focusNeighbour(sessionId: string) {
  const row = document.querySelector<HTMLElement>(
    `[data-session-row="${CSS.escape(sessionId)}"]`,
  )
  const rows = [
    ...(row
      ?.closest('[role="listbox"]')
      ?.querySelectorAll<HTMLElement>("[data-session-row]") ?? []),
  ]
  const at = row ? rows.indexOf(row) : -1
  if (at === -1) return
  ;(rows[at + 1] ?? rows[at - 1])?.focus()
}

/**
 * What a row shows under the pointer, in its time's place: Archive, and "More",
 * which opens the row's menu less the ways to open it. Pointer conveniences:
 * out of the Tab order, since the context-menu key opens the same menu from
 * the row. Nothing pressed here, the menu's items included (React carries
 * them up through the portal), reaches the row: it would open the session.
 */
function SessionRowButtons({
  sessionId,
  title,
  more,
  onMore,
  items,
  onCloseAutoFocus,
}: {
  sessionId: string
  title: string
  more: boolean
  onMore: (open: boolean) => void
  items: SessionRowActions["items"]
  onCloseAutoFocus: (event: Event) => void
}) {
  const dispatch = useWorkspaceDispatch()
  const begun = useWorkspaceSelector((state) => {
    const session = selectSession(state, sessionId)
    return session !== undefined && knownToSource(session)
  })
  const stop = (event: { stopPropagation: () => void }) => event.stopPropagation()
  return (
    <span
      className="workspace-session-actions"
      // The row's menu (right-click, the context-menu key) holds the same,
      // for the keys and assistive technology; these are the pointer's.
      aria-hidden="true"
      onClick={stop}
      onDoubleClick={stop}
      onKeyDown={stop}
      onPointerDown={stop}
    >
      <IconButton
        icon="archive"
        label="Archive"
        size="sm"
        tabIndex={-1}
        disabled={!begun}
        aria-label={`Archive ${title}`}
        data-session-action="archive"
        onClick={() => void dispatch(archiveSession({ sessionId, initiator: "person" }))}
      />
      <DropdownMenu open={more} onOpenChange={onMore}>
        <DropdownMenuTrigger asChild>
          <IconButton
            icon="moreHorizontal"
            label="More"
            size="sm"
            tabIndex={-1}
            aria-label={`More for ${title}`}
            data-session-action="more"
          />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" onCloseAutoFocus={onCloseAutoFocus}>
          <SessionMenuItems sessionId={sessionId} navigation={false} {...items} />
        </DropdownMenuContent>
      </DropdownMenu>
    </span>
  )
}

function SessionTime({ at }: { at: number }) {
  const now = useNow(30_000)
  return <time>{sessionTime(at, now)}</time>
}
