import { conversation, type ConversationSelection } from "../../model"
import type { LocalTabs } from "../local-tabs"
import { takeConversationId } from "../internal"

export function openConversation(
  tabs: LocalTabs,
  selection?: ConversationSelection,
): LocalTabs {
  const next = takeConversationId(tabs)
  return {
    ...next.tabs,
    conversations: [...next.tabs.conversations, { ...conversation(next.id), selection }],
    activeId: next.id,
  }
}

/** A model choice changes an unsent draft, or starts a separate conversation. */
export function chooseModel(
  tabs: LocalTabs,
  selection: ConversationSelection,
): LocalTabs {
  const active = tabs.conversations.find((item) => item.id === tabs.activeId)
  if (active?.serverConversationId) return openConversation(tabs, selection)
  return {
    ...tabs,
    conversations: tabs.conversations.map((item) =>
      item.id === tabs.activeId ? { ...item, selection } : item,
    ),
  }
}
