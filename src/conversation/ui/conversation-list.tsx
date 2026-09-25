import * as React from "react"
import { Archive, SquarePen, Trash2 } from "lucide-react"
import {
  ConversationHistory,
  type ConversationHistoryAction,
} from "@nessa-ui/react/conversation-history"

import {
  archiveConversation,
  deleteConversation,
  listConversations,
} from "../adapters/store/history"
import { useConversationDispatch, useConversationSelector } from "../adapters/store/hooks"
import { canUseGateway } from "../../session"
import {
  rosterRows,
  rosterSelection,
  type RosterTarget,
  UNTITLED,
} from "../application/queries/roster"

/**
 * What a row offers when it is swiped open, or opened from the keyboard.
 * Archive first, so a full swipe archives, as Mail's does; Delete is
 * destructive, so the row itself asks to confirm it and a swipe never commits
 * it. Defined once: the list keeps a row as it was while its actions are the
 * same objects.
 */
const rowActionList: readonly ConversationHistoryAction[] = [
  { id: "archive", label: "Archive", icon: <Archive aria-hidden="true" /> },
  {
    id: "delete",
    label: "Delete",
    icon: <Trash2 aria-hidden="true" />,
    tone: "destructive",
  },
]
const rowActions = () => rowActionList

/** What the announcement says each action did. */
const verb = { archive: "Archived", unarchive: "Unarchived", delete: "Deleted" }

/**
 * The Messages list: a search field with a compose control, and a row per
 * conversation the gateway holds — closed ones included — with its last line as
 * the preview. The iMessage thread list.
 *
 * It lists and reports; it does not open anything. Choosing a row reports what
 * the row stands for — a tab this window holds, or a conversation to reopen —
 * and showing it is the host's, which in the panel means that conversation's
 * own tab, so a thread is only ever drawn in one place. Archiving and deleting
 * are the gateway's, asked for from each row's actions.
 *
 * The list is read from the gateway each time it is shown, and again if the
 * gateway only arrives after. Listing starts nothing on the gateway, so reading
 * on every opening is cheap; it is not polled, because the list is somewhere a
 * person glances, and it is the active conversation that has to stay live.
 */
