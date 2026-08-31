import type { ConversationStrip } from "../../model"
import type { ReplySource } from "../ports"
import {
  advance,
  conversationOnStrip,
  replaceConversation,
  standInReply,
  takeTurnId,
} from "../internal"

/**
 * Walk `thinking → streaming → idle`. Server-owned: a real runtime will
 * drive this from stream events instead of a clock.
 */
export function advanceReply(
  strip: ConversationStrip,
  conversationId: string,
  replies: ReplySource = standInReply,
): ConversationStrip {
  const current = conversationOnStrip(strip, conversationId)
  if (!current) return strip
  const nextId = takeTurnId(strip)
  return replaceConversation(nextId.strip, advance(current, nextId.id, replies.reply))
}
