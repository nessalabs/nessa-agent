import type { LocalStrip } from "../local-strip"
import { conversationOnStrip, replaceConversation, stop } from "../internal"

export function stopGenerating(strip: LocalStrip, conversationId?: string): LocalStrip {
  const current = conversationOnStrip(strip, conversationId ?? strip.activeId)
  if (!current) return strip
  return replaceConversation(strip, stop(current))
}
