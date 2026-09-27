import { type MessageContent } from "../../model"
import { type Conversation } from "../../model"

export function withDraft(current: Conversation, draft: MessageContent): Conversation {
  return { ...current, draft }
}
