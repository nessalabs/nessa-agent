/**
 * Which sessions the agents overview lists: what is going on (waiting on the
 * person or working — where it opens), or all of them; within a span of time
 * by when each last moved; and — once sessions carry them — by tag. The
 * header's counts follow the same choice, since they are counted from what
 * this lets through.
 */
import type { SessionSummary } from "../../../workspace/model/workspace-index"

/** `ongoing`: waiting on the person or working. `all`: every session, idle ones too. */
export type AgentsScope = "ongoing" | "all"

/** How far back a session's last change may be: any time, or since a moment before `now`. */
export type AgentsRange = "any" | "today" | "week" | "month"

/**
 * A tag a session could carry. Sessions carry none yet — the source's
 * summary has no tags — so the list of tags is empty and the Tags menu says
 * so; a filter naming tags lets through only sessions carrying one.
 */
export interface SessionTag {
  readonly id: string
  readonly label: string
}

export interface AgentsFilter {
  readonly scope: AgentsScope
  readonly range: AgentsRange
  /** Tag ids; empty lets every session through. */
  readonly tags: readonly string[]
}

/** Where the overview opens: what is going on, at any time, any tag. */
export const defaultFilter: AgentsFilter = { scope: "ongoing", range: "any", tags: [] }

export const scopeLabels: Readonly<Record<AgentsScope, string>> = {
  ongoing: "Ongoing",
  all: "All",
}

export const rangeLabels: Readonly<Record<AgentsRange, string>> = {
  any: "Any time",
  today: "Today",
  week: "Last 7 days",
  month: "Last 30 days",
}

/** The choices, in the order the menu offers them. */
export const scopes: readonly AgentsScope[] = ["ongoing", "all"]
export const ranges: readonly AgentsRange[] = ["any", "today", "week", "month"]

const day = 24 * 60 * 60 * 1000

function startOfDay(at: number): number {
  const date = new Date(at)
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime()
}

/** The earliest `updatedAt` a range lets through at `now`; `-Infinity` for any time. */
export function rangeStart(range: AgentsRange, now: number): number {
  switch (range) {
    case "any":
      return -Infinity
    case "today":
      return startOfDay(now)
    case "week":
      return startOfDay(now) - 6 * day
    case "month":
      return startOfDay(now) - 29 * day
  }
}

/**
 * Whether a session passes the filter at `now`. What waits on the person
 * passes any span of time: a request is never hidden for being old.
 */
export function passes(
  session: SessionSummary,
  filter: AgentsFilter,
  now: number,
  tagsOf: (session: SessionSummary) => readonly string[],
): boolean {
  if (filter.scope === "ongoing" && session.status === "idle") return false
  if (session.status !== "needs-you" && session.updatedAt < rangeStart(filter.range, now))
    return false
  if (filter.tags.length > 0) {
    const carried = tagsOf(session)
    if (!filter.tags.some((tag) => carried.includes(tag))) return false
  }
  return true
}

/** The sessions a filter lets through, in the order given. */
export function filtered(
  sessions: readonly SessionSummary[],
  filter: AgentsFilter,
  now: number,
  tagsOf: (session: SessionSummary) => readonly string[],
): SessionSummary[] {
  return sessions.filter((session) => passes(session, filter, now, tagsOf))
}

/** What the filter button says: the scope, and the span when one is chosen. */
export function filterLabel(filter: AgentsFilter): string {
  const scope = scopeLabels[filter.scope]
  return filter.range === "any" ? scope : `${scope} · ${rangeLabels[filter.range]}`
}

/** A stored filter, read back: anything unknown or malformed falls back, field by field. */
export function parseFilter(stored: unknown): AgentsFilter {
  if (typeof stored !== "string") return defaultFilter
  let value: unknown
  try {
    value = JSON.parse(stored)
  } catch {
    return defaultFilter
  }
  if (typeof value !== "object" || value === null) return defaultFilter
  const read = (key: string): unknown =>
    Object.hasOwn(value, key) ? (value as Record<string, unknown>)[key] : undefined
  const scope = read("scope")
  const range = read("range")
  const tags = read("tags")
  return {
    scope: scopes.find((each) => each === scope) ?? defaultFilter.scope,
    range: ranges.find((each) => each === range) ?? defaultFilter.range,
    tags: Array.isArray(tags)
      ? tags.filter((tag): tag is string => typeof tag === "string")
      : [],
  }
}

export function serializeFilter(filter: AgentsFilter): string {
  return JSON.stringify(filter)
}
