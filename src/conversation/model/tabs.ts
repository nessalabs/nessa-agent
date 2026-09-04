import { conversation, type Conversation } from "./types"

/** Open conversation tabs and which one is selected. */
export type ConversationTabs = {
  conversations: Conversation[]
  activeId: string
}

export function emptyTabs(): ConversationTabs {
  return {
    conversations: [conversation("c0")],
    activeId: "c0",
  }
}

export function conversationInTabs(
  items: Conversation[],
  activeId: string,
): Conversation {
  const found = items.find((item) => item.id === activeId) ?? items[0]
  if (!found) throw new Error("conversation tabs are empty")
  return found
}
