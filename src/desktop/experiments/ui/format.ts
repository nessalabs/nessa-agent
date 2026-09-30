/** Scores and money as the experiment views say them; counts and times are the window's (`../../ui/format`). */

const minus = "−"

/** A score: "71.6". */
export const score = (value: number) => value.toFixed(1)

/** A change in points, always signed: "+1.8", "−0.4", "±0.0". */
export function points(value: number): string {
  const rounded = Math.round(value * 10) / 10
  if (rounded === 0) return "±0.0"
  return `${rounded > 0 ? "+" : minus}${Math.abs(rounded).toFixed(1)}`
}

/** Dollars per task: "$0.049". */
export const dollars = (value: number) => `$${value.toFixed(3)}`

/** A relative change: "+6%", "−38%". */
export function percentChange(from: number, to: number): string {
  const change = Math.round(((to - from) / from) * 100)
  if (change === 0) return "±0%"
  return `${change > 0 ? "+" : minus}${Math.abs(change)}%`
}
