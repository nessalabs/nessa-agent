/**
 * The source's replacements carry a revision: its own count of changes to one
 * session's summary, or to its conversation. Of two replacements for the same
 * thing, the one with the larger revision is the newer, whichever channel
 * brought it and whenever it arrived — a read answered late, an update that
 * overtook it, an index read while updates were already flowing.
 *
 * The source counts from 1. Revision 0 is the window's own: a session whose
 * first message is on its way, which the source has not spoken of yet.
 *
 * Two counters per session, never compared with each other: the summary's
 * and the conversation's. A removal is counted with the summary — it is the
 * summary's next revision — so it is weighed against summaries only
 * (`usecases/updates.ts`, "a removal's revision" in its tests).
 */
export interface Revised {
  readonly revision: number
}

/**
 * Whether `incoming` takes the place of `held`: it is newer. An equal revision
 * is the same replacement again, and changes nothing — not even what the
 * person changed on top of it while the source had yet to answer.
 */
export function supersedes(incoming: Revised, held: Revised | undefined): boolean {
  return held === undefined || incoming.revision > held.revision
}

/**
 * Whether a revision is one the source could have sent: a whole number from 1,
 * small enough that the next one is still larger.
 * Anything else — 0, negative, fractional, not a number — is refused at the
 * door, so a malformed replacement can neither pose as the window's own nor
 * become one nothing can supersede.
 */
export function fromSource(incoming: Revised): boolean {
  return Number.isSafeInteger(incoming.revision) && incoming.revision >= 1
}

/** Whether the source has spoken of this yet: anything it sent is revision 1 or later. */
export function knownToSource(held: Revised): boolean {
  return held.revision > 0
}
