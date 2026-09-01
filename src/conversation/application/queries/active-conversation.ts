import {
  conversationInStrip,
  type Conversation,
  type ConversationStrip,
} from "../../model"

export function activeConversation(strip: ConversationStrip): Conversation {
  return conversationInStrip(strip.conversations, strip.activeId)
}
