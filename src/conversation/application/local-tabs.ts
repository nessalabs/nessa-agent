import { emptyTabs, type ConversationTabs } from "../model"

/** UI-session tab/turn lifetimes, distinct from durable gateway identities. */
export type LocalTabs = ConversationTabs & {
  nextConversationId: number
  nextTurnId: number
}

export function emptyLocalTabs(): LocalTabs {
  return {
    ...emptyTabs(),
    nextConversationId: 1,
    nextTurnId: 1,
  }
}
