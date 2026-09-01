import type { LocalStrip } from "../local-strip"
import { conversationOnStrip, replaceConversation, send, takeTurnId } from "../internal"

export function sendDraft(strip: LocalStrip, conversationId?: string): LocalStrip {
  const id = conversationId ?? strip.activeId
  const current = conversationOnStrip(strip, id)
  if (!current) return strip
  const user = takeTurnId(strip)
  const assistant = takeTurnId(user.strip)
  const next = send(current, current.draft, user.id, assistant.id)
  if (next === current) return strip
  return replaceConversation(assistant.strip, next)
}
