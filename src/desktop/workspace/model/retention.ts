/**
 * How much the window keeps of what it is not showing, as data: the source
 * holds every session, so the window only keeps what makes going back
 * instant, and forgets the rest to be read again when asked.
 *
 * - **Conversations**: every one a pane shows; those the open Agents
 *   overview shows, up to a bound, most recently active first; and the few
 *   most recently active of the rest (by the summary's `updatedAt`). Another
 *   is read afresh when a pane or the overview shows it.
 * - **Removals**: the newest few hundred, each with the revision it was taken
 *   at, so an older read answered late cannot bring a session back. A read
 *   older than that many removals is let go by the next read of the
 *   index, which is the resync (`application/ports.ts`).
 */
export const retention = {
  /** Conversations kept for sessions nothing on screen shows. */
  unshownConversations: 8,
  /** Conversations the open overview shows that are read and kept, at most. */
  overviewConversations: 24,
  /** Removals remembered. */
  removals: 256,
} as const

/** A session the source took out, and the summary revision it did so at. */
export interface Removal {
  readonly sessionId: string
  readonly revision: number
}

/**
 * The held conversations to keep: every one `shown`, and the `limit` most
 * recent of the rest by `recency`. The same record when none is let go.
 */
export function keptConversations<T>(
  held: Readonly<Record<string, T>>,
  shown: ReadonlySet<string>,
  recency: (sessionId: string) => number,
  limit: number = retention.unshownConversations,
): Readonly<Record<string, T>> {
  const unshown = Object.keys(held).filter((id) => !shown.has(id))
  if (unshown.length <= limit) return held
  const dropped = new Set(
    unshown.sort((a, b) => recency(b) - recency(a) || (a < b ? -1 : 1)).slice(limit),
  )
  return Object.fromEntries(Object.entries(held).filter(([id]) => !dropped.has(id)))
}

/** The revision a session was removed at, if it is remembered. */
export function removedAt(
  removals: readonly Removal[],
  sessionId: string,
): number | undefined {
  return removals.find((removal) => removal.sessionId === sessionId)?.revision
}

/**
 * Remembers a removal as the newest, keeping the later of two revisions for
 * one session, and forgets the oldest past `limit`.
 */
export function remembered(
  removals: readonly Removal[],
  removal: Removal,
  limit: number = retention.removals,
): readonly Removal[] {
  const earlier = removedAt(removals, removal.sessionId)
  const revision = Math.max(removal.revision, earlier ?? removal.revision)
  const rest = removals.filter((each) => each.sessionId !== removal.sessionId)
  return [...rest, { sessionId: removal.sessionId, revision }].slice(-limit)
}

/** Forgets a session's removal — the source listed it again. The same list when none was held. */
export function forgotten(
  removals: readonly Removal[],
  sessionId: string,
): readonly Removal[] {
  return removedAt(removals, sessionId) === undefined
    ? removals
    : removals.filter((removal) => removal.sessionId !== sessionId)
}
