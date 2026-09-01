import { conversation, type Conversation } from "./types"

export type ConversationStrip = {
  conversations: Conversation[]
  activeId: string
}

export function emptyStrip(): ConversationStrip {
  return {
    conversations: [conversation("c0")],
    activeId: "c0",
  }
}

export function conversationInStrip(
  items: Conversation[],
  activeId: string,
): Conversation {
  const found = items.find((item) => item.id === activeId) ?? items[0]
  if (!found) throw new Error("conversation strip is empty")
  return found
}
