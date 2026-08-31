import type { ConversationStrip } from "../../model"
import { conversationOnStrip, replaceConversation, send, takeTurnId } from "../internal"

/** Send the conversation's draft. Server-owned: opens the assistant row. */
export function sendDraft(
  strip: ConversationStrip,
  conversationId?: string,
): ConversationStrip {
  const id = conversationId ?? strip.activeId
  const current = conversationOnStrip(strip, id)
  if (!current) return strip
  const user = takeTurnId(strip)
  const assistant = takeTurnId(user.strip)
  const next = send(current, current.draft, user.id, assistant.id)
  if (next === current) return strip
  return replaceConversation(assistant.strip, next)
}
