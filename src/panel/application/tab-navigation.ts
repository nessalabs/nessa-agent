/**
 * Moving about the tab strip, which is one strip.
 *
 * The update tab sits in it beside the conversations, and the shortcuts used to
 * navigate `chat.conversations` alone — so with `[A, B, Update]` showing, next
 * from B went to A rather than to Update, previous from Update went to A rather
 * than B, and an indexed shortcut aimed at Update's position found nothing
 * there. The strip a person sees and the strip the keyboard moves through have
 * to be the same list.
 *
 * Ids rather than anything richer: what a tab *is* belongs to whoever renders
 * it, and all this owns is which one comes next.
 */

/** The tab a step in `direction` lands on, wrapping at either end. */
export function tabAfter(
  ids: readonly string[],
  selected: string,
  direction: -1 | 1,
): string | undefined {
  if (ids.length === 0) return undefined
  const at = ids.indexOf(selected)
  // A selection that is not in the strip — a conversation closing as the key is
  // pressed — starts from the end the step is coming from, so the shortcut
  // still moves rather than doing nothing.
  const from = at === -1 ? (direction === 1 ? -1 : 0) : at
  const next = (from + direction + ids.length) % ids.length
  return ids[next]
}

/** The tab at a position, for the numbered shortcuts. */
export function tabAt(ids: readonly string[], index: number): string | undefined {
  return ids[index]
}
