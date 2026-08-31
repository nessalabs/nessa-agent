import type { ConversationStrip } from "../../model"
import { conversationOnStrip, replaceConversation, withDraft } from "../internal"

/**
 * Write the composer draft. UI session state: the panel owns the in-progress
 * sentence. A server does not need this until drafts persist.
 */
export function setDraft(
  strip: ConversationStrip,
  input: { draft: string; id?: string },
): ConversationStrip {
  const current = conversationOnStrip(strip, input.id ?? strip.activeId)
  if (!current) return strip
  return replaceConversation(strip, withDraft(current, input.draft))
}
