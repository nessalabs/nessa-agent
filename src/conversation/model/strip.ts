import { conversation, type Conversation } from "./types"

/**
 * The strip the panel paints. Counters are how the local stand-in mints
 * ids; a server will mint its own and the UI will just store what it gets.
 */
export type ConversationStrip = {
  conversations: Conversation[]
  activeId: string
  nextConversationId: number
  nextTurnId: number
}

export function emptyStrip(): ConversationStrip {
  return {
    conversations: [conversation("c0")],
    activeId: "c0",
    nextConversationId: 1,
    nextTurnId: 1,
  }
}
