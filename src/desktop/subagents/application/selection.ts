/**
 * Which subagent a conversation's panel shows. One per conversation, shared
 * by the panel and `useOpenSubagent`. Other verticals do not read this map;
 * they call `useOpenSubagent`, which writes it.
 */

const chosen = new Map<string, string>()
const listeners = new Set<() => void>()

export function selectedSubagent(sessionId: string): string | null {
  return chosen.get(sessionId) ?? null
}

/** Shows `subagentId` for `sessionId`, or the list when `subagentId` is null. */
export function chooseSubagent(sessionId: string, subagentId: string | null): void {
  if (subagentId === null) chosen.delete(sessionId)
  else chosen.set(sessionId, subagentId)
  for (const listener of listeners) listener()
}

export function subscribeSubagentChoice(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/** Forgets every choice. Tests call it so one file does not leak into the next. */
export function clearSubagentChoices(): void {
  chosen.clear()
}
