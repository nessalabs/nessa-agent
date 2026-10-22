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
  /** Absolute reset instant from the gateway. Display uses this. */
  readonly resetsAtMs?: number
  /**
   * That instant on the client clock, from the same relationship as a
   * permission deadline: as far after the view arrived as `resetsAtMs` was
   * after the view's `serverNowMs`. Absent when this notice was already
   * built on `nowMs`'s clock. The scheduling-failure decision uses it.
   */
  readonly resetsAtClientMs?: number
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
 * The reset on the clock that received the view.
 *
 * `resetsAtMs` and `serverNowMs` are the gateway's. `receivedAtMs` is the
 * client's, read as the view arrived. The result is as far after that
 * arrival as the reset was after `serverNowMs` — `clientDueAtMs`'s
 * relationship — so a later reading of the client clock agrees with the
 * gateway about whether the reset is still ahead. The absolute instant
 * stays on the notice for the words under the title.
 */
export function usageLimitOnClientClock(
  limit: UsageLimitNotice,
  serverNowMs: number,
  receivedAtMs: number,
): UsageLimitNotice {
  if (limit.resetsAtMs === undefined) return limit
  return {
    ...limit,
    resetsAtClientMs: receivedAtMs + (limit.resetsAtMs - serverNowMs),
  }
}

/**
 * The line under the card's title. `preferenceOn` is the confirmed gateway
 * preference. Auto-resuming is copied only when a schedule has been accepted.
 * A confirmed preference that is on, with a future reset and no schedule, is
 * a scheduling failure. Anything else with no trustworthy instant says the
 * reset time is unknown. `nowMs` is the client clock. A notice that crossed
 * from the gateway compares `resetsAtClientMs`; one already on this clock
 * compares `resetsAtMs`.
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
  const due = limit.resetsAtClientMs ?? limit.resetsAtMs
  if (preferenceOn && due > nowMs) return "Scheduling failed."
  return `Resets ${when}`
}
