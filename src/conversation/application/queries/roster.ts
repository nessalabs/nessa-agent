import { conversation, type Conversation } from "../../model"
import type { ConversationSummary } from "../view"

/** What choosing a row shows: a tab this window holds, or a listed conversation to reopen. */
export type RosterTarget =
  { tabId: string } | { serverConversationId: string; title: string | null }

/** One row of the Messages list, keyed by the gateway's identity for the conversation. */
export type RosterRow = {
  id: string
  title: string
  preview?: string
  target: RosterTarget
}

/**
 * What a conversation the gateway has no title for is called, in a row and in
 * the tab a row opens: one row, one name.
 */
export const UNTITLED = "Conversation"

/** What a tab is called before anything names it; a row says {@link UNTITLED}. */
const UNNAMED_TAB = conversation("").title

/**
 * The Messages list: the gateway's list of conversations, joined to the tabs
 * this window holds, narrowed by the search text.
 *
 * The gateway owns what a row says — its title and its preview are derived
 * there, from what was said, so this shows them as given rather than deriving
 * them a second way. What only this window knows is laid over them: a tab a
 * person renamed keeps that name, and a row whose conversation is open here
 * selects that tab rather than opening another.
 *
 * An open tab the list does not name leads only when the list has not caught
 * up with it — a conversation somebody has written in since the list was read,
 * which this window has turns for. A tab the gateway left out on purpose does
 * not: one it lists among the archived, one this window has read as deleted,
 * and one this window has not read at all (a tab restored after a reload,
 * which may name a conversation deleted elsewhere or nothing was said in).
 * A tab this window did read, onto a conversation since deleted elsewhere,
 * cannot be told from one the list has not caught up with: only the active
 * tab reads, so it leads until it is opened, and opening it says it was
 * deleted — or until an action on its row is refused as deleted, which the
 * list then knows (`deletedIds`).
 *
 * `leftOut` is every conversation no row may name, whether listed or only
 * open in a tab: archived, with an archive or delete out, or known deleted.
 * A tab bound to no gateway conversation has nothing said in it and is not a
 * row at all. Past the list's bound (500 of each kind) the gateway names
 * neither the conversation nor its archiving, so an open tab onto one of
 * those leads as if new; a limit of the bound, not of this join.
 */
export function rosterRows(
  tabs: readonly Conversation[],
  listed: readonly ConversationSummary[],
  leftOut: ReadonlySet<string>,
  query: string,
): RosterRow[] {
  const byServerId = new Map(
    tabs.flatMap((tab) =>
      tab.serverConversationId ? [[tab.serverConversationId, tab] as const] : [],
    ),
  )
  const listedIds = new Set(listed.map((row) => row.conversationId))
  const unlisted = tabs.flatMap((tab): RosterRow[] =>
    tab.serverConversationId &&
    tab.turns.length > 0 &&
    !listedIds.has(tab.serverConversationId) &&
    !leftOut.has(tab.serverConversationId) &&
    tab.readError !== "deleted"
      ? [
          {
            id: tab.serverConversationId,
            title: tab.titleEdited || tab.title !== UNNAMED_TAB ? tab.title : UNTITLED,
            target: { tabId: tab.id },
          },
        ]
      : [],
  )
  const rows: RosterRow[] = [
    ...unlisted,
    // A row read in the same breath as its archiving can appear in both lists;
    // being left out wins.
    ...listed
      .filter((row) => !leftOut.has(row.conversationId))
      .map((row) => {
        const tab = byServerId.get(row.conversationId)
        return {
          id: row.conversationId,
          title: (tab?.titleEdited ? tab.title : row.title) ?? UNTITLED,
          preview: row.preview ?? undefined,
          target: tab
            ? { tabId: tab.id }
            : { serverConversationId: row.conversationId, title: row.title },
        }
      }),
  ]
  return rows.filter((row) => matchesRosterQuery(row, query))
}

/** The row that stands for the active tab, so the list can mark it. */
export function rosterSelection(tabs: readonly Conversation[], activeId: string) {
  return tabs.find((tab) => tab.id === activeId)?.serverConversationId ?? null
}

/** Case-insensitive match on what the row shows; an empty query keeps every row. */
export function matchesRosterQuery(
  row: { title: string; preview?: string },
  query: string,
) {
  const needle = query.trim().toLocaleLowerCase()
  if (needle === "") return true
  return [row.title, row.preview ?? ""].some((field) =>
    field.toLocaleLowerCase().includes(needle),
  )
}
