import type { Conversation } from "../../model"
import type { LocalStrip } from "../local-strip"

export function takeConversationId(strip: LocalStrip): {
  strip: LocalStrip
  id: string
} {
  return {
    strip: { ...strip, nextConversationId: strip.nextConversationId + 1 },
    id: `c${strip.nextConversationId}`,
  }
}

export function takeTurnId(strip: LocalStrip): {
  strip: LocalStrip
  id: string
} {
  return {
    strip: { ...strip, nextTurnId: strip.nextTurnId + 1 },
    id: `t${strip.nextTurnId}`,
  }
}

export function replaceConversation(strip: LocalStrip, next: Conversation): LocalStrip {
  return {
    ...strip,
    conversations: strip.conversations.map((item) => (item.id === next.id ? next : item)),
  }
}

export function conversationOnStrip(
  strip: LocalStrip,
  id: string,
): Conversation | undefined {
  return strip.conversations.find((item) => item.id === id)
}
