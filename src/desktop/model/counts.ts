/**
 * Counts as the window says them: "840", "1.3K", "1 agent", "2 agents".
 * A time is the workspace's (`workspace/model/time-labels.ts`); this is not
 * a second formatter for one.
 */

const compact = new Intl.NumberFormat("en", {
  notation: "compact",
  maximumFractionDigits: 1,
})
const whole = new Intl.NumberFormat("en")

/** A count as briefly as it reads: "840", "1.3K", "10.5K". */
export function countLabel(value: number): string {
  return value < 1000 ? whole.format(value) : compact.format(value)
}

/** A count in full: "10,482". */
export function exactCount(value: number): string {
  return whole.format(value)
}

/**
 * A count and its word: "1 agent", "1.3K agents". `many` defaults to `one`
 * with an s.
 */
export function plural(value: number, one: string, many = `${one}s`): string {
  return `${countLabel(value)} ${value === 1 ? one : many}`
}

/** A count beside a word that does not change with it: "1 working", "2 working". */
export function counted(value: number, word: string): string {
  return `${countLabel(value)} ${word}`
}
