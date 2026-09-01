import type { LocalStrip } from "../local-strip"
import type { ReplySource } from "../ports"
import { advance, conversationOnStrip, replaceConversation } from "../internal"

export function advanceReply(
  strip: LocalStrip,
  conversationId: string,
  replies: ReplySource,
): LocalStrip {
  const current = conversationOnStrip(strip, conversationId)
  if (!current) return strip
  const next = advance(current, replies.reply)
  if (next === current) return strip
  return replaceConversation(strip, next)
}
