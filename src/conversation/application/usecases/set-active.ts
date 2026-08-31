import type { ConversationStrip } from "../../model"

/**
 * Which tab is open. UI session state: the panel remembers the selection.
 */
export function setActive(
  strip: ConversationStrip,
  conversationId: string,
): ConversationStrip {
  if (!strip.conversations.some((item) => item.id === conversationId)) return strip
  return { ...strip, activeId: conversationId }
}
