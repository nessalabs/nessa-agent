import type { LocalStrip } from "../local-strip"
import { conversationOnStrip, replaceConversation, withDraft } from "../internal"

export function setDraft(
  strip: LocalStrip,
  input: { draft: string; id?: string },
): LocalStrip {
  const current = conversationOnStrip(strip, input.id ?? strip.activeId)
  if (!current) return strip
  return replaceConversation(strip, withDraft(current, input.draft))
}
