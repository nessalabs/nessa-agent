import { conversation } from "../../model"
import type { LocalStrip } from "../local-strip"
import { takeConversationId } from "../internal"

export function openConversation(strip: LocalStrip): LocalStrip {
  const next = takeConversationId(strip)
  return {
    ...next.strip,
    conversations: [...next.strip.conversations, conversation(next.id)],
    activeId: next.id,
  }
}