export function ConversationList({
  onSelect,
  onNew,
}: {
  onSelect: (target: RosterTarget) => void
  onNew: () => void
}) {
  const [query, setQuery] = React.useState("")
  const [announcement, setAnnouncement] = React.useState("")
  const dispatch = useConversationDispatch()
  const tabs = useConversationSelector((state) => state.conversation)
  const history = useConversationSelector((state) => state.conversationHistory)
  const gatewayAvailable = useConversationSelector((state) =>
    canUseGateway(state.session),
  )
  React.useEffect(() => {
    void dispatch(listConversations())
  }, [dispatch, gatewayAvailable])
  // Rows an archive or delete was asked for and has not answered: out of the
  // list at once, so the row goes where the gesture sent it and focus moves to
  // the row that took its place. A refusal puts the row back.
  const leaving = React.useMemo(() => new Set(history.leavingIds), [history.leavingIds])
  // Left out of the list, listed or held open by a tab: archived by the
  // gateway, leaving at somebody's request, or known here to have been deleted.
  const leftOut = React.useMemo(
    () => new Set([...history.archivedIds, ...leaving, ...history.deletedIds]),
    [history.archivedIds, leaving, history.deletedIds],
  )
  const rows = React.useMemo(
    () => rosterRows(tabs.conversations, history.rows ?? [], leftOut, query),
    [tabs.conversations, history.rows, leftOut, query],
  )
  const targets = new Map(rows.map((row) => [row.id, row.target]))
  const titles = new Map(rows.map((row) => [row.id, row.title]))

  async function act(serverConversationId: string, actionId: string, title?: string) {
    const named = title ?? titles.get(serverConversationId) ?? UNTITLED
    const archive = actionId === "archive" || actionId === "unarchive"
    if (!archive && actionId !== "delete") return
    const result = archive
      ? await dispatch(
          archiveConversation({
            serverConversationId,
            archived: actionId === "archive",
            title: named,
          }),
        )
      : await dispatch(deleteConversation({ serverConversationId, title: named }))
    // Announced only when done: a delete answered, or an archive the gateway
    // applied. A second identical sentence would not change the region's text,
    // and an unchanged region is not read again; the alternating mark makes it
    // change.
    const done = result.meta.requestStatus === "fulfilled" && result.payload !== false
    if (done)
      setAnnouncement(
        (previous) =>
          `${verb[actionId as keyof typeof verb]} ${named}${previous.endsWith("\u200b") ? "" : "\u200b"}`,
      )
  }

  const empty =
    query.trim() !== ""
      ? "No results"
      : history.rows !== null
        ? "No conversations yet"
        : !gatewayAvailable
          ? "Connecting to the gateway…"
          : history.failure !== null
            ? "Conversations could not be loaded"
            : "Loading conversations…"
  // What the list has to say beside its rows: an archive or delete that did not
  // do what was asked, or rows older than the gateway's because the last read
  // of them failed.
  const status = [
    history.commandError,
    history.rows !== null && history.failure !== null
      ? "The list could not be refreshed, so it may be out of date."
      : null,
    // The gateway's own word that it left some out: an empty list or search
    // is then not all there is.
    history.rows !== null && !history.complete
      ? "Not every conversation is shown."
      : null,
  ]
    .filter(Boolean)
    .join(" ")
  // Set after mount, so a sentence that arrived while the list was closed is a
  // change the region reads out, rather than text it was born with.
  const [shownStatus, setShownStatus] = React.useState("")
  React.useEffect(() => setShownStatus(status), [status])
  // The tab already says what this is, so the list carries no title of its
  // own: compose sits in the search field's trailing end, as it does beside
  // the search in Messages.
  return (
    <div className="relative flex min-h-0 flex-1 flex-col font-sans">
      <ConversationHistory
        conversations={rows}
        value={rosterSelection(tabs.conversations, tabs.activeId)}
        onValueChange={(id) => {
          const target = targets.get(id)
          if (target) onSelect(target)
        }}
        rowActions={rowActions}
        onRowAction={(id, actionId) => void act(id, actionId)}
        query={query}
        onQueryChange={setQuery}
        searchPlaceholder="Search"
        emptyMessage={empty}
        className="[&_input[type=search]]:pe-10"
      />
      {/* Always mounted and never hidden, and filled only after it is mounted:
          a region that arrives together with its text is not reliably read. */}
      <p
        role="status"
        className={`m-0 px-2 nessa-text-2 text-muted-foreground ${
          shownStatus ? "pt-2" : "h-0 overflow-hidden"
        }`}
      >
        {shownStatus}
      </p>
      {/* The panel lists no archived conversations, so undoing the last
          archive is the way back to one, as Mail offers after a swipe. Not
          offered again while that undo is out: pressing twice would ask twice. */}
      {history.undoable && !leaving.has(history.undoable.serverConversationId) && (
        <p className="m-0 flex items-center gap-2 px-2 pt-2 nessa-text-2 text-muted-foreground">
          <span className="min-w-0 truncate">
            Archived {`“${history.undoable.title}”`}
          </span>
          <button
            type="button"
            onClick={() => {
              const { serverConversationId, title } = history.undoable ?? {}
              if (serverConversationId) void act(serverConversationId, "unarchive", title)
            }}
            className="cursor-pointer rounded border-0 bg-transparent p-0 font-medium text-(--nessa-chat-accent) hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
          >
            Undo
          </button>
        </p>
      )}
      <p role="status" className="sr-only">
        {announcement}
      </p>
      <button
        type="button"
        aria-label="New conversation"
        title="New conversation"
        onClick={onNew}
        className="absolute top-0.5 right-0.5 inline-flex size-8 cursor-pointer items-center justify-center rounded-full border-0 bg-transparent p-0 text-(--nessa-chat-accent) hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
      >
        <SquarePen aria-hidden="true" className="size-4" />
      </button>
    </div>
  )
}
