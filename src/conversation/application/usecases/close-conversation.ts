import type { ConversationStrip } from "../../model"
import { closeInStrip, takeConversationId } from "../internal"

/** Close a tab. The strip never empties. Server-owned once conversations persist. */
export function closeConversation(
  strip: ConversationStrip,
  conversationId: string,
): ConversationStrip {
  let current = strip
  const closed = closeInStrip(strip.conversations, conversationId, strip.activeId, () => {
    const taken = takeConversationId(current)
    current = taken.strip
    return taken.id
  })
  return {
    ...current,
    conversations: closed.items,
    activeId: closed.activeId,
  }
}
