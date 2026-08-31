import type { Conversation, ConversationStrip } from "../../model"
import { conversationInStrip } from "../internal"

/** The open conversation from a strip snapshot. */
export function activeConversation(strip: ConversationStrip): Conversation {
  return conversationInStrip(strip.conversations, strip.activeId)
}
