/**
 * Which sessions the agents overview lists: what is going on (waiting on the
 * person or working — where it opens), or all of them; within a span of time
 * by when each last moved; and — once sessions carry them — by tag. The
 * header's counts follow the same choice, since they are counted from what
 * this lets through.
 */
import type { SessionSummary } from "../workspace-index"

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
 * The tags sessions carry, and the ones a session carries: none yet — the
 * source's summary has no tags. This is the seam a tagged summary fills; the
 * filter and its menu already read it.
 */
export const sessionTags: readonly SessionTag[] = []
const untagged: readonly string[] = []
export const tagsOf = (_session: SessionSummary): readonly string[] => untagged

/** The tags a session carries, as the filter reads them; `tagsOf` unless a test says otherwise. */
export type TagsOf = (session: SessionSummary) => readonly string[]

/** Whether a session carries a tag the filter names; any session, when it names none. */
function tagged(session: SessionSummary, filter: AgentsFilter, tags: TagsOf): boolean {
  if (filter.tags.length === 0) return true
  const carried = tags(session)
  return filter.tags.some((tag) => carried.includes(tag))
}

/**
 * Whether the filter lists a session waiting on the person: whatever the
 * scope and the span of time — a request is never hidden for being old —
 * but only under a tag it names. What the overview shows, and so what it
 * answers (`overviewShows`), asks this.
 */
export function listsWaiting(
  session: SessionSummary,
  filter: AgentsFilter,
  tags: TagsOf = tagsOf,
): boolean {
  return session.status === "needs-you" && tagged(session, filter, tags)
}

/** Whether a session passes the filter at `now`. */
export function passes(
  session: SessionSummary,
  filter: AgentsFilter,
  now: number,
  tags: TagsOf = tagsOf,
): boolean {
  if (session.status === "needs-you") return listsWaiting(session, filter, tags)
  if (filter.scope === "ongoing" && session.status === "idle") return false
  if (session.updatedAt < rangeStart(filter.range, now)) return false
  return tagged(session, filter, tags)
}

/** The sessions a filter lets through, in the order given. */
export function filtered(
  sessions: readonly SessionSummary[],
  filter: AgentsFilter,
  now: number,
  tags: TagsOf = tagsOf,
): SessionSummary[] {
  return sessions.filter((session) => passes(session, filter, now, tags))
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
    // Only tags sessions can carry (`sessionTags`, none yet): a kept tag
    // this build does not know would hide every session behind a menu that
    // cannot show it.
    tags: Array.isArray(tags)
      ? tags.filter(
          (tag): tag is string =>
            typeof tag === "string" && sessionTags.some((known) => known.id === tag),
        )
      : [],
  }
}

export function serializeFilter(filter: AgentsFilter): string {
  return JSON.stringify(filter)
}
