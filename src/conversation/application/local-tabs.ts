import { emptyTabs, type ConversationTabs } from "../model"

/** UI-session tabs and counters for locally owned conversation, turn and placement identities. */
export type LocalTabs = ConversationTabs & {
  nextConversationId: number
  nextTurnId: number
  nextPlacementId: number
}

export function emptyLocalTabs(): LocalTabs {
  return {
    ...emptyTabs(),
    nextConversationId: 1,
    nextTurnId: 1,
    nextPlacementId: 1,
  }
}
