/**
 * Times as the workspace says them: "now", "4m", "3h", "Yesterday", "Mon",
 * "Sep 19" beside a session; "started 22m ago" under a heading; "39s" beside
 * what an agent is doing. `now` is always passed in: nothing here reads a clock.
 */

const minute = 60_000
const hour = 60 * minute
const day = 24 * hour

const weekday = new Intl.DateTimeFormat("en-US", { weekday: "short" })
const monthDay = new Intl.DateTimeFormat("en-US", { month: "short", day: "numeric" })

function startOfDay(at: number): number {
  const date = new Date(at)
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime()
}

/** Beside a session: how long ago it last moved, as briefly as it can be said. */
export function sessionTime(at: number, now: number): string {
  const ago = Math.max(0, now - at)
  if (ago < minute) return "now"
  if (ago < hour) return `${Math.floor(ago / minute)}m`
  if (ago < 12 * hour || startOfDay(at) === startOfDay(now))
    return `${Math.floor(ago / hour)}h`
  const days = Math.round((startOfDay(now) - startOfDay(at)) / day)
  if (days === 1) return "Yesterday"
  if (days < 7) return weekday.format(at)
  return monthDay.format(at)
}

/** Under a conversation's heading: when it began. `lead` capitalises it when nothing comes before it. */
export function startedLabel(at: number, now: number, lead = false): string {
  const when = sessionTime(at, now)
  const phrase =
    when === "now"
      ? "just now"
      : /^\d+[mh]$/.test(when)
        ? `${when} ago`
        : when === "Yesterday"
          ? "yesterday"
          : when
  return `${lead ? "Started" : "started"} ${phrase}`
}

/** Beside what an agent is doing: how long it has been at it. */
export function elapsed(since: number, now: number): string {
  const seconds = Math.max(0, Math.round((now - since) / 1000))
  return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`
}
