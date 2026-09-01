import type { LocalStrip } from "../local-strip"
import { closeInStrip, takeConversationId } from "../internal"

export function closeConversation(strip: LocalStrip, conversationId: string): LocalStrip {
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
