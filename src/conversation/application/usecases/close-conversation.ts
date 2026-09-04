import type { LocalTabs } from "../local-tabs"
import { closeTab, takeConversationId } from "../internal"

export function closeConversation(tabs: LocalTabs, conversationId: string): LocalTabs {
  let current = tabs
  const closed = closeTab(tabs.conversations, conversationId, tabs.activeId, () => {
    const taken = takeConversationId(current)
    current = taken.tabs
    return taken.id
  })
  return {
    ...current,
    conversations: closed.items,
    activeId: closed.activeId,
  }
}
