/**
 * How sessions are gathered for the sidebar and the session list: by status
 * into "Needs you", "Running" and "Earlier"; by channel, with only the few a
 * person needs disclosed; and by what a search asks for.
 */
import {
  agentName,
  agentOf,
  byRecency,
  modelName,
  type SessionStatus,
  type SessionSummary,
} from "./workspace-index"

/**
 * What each state is called wherever it is named — the list's groups, a
 * row's mark, the switcher's heading — so it is said one way everywhere.
 */
export const statusLabels: Readonly<Record<SessionStatus, string>> = {
  "needs-you": "Needs you",
  running: "Running",
  idle: "Earlier",
}

/** The list's groups, in the order they are shown. */
const groupOrder: readonly SessionStatus[] = ["needs-you", "running", "idle"]

/** What the session list shows: one channel's sessions. */
export interface SessionView {
  readonly channelId: string
}

export function inView(session: SessionSummary, view: SessionView): boolean {
  return session.channelId === view.channelId
}

interface StatusGroup {
  /** The group's own name among its siblings. */
  readonly id: string
  readonly label: string
  readonly sessions: readonly SessionSummary[]
}

/**
 * Sessions by status in the list's order, newest first in each; empty groups
 * are left out. With `runningFirst` off (Settings › Workspace › Sessions),
 * running sessions are not kept above the rest: "Needs you", then every other
 * session in one group, newest first.
 */
export function groupByStatus(
  sessions: readonly SessionSummary[],
  { runningFirst = true }: { runningFirst?: boolean } = {},
): StatusGroup[] {
  const newest = [...sessions].sort(byRecency)
  const groups: StatusGroup[] = runningFirst
    ? groupOrder.map((status) => ({
        id: status,
        label: statusLabels[status],
        sessions: newest.filter((session) => session.status === status),
      }))
    : [
        {
          id: "needs-you",
          label: statusLabels["needs-you"],
          sessions: newest.filter((session) => session.status === "needs-you"),
        },
        {
          id: "rest",
          label: "Sessions",
          sessions: newest.filter((session) => session.status !== "needs-you"),
        },
      ]
  return groups.filter((group) => group.sessions.length > 0)
}

/** Whether a session answers the list's search, by title, preview, model or agent. */
export function matchesSearch(session: SessionSummary, query: string): boolean {
  const wanted = query.trim().toLowerCase()
  if (!wanted) return true
  return `${session.title} ${session.preview} ${modelName(session.model)} ${agentName(agentOf(session.model))}`
    .toLowerCase()
    .includes(wanted)
}

/** The session most worth opening: one waiting on the person, then one running, then the newest. */
export function mostPressing(
  sessions: readonly SessionSummary[],
): SessionSummary | undefined {
  const newest = [...sessions].sort(byRecency)
  return (
    newest.find((session) => session.status === "needs-you") ??
    newest.find((session) => session.status === "running") ??
    newest[0]
  )
}

/** How many sessions a channel discloses in the sidebar before "Show all". */
export const branchCap = 3

/**
 * The sessions a channel's branch shows, newest first: the newest few, and
 * any other that is on screen or waiting on the person, so neither is ever
 * hidden behind "Show all".
 */
export function branchSessions(
  sessions: readonly SessionSummary[],
  { showAll, shown }: { showAll: boolean; shown: ReadonlySet<string> },
): { visible: SessionSummary[]; hidden: number } {
  const newest = [...sessions].sort(byRecency)
  const visible = showAll
    ? newest
    : newest.filter(
        (session, index) =>
          index < branchCap || shown.has(session.id) || session.status === "needs-you",
      )
  return { visible, hidden: newest.length - visible.length }
}

/** What a channel's row says about its sessions without disclosing them. */
export interface ChannelActivity {
  readonly waiting: number
  readonly running: boolean
  readonly unread: boolean
}

export function channelActivity(sessions: readonly SessionSummary[]): ChannelActivity {
  return {
    waiting: sessions.filter((session) => session.status === "needs-you").length,
    running: sessions.some((session) => session.status === "running"),
    unread: sessions.some((session) => session.unread),
  }
}

export function statusCounts(sessions: readonly SessionSummary[]): {
  needsYou: number
} {
  return {
    needsYou: sessions.filter((session) => session.status === "needs-you").length,
  }
}
