import {
  conversationInTabs,
  type Conversation,
  type ConversationTabs,
} from "../../model"

export function activeConversation(tabs: ConversationTabs): Conversation {
  return conversationInTabs(tabs.conversations, tabs.activeId)
}
