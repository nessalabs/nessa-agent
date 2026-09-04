import { emptyTabs, type ConversationTabs } from "../model"

/** UI-session tabs plus local id counters (gone once the server mints ids). */
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
