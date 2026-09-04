import { conversation, type Conversation } from "../../model"

/** Close a tab; always leave at least one conversation open. */
export function closeTab(
  items: Conversation[],
  id: string,
  activeId: string,
  nextId: () => string,
): { items: Conversation[]; activeId: string } {
  if (!items.some((item) => item.id === id)) return { items, activeId }
  if (items.length === 1) {
    const next = conversation(nextId())
    return { items: [next], activeId: next.id }
  }
  const remaining = items.filter((item) => item.id !== id)
  if (id !== activeId) return { items: remaining, activeId }
  const closed = items.findIndex((item) => item.id === id)
  const next = remaining[Math.min(closed, remaining.length - 1)]
  if (!next) {
    const created = conversation(nextId())
    return { items: [created], activeId: created.id }
  }
  return { items: remaining, activeId: next.id }
}
