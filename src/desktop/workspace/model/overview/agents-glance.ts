/**
 * What the agents overview shows at a glance, and in what order: the
 * sessions waiting on the person first, then those working, then those that
 * finished and have not been looked at, then — when the filter lets idle
 * sessions through — the rest, earlier. Everything here is ids and counts,
 * so a view renders again only when a session changes group or place, never
 * for a streamed word.
 *
 * A request the person has just answered is **held** in its place among the
 * waiting until its settle has played, even though the source already moved
 * the session on: the list re-flows once, when the settle ends, rather than
 * the row jumping into "Working" under the pointer.
 *
 * The person can narrow it to one **group** — only what needs them, only
 * what is working, only what finished, only what is earlier — by choosing
 * its count in the header. The counts stay what the filter lets through, so
 * every other group can still be chosen from the same line; only the list
 * narrows, and what it says it keeps out is what the filter keeps out of
 * that group.
 */
import { byRecency, type SessionSummary } from "../workspace-index"
import { passes, tagsOf as sessionTagsOf, type AgentsFilter, type TagsOf } from "./filter"

/** A group of the overview, as its header counts it and the person can show it alone. */
export type AgentsGroup = "needsYou" | "working" | "finished" | "earlier"

/** The groups, in the order the overview shows and counts them. */
export const agentsGroups: readonly AgentsGroup[] = [
  "needsYou",
  "working",
  "finished",
  "earlier",
]

/** The group a session belongs in by where it stands: the one rule every listing asks. */
export function groupOf(session: SessionSummary): AgentsGroup {
  switch (session.status) {
    case "needs-you":
      return "needsYou"
    case "running":
      return "working"
    case "idle":
      return session.unread ? "finished" : "earlier"
  }
}

/** Whether a session is in the group shown: any, when none is chosen. */
export function inGroup(session: SessionSummary, group: AgentsGroup | null): boolean {
  return group === null || groupOf(session) === group
}

/** An answered request kept in its place while it settles: where it stood when answered. */
export interface Held {
  readonly sessionId: string
  /** The session's `updatedAt` when it was answered, so it keeps its place. */
  readonly updatedAt: number
}

/**
 * The overview's groups, as ids in the order they are shown, and what is
 * only counted. With a group chosen, the others list nothing — but for the
 * session being looked at — and are still counted.
 */
export interface AgentsGlance {
  /** Waiting on the person, newest first, with held requests where they stood. */
  readonly needsYou: readonly string[]
  readonly working: readonly string[]
  /** Finished and not yet looked at, newest first. */
  readonly finished: readonly string[]
  /** Idle and already seen, newest first. */
  readonly earlier: readonly string[]
  /** Sessions the filter keeps out of the group shown (of every group, with none chosen): the footer's figure. */
  readonly hidden: number
  /**
   * The header's figures: how many of each group the filter lets through,
   * whichever group is shown. Needs you counts what waits now, held
   * requests not counted.
   */
  readonly counts: Readonly<Record<AgentsGroup, number>>
  /** The group shown alone; `null` shows every group. */
  readonly group: AgentsGroup | null
}

/** What the glance asks of the moment and the sessions' tags, beside the filter. */
export interface GlanceContext {
  readonly filter: AgentsFilter
  /** The group shown alone (`OverviewState.group`); every group when absent or `null`. */
  readonly group?: AgentsGroup | null
  readonly now: number
  /** The tags a session carries; the summary's own (`tagsOf`) unless a test says otherwise. */
  readonly tagsOf?: TagsOf
  /**
   * The session the person is looking at, listed whatever the filter says —
   * one that finishes while its peek is open, or while a reply is written
   * to it, does not vanish from under them. It goes once they move on.
   */
  readonly looking?: string | null
}

