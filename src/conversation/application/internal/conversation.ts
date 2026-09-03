import { type Conversation } from "../../model"

export function withDraft(current: Conversation, draft: string): Conversation {
  return { ...current, draft }
}
