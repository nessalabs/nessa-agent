import type { Conversation } from "../../model"
import type { LocalTabs } from "../local-tabs"

/**
 * A conversation id no conversation on screen already answers to.
 *
 * The counter alone was trusted to say that, and it is a separate field from
 * the conversations it names, so nothing in the type stops the two parting
 * company. No path builds that state today — `emptyLocalTabs` and
 * `restoreConversationTabs` each set the counter consistently with the
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
 * `takeTurnId` below has the same shape and no such guard. Restoration resets
 * `nextTurnId` to 1 while dropping every local turn, so the two cannot
 * disagree today; that is a coupling between two files rather than a property
 * of either.
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
 * rows. Restoration resets `nextTurnId` to 1 while dropping every local turn,
 * so the counter and the turns cannot disagree today — but that is a coupling
 * between two files rather than a property of either, and the file that would
 * break it is not this one.
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