export function agentsGlance(
  sessions: readonly SessionSummary[],
  held: readonly Held[],
  { filter, group = null, now, tagsOf = sessionTagsOf, looking = null }: GlanceContext,
): AgentsGlance {
  const listed = new Map(sessions.map((session) => [session.id, session]))
  // A held request whose session is gone — archived meanwhile — is not kept;
  // one still settling stays whatever the filter says, so it can finish —
  // among the waiting, while they are shown.
  const holding = new Map(
    held
      .filter((hold) => listed.has(hold.sessionId))
      .map((hold) => [hold.sessionId, hold]),
  )
  const free = sessions.filter((session) => !holding.has(session.id))
  const passing = free.filter(
    (session) => session.id === looking || passes(session, filter, now, tagsOf),
  )
  // What the filter lets through, counted whatever the group; then what the group lists.
  const counted = (status: SessionSummary["status"], unread?: boolean) =>
    passing
      .filter(
        (session) =>
          session.status === status &&
          (unread === undefined || session.unread === unread),
      )
      .sort(byRecency)
  const waitingNow = counted("needs-you")
  const running = counted("running")
  const done = counted("idle", true)
  const seen = counted("idle", false)
  const shows = (id: string, of: AgentsGroup) =>
    group === null || group === of || id === looking
  const ids = (of: AgentsGroup, from: readonly SessionSummary[]) =>
    from.filter((session) => shows(session.id, of)).map((session) => session.id)
  const needsYou = [
    ...waitingNow.map((session) => ({ id: session.id, updatedAt: session.updatedAt })),
    ...[...holding.values()].map((hold) => ({
      id: hold.sessionId,
      updatedAt: hold.updatedAt,
    })),
  ]
    .filter((entry) => shows(entry.id, "needsYou"))
    .sort((a, b) => b.updatedAt - a.updatedAt || (a.id < b.id ? -1 : 1))
    .map((entry) => entry.id)
  return {
    needsYou,
    working: ids("working", running),
    finished: ids("finished", done),
    earlier: ids("earlier", seen),
    hidden: free.filter(
      (session) =>
        session.id !== looking &&
        !passes(session, filter, now, tagsOf) &&
        inGroup(session, group),
    ).length,
    counts: {
      needsYou: waitingNow.length,
      working: running.length,
      finished: done.length,
      earlier: seen.length,
    },
    group,
  }
}

/** Whether two glances list the same ids in the same places and count the same. */
export function sameGlance(a: AgentsGlance, b: AgentsGlance): boolean {
  const same = (x: readonly string[], y: readonly string[]) =>
    x.length === y.length && x.every((id, index) => id === y[index])
  return (
    same(a.needsYou, b.needsYou) &&
    same(a.working, b.working) &&
    same(a.finished, b.finished) &&
    same(a.earlier, b.earlier) &&
    a.hidden === b.hidden &&
    a.group === b.group &&
    agentsGroups.every((group) => a.counts[group] === b.counts[group])
  )
}

/** Every listed id in reading order: what the arrow keys walk. */
export function readingOrder(glance: AgentsGlance): readonly string[] {
  return [...glance.needsYou, ...glance.working, ...glance.finished, ...glance.earlier]
}

/** One of the header's counts: the group it shows alone, and how it reads. */
export interface GlanceCount {
  readonly group: AgentsGroup
  readonly count: number
  readonly label: string
}

const countLabels: Readonly<Record<AgentsGroup, (count: number) => string>> = {
  needsYou: (count) => `${count} ${count === 1 ? "needs" : "need"} you`,
  working: (count) => `${count} working`,
  finished: (count) => `${count} finished`,
  earlier: (count) => `${count} earlier`,
}

/**
 * The header's counts under "Agents": how many wait on the person, how many
 * are working, finished unseen and earlier — each said only when there are
 * some, and only of what the filter lets through — and the group shown
 * alone always, at nought too, so it can be let go where it was chosen.
 * None at all reads "All quiet" (`quietLine`).
 */
export function glanceCounts(glance: AgentsGlance): readonly GlanceCount[] {
  return agentsGroups
    .filter((group) => glance.counts[group] > 0 || glance.group === group)
    .map((group) => ({
      group,
      count: glance.counts[group],
      label: countLabels[group](glance.counts[group]),
    }))
}

/** What the header says when there is nothing to count. */
export const quietLine = "All quiet"
