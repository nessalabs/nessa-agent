/** What setup may say about linger. The host decides; this file only reads it.
 *
 * `enabled` is the only tag that means the gateway keeps running after logout,
 * and it is present only when logind's confirming read said so.
 */

export const LINGER_SHOWN = [
  "not-applicable",
  "offer",
  "enabled",
  "declined",
  "refused",
  "waiting",
  "unsupported",
  "unconfirmed",
] as const

export type LingerShown = (typeof LINGER_SHOWN)[number]

export const LINGER_AUDIT = ["not-required", "recorded", "failed"] as const

export type LingerAudit = (typeof LINGER_AUDIT)[number]

export interface LingerView {
  readonly shown: LingerShown
  readonly audit: LingerAudit
}

function isListed<T extends string>(values: readonly T[], value: unknown): value is T {
  return typeof value === "string" && values.includes(value as T)
}

/** A view this build understands. Anything else, including a tag inherited
 * from the prototype, is not a view and cannot become a claim. */
export function parseLingerView(value: unknown): LingerView | undefined {
  if (typeof value !== "object" || value === null) return undefined
  if (!Object.hasOwn(value, "shown") || !Object.hasOwn(value, "audit")) return undefined
  const record = value as { shown: unknown; audit: unknown }
  if (!isListed(LINGER_SHOWN, record.shown) || !isListed(LINGER_AUDIT, record.audit)) {
    return undefined
  }
  return { shown: record.shown, audit: record.audit }
}
