import type { Conversation } from "../../model"
import type { LocalTabs } from "../local-tabs"

/**
 * A conversation id no conversation on screen already answers to.
 *
 * The counter alone was trusted to say that, and it is a separate field from
 * the conversations it names, so nothing in the type stops the two parting
 * company. No path builds that state today — `emptyLocalTabs` and
 * `restoreConversationTabs` each advance the counter consistently with the
 * conversations they return — so this enforces a contract rather than fixing a
 * reachable fault. It is worth enforcing because the cost of a repeated id is
 * out of proportion to the mistake: ids key the tab strip, and a repeated key
 * is a defect the renderer will not refuse.
 *
 * The conversations are the authority on what is taken; the counter is only
 * where the search starts, and it moves past what it found, so this stays a
 * scan of the few ids nearby rather than a walk from zero. With nothing
 * claimed — the ordinary path — the counter's own answer is taken unchanged.
 *
 * Restoration also uses this allocator, so a tab replacing one whose callbacks
 * still run receives a new name (`restored-lifetimes.test.ts`, issue 812).
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

/**
 * A turn id no turn in this conversation already answers to.
 *
 * The same guard as above, for the same reason: turn ids key the transcript's
 * rows. Restoration preserves this counter while replacing the tabs, so new
 * turns do not reuse names still carried by an older local lifetime.
 */
export function takeTurnId(tabs: LocalTabs): {
  tabs: LocalTabs
  id: string
} {
  const taken = new Set(
    tabs.conversations.flatMap((item) => item.turns.map((turn) => turn.id)),
  )
  let next = tabs.nextTurnId
  while (taken.has(`t${next}`)) next += 1
  return {
    tabs: { ...tabs, nextTurnId: next + 1 },
    id: `t${next}`,
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
