import { conversation, type ConversationStrip } from "../../model"
import { takeConversationId } from "../internal"

/** Open a new empty tab. Server-owned once conversations persist. */
export function openConversation(strip: ConversationStrip): ConversationStrip {
  const next = takeConversationId(strip)
  return {
    ...next.strip,
    conversations: [...next.strip.conversations, conversation(next.id)],
    activeId: next.id,
  }
}
