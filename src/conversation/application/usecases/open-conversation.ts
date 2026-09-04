import { conversation } from "../../model"
import type { LocalTabs } from "../local-tabs"
import { takeConversationId } from "../internal"

export function openConversation(tabs: LocalTabs): LocalTabs {
  const next = takeConversationId(tabs)
  return {
    ...next.tabs,
    conversations: [...next.tabs.conversations, conversation(next.id)],
    activeId: next.id,
  }
}
