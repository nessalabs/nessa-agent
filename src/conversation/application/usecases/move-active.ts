import type { LocalTabs } from "../local-tabs"

/** Activate an adjacent open conversation, wrapping at either end without changing drafts. */
export function moveActive(tabs: LocalTabs, direction: -1 | 1): LocalTabs {
  const index = tabs.conversations.findIndex((item) => item.id === tabs.activeId)
  if (index < 0 || tabs.conversations.length < 2) return tabs
  const next = (index + direction + tabs.conversations.length) % tabs.conversations.length
  const target = tabs.conversations[next]
  return target ? { ...tabs, activeId: target.id } : tabs
}
