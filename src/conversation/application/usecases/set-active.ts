import type { LocalStrip } from "../local-strip"

export function setActive(strip: LocalStrip, conversationId: string): LocalStrip {
  if (!strip.conversations.some((item) => item.id === conversationId)) return strip
  return { ...strip, activeId: conversationId }
}
