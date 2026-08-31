import type { ConversationStrip } from "../../model"
import { conversationOnStrip, replaceConversation, stop } from "../internal"

/** Cancel a reply in flight. Server-owned. */
export function stopGenerating(
  strip: ConversationStrip,
  conversationId?: string,
): ConversationStrip {
  const current = conversationOnStrip(strip, conversationId ?? strip.activeId)
  if (!current) return strip
  return replaceConversation(strip, stop(current))
}
