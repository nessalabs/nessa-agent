import type { LocalTabs } from "../local-tabs"

export function setActive(tabs: LocalTabs, conversationId: string): LocalTabs {
  if (!tabs.conversations.some((item) => item.id === conversationId)) return tabs
  return { ...tabs, activeId: conversationId }
}
