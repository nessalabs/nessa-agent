/**
 * Counts and times as the window says them, at any size: "840", "1.3K",
 * "10.5K"; "now", "4m", "2h 10m"; "38s". Anything that shows a number of
 * things or an age reads it through here.
 */

/** How long ago, briefly: "now", "4m", "2h 10m". */
export function ago(at: number, now: number): string {
  const minutes = Math.floor(Math.max(0, now - at) / 60_000)
  if (minutes < 1) return "now"
  if (minutes < 60) return `${minutes}m`
  const hours = Math.floor(minutes / 60)
  const rest = minutes % 60
  return rest === 0 ? `${hours}h` : `${hours}h ${rest}m`
}

/** How long something has been going: "38s", "4m 12s". */
export function running(since: number, now: number): string {
  const seconds = Math.max(0, Math.round((now - since) / 1000))
  if (seconds < 60) return `${seconds}s`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`
  return ago(since, now)
}

const compact = new Intl.NumberFormat("en", {
  notation: "compact",
  maximumFractionDigits: 1,
})
const whole = new Intl.NumberFormat("en")

/**
 * A count at any size, as briefly as it reads: "840", "1.3K", "10.5K",
 * "100K", "1.2M". Where the exact number matters, pair it with `exact`
 * (a tooltip, a label).
 */
export function count(value: number): string {
  return value < 1000 ? whole.format(value) : compact.format(value)
}

/** A count in full: "10,482". */
export const exact = (value: number) => whole.format(value)

/** "1 file", "1.2K files". */
export function plural(value: number, one: string, many = `${one}s`): string {
  return `${count(value)} ${value === 1 ? one : many}`
}
