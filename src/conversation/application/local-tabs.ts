import { emptyTabs, type ConversationTabs, type ModelCatalog } from "../model"

/** UI-session tabs plus local id counters (gone once the server mints ids). */
export type LocalTabs = ConversationTabs & {
  nextConversationId: number
  nextTurnId: number
  /**
   * What a tab's model can be chosen from, once loaded. Not saved with the
   * tabs: it is the gateway's answer, read again each time.
   */
  catalog?: ModelCatalog
}

export function emptyLocalTabs(): LocalTabs {
  return {
    ...emptyTabs(),
    nextConversationId: 1,
    nextTurnId: 1,
  }
}
