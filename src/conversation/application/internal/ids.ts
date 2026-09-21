import type { Conversation } from "../../model"
import type { LocalTabs } from "../local-tabs"

/**
 * A conversation id no conversation on screen already answers to.
 *
 * The counter alone was trusted to say that, and it is not enough: it is a
 * separate field from the conversations it names, so the two can part company.
 * `emptyTabs` opens `c0` with the counter at 1, a restore sets the counter from
 * how many tabs it restored, and anything that replaces the list wholesale
 * carries its own idea of both. One counter behind its conversations by a
 * single step hands out an id already in use.
 *
 * A repeated id is not a cosmetic collision. The tab strip keys each tab by it,
 * and React's answer to a repeated key is to duplicate the children under it —
 * which is what filled the composer with queue chips that each reported a
 * different count and outlived the conversation they belonged to.
 *
 * So the conversations are the authority on what is taken, and the counter is
 * only where the search starts. It moves past what it found, so this stays a
 * scan of the few ids nearby rather than a walk from zero every time.
 */
export function takeConversationId(tabs: LocalTabs): {
  tabs: LocalTabs
  id: string
} {
  const taken = new Set(tabs.conversations.map((item) => item.id))
  let next = tabs.nextConversationId
  while (taken.has(`c${next}`)) next += 1
  return {
    tabs: { ...tabs, nextConversationId: next + 1 },
    id: `c${next}`,
  }
}

export function takeTurnId(tabs: LocalTabs): {
  tabs: LocalTabs
  id: string
} {
  return {
    tabs: { ...tabs, nextTurnId: tabs.nextTurnId + 1 },
    id: `t${tabs.nextTurnId}`,
  }
}

export function replaceConversation(tabs: LocalTabs, next: Conversation): LocalTabs {
  return {
    ...tabs,
    conversations: tabs.conversations.map((item) => (item.id === next.id ? next : item)),
  }
}

export function findConversation(tabs: LocalTabs, id: string): Conversation | undefined {
  return tabs.conversations.find((item) => item.id === id)
}
