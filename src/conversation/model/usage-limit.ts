/**
 * A provider usage limit as the transcript shows it. The gateway publishes
 * the classification; this module only decides the card's words.
 *
 * `autoResuming` is true only after a continuation has been accepted.
 * The checkbox is the gateway preference, not this flag.
 */

export type UsageLimitNotice = {
  readonly provider: "claude" | "codex"
  readonly recovery: "timed_reset" | "user_action" | "unknown"
  readonly resetsAtMs?: number
  readonly autoResuming?: boolean
}

export const USAGE_LIMIT_TITLE = "Usage limit reached"

export const USAGE_LIMIT_TOGGLE = "Auto-continue this project's threads when limits reset"

const monthDay = new Intl.DateTimeFormat("en-US", {
  month: "short",
  day: "numeric",
})
const clock = new Intl.DateTimeFormat("en-US", {
  hour: "numeric",
  minute: "2-digit",
})

/** "Oct 21 at 12:00 PM", in the environment's time zone. */
export function formatUsageLimitReset(resetsAtMs: number): string {
  return `${monthDay.format(resetsAtMs)} at ${clock.format(resetsAtMs)}`
}

/**
 * The line under the card's title. Auto-resuming is copied only when a
 * schedule has been accepted. A preference that is on, with a future reset
 * and no schedule, is a scheduling failure. Anything else with no trustworthy
 * instant says the reset time is unknown.
 */
export function usageLimitDetail(
  limit: UsageLimitNotice,
  preferenceOn: boolean,
  nowMs: number,
): string {
  if (limit.recovery === "user_action")
    return "An account change is required before this thread can continue."
  if (limit.recovery !== "timed_reset" || limit.resetsAtMs === undefined)
    return "Reset time unknown"
  const when = formatUsageLimitReset(limit.resetsAtMs)
  if (limit.autoResuming === true) return `Auto-resuming ${when}`
  if (preferenceOn && limit.resetsAtMs > nowMs) return "Scheduling failed."
  return `Resets ${when}`
}
