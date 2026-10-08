/** What setup may say about linger. The host decides; this file only reads it.
 *
 * `enabled` is the only tag that means the gateway keeps running after logout,
 * and it is present only when the confirming read said so.
 */

export const LINGER_SHOWN = [
  "not-applicable",
  "offer",
  "enabled",
  "refused",
  "failed",
  "unsupported",
] as const

export type LingerShown = (typeof LINGER_SHOWN)[number]

export interface LingerView {
  readonly shown: LingerShown
}

function isShown(value: unknown): value is LingerShown {
  return typeof value === "string" && LINGER_SHOWN.includes(value as LingerShown)
}

/** A view this build understands. Anything else, including a tag inherited
 * from the prototype, is not a view and cannot become a claim. */
export function parseLingerView(value: unknown): LingerView | undefined {
  if (typeof value !== "object" || value === null) return undefined
  if (!Object.hasOwn(value, "shown")) return undefined
  const shown = (value as { shown: unknown }).shown
  if (!isShown(shown)) return undefined
  return { shown }
}
