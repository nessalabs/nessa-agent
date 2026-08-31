import { conversation, type Conversation } from "../../model"

/** The open conversation. The strip never empties; a missing write is a bug. */
export function conversationInStrip(
  items: Conversation[],
  activeId: string,
): Conversation {
  const found = items.find((item) => item.id === activeId) ?? items[0]
  if (!found) throw new Error("conversation strip is empty")
  return found
}

/**
 * Removes a conversation from the strip. The strip is the app's only
 * navigation, so it never empties: closing the last tab replaces it.
 */
export function closeInStrip(
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
