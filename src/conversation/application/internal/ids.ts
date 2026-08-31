import type { Conversation, ConversationStrip } from "../../model"

export function takeConversationId(strip: ConversationStrip): {
  strip: ConversationStrip
  id: string
} {
  return {
    strip: { ...strip, nextConversationId: strip.nextConversationId + 1 },
    id: `c${strip.nextConversationId}`,
  }
}

export function takeTurnId(strip: ConversationStrip): {
  strip: ConversationStrip
  id: string
} {
  return {
    strip: { ...strip, nextTurnId: strip.nextTurnId + 1 },
    id: `t${strip.nextTurnId}`,
  }
}

export function replaceConversation(
  strip: ConversationStrip,
  next: Conversation,
): ConversationStrip {
  return {
    ...strip,
    conversations: strip.conversations.map((item) => (item.id === next.id ? next : item)),
  }
}

export function conversationOnStrip(
  strip: ConversationStrip,
  id: string,
): Conversation | undefined {
  return strip.conversations.find((item) => item.id === id)
}
