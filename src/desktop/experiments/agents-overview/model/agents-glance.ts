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
 */
import { byRecency, type SessionSummary } from "../../../workspace/model/overview"
import { passes, type AgentsFilter } from "./filter"

/** An answered request kept in its place while it settles: where it stood when answered. */
export interface Held {
  readonly sessionId: string
  /** The session's `updatedAt` when it was answered, so it keeps its place. */
  readonly updatedAt: number
}

/** The overview's groups, as ids in the order they are shown, and what is only counted. */
export interface AgentsGlance {
  /** Waiting on the person, newest first, with held requests where they stood. */
  readonly needsYou: readonly string[]
  readonly working: readonly string[]
  /** Finished and not yet looked at, newest first. */
  readonly finished: readonly string[]
  /** Idle and already seen, newest first. */
  readonly earlier: readonly string[]
  /** Sessions the filter keeps out: the footer's figure. */
  readonly hidden: number
  /** How many wait on the person now, held requests not counted: the header's figure. */
  readonly waiting: number
}

/** What the glance asks of the moment and the sessions' tags, beside the filter. */
export interface GlanceContext {
  readonly filter: AgentsFilter
  readonly now: number
  readonly tagsOf: (session: SessionSummary) => readonly string[]
}

export function agentsGlance(
  sessions: readonly SessionSummary[],
  held: readonly Held[],
  { filter, now, tagsOf }: GlanceContext,
): AgentsGlance {
  const listed = new Map(sessions.map((session) => [session.id, session]))
  // A held request whose session is gone — archived meanwhile — is not kept;
  // one still settling stays whatever the filter says, so it can finish.
  const holding = new Map(
    held
      .filter((hold) => listed.has(hold.sessionId))
      .map((hold) => [hold.sessionId, hold]),
  )
  const free = sessions.filter((session) => !holding.has(session.id))
  const shown = free.filter((session) => passes(session, filter, now, tagsOf))
  const waitingNow = shown.filter((session) => session.status === "needs-you")
  const needsYou = [
    ...waitingNow.map((session) => ({ id: session.id, updatedAt: session.updatedAt })),
    ...[...holding.values()].map((hold) => ({
      id: hold.sessionId,
      updatedAt: hold.updatedAt,
    })),
  ]
    .sort((a, b) => b.updatedAt - a.updatedAt || (a.id < b.id ? -1 : 1))
    .map((entry) => entry.id)
  const ids = (status: SessionSummary["status"], unread?: boolean) =>
    shown
      .filter(
        (session) =>
          session.status === status && (unread === undefined || session.unread === unread),
      )
      .sort(byRecency)
      .map((session) => session.id)
  return {
    needsYou,
    working: ids("running"),
    finished: ids("idle", true),
    earlier: ids("idle", false),
    hidden: free.length - shown.length,
    waiting: waitingNow.length,
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
    a.waiting === b.waiting
  )
}

/** Every listed id in reading order: what the arrow keys walk. */
export function readingOrder(glance: AgentsGlance): readonly string[] {
  return [...glance.needsYou, ...glance.working, ...glance.finished, ...glance.earlier]
}

/**
 * The header's line under "Agents": how many wait on the person, how many are
 * working, how many finished unseen — each said only when there are some,
 * and only of what the filter lets through.
 */
export function glanceLine(glance: AgentsGlance): string {
  const parts = [
    glance.waiting > 0
      ? `${glance.waiting} ${glance.waiting === 1 ? "needs" : "need"} you`
      : "",
    glance.working.length > 0 ? `${glance.working.length} working` : "",
    glance.finished.length > 0 ? `${glance.finished.length} finished` : "",
    glance.earlier.length > 0 ? `${glance.earlier.length} earlier` : "",
  ].filter(Boolean)
  return parts.length > 0 ? parts.join(" · ") : "All quiet"
}
