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
} from "./organisation"

/** The list's groups, in the order they are shown. */
export const statusGroups: readonly { status: SessionStatus; label: string }[] = [
  { status: "needs-you", label: "Needs you" },
  { status: "running", label: "Running" },
  { status: "idle", label: "Earlier" },
]

/** What the session list shows: one channel's sessions, or every session in one state. */
export type SessionView =
  | { readonly kind: "channel"; readonly channelId: string }
  | { readonly kind: "status"; readonly status: "needs-you" | "running" }

export function inView(session: SessionSummary, view: SessionView): boolean {
  return view.kind === "channel"
    ? session.channelId === view.channelId
    : session.status === view.status
}

/** A view's name: the channel's, or the state's. */
export function statusViewLabel(status: "needs-you" | "running"): string {
  return status === "needs-you" ? "Needs you" : "Running"
}

export interface StatusGroup {
  readonly status: SessionStatus
  readonly label: string
  readonly sessions: readonly SessionSummary[]
}

/** Sessions by status in the list's order, newest first in each; empty groups are left out. */
export function groupByStatus(sessions: readonly SessionSummary[]): StatusGroup[] {
  const newest = [...sessions].sort(byRecency)
  return statusGroups
    .map((group) => ({
      ...group,
      sessions: newest.filter((session) => session.status === group.status),
    }))
    .filter((group) => group.sessions.length > 0)
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
  running: number
} {
  return {
    needsYou: sessions.filter((session) => session.status === "needs-you").length,
    running: sessions.filter((session) => session.status === "running").length,
  }
}
