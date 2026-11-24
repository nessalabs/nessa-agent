/**
 * The quick switcher's search: a subsequence match that favours runs and word
 * starts, and the rows it offers — sessions, the person's terminals,
 * channels, and starting a new session (with what was typed) or a terminal. The switcher renders these rows; it decides
 * nothing about them.
 */
import {
  agentName,
  agentOf,
  byRecency,
  type Channel,
  type SessionSummary,
} from "./workspace-index"
import { statusLabels } from "./session-groups"
import type { TerminalEntry } from "./terminal"

export interface FuzzyMatch {
  readonly score: number
  /** Indexes of the matched characters in the text, for highlighting. */
  readonly hits: readonly number[]
}

/** Matches `query`'s characters in order anywhere in `text`, or not at all. */
export function fuzzy(query: string, text: string): FuzzyMatch | null {
  const needle = query.toLowerCase().replace(/\s+/g, "")
  if (!needle) return { score: 0, hits: [] }
  const haystack = text.toLowerCase()
  const hits: number[] = []
  let score = 0
  let last = -2
  let at = 0
  for (let index = 0; index < haystack.length && at < needle.length; index++) {
    if (haystack[index] !== needle[at]) continue
    const wordStart = index === 0 || /[\s\-#/_.:]/.test(haystack[index - 1])
    score += 1 + (index === last + 1 ? 3 : 0) + (wordStart ? 4 : 0)
    hits.push(index)
    last = index
    at++
  }
  if (at < needle.length) return null
  return { score: score - haystack.length * 0.02, hits }
}

export type SwitcherRow =
  | {
      readonly kind: "session"
      readonly group: string
      readonly session: SessionSummary
      readonly hits: readonly number[]
    }
  | {
      readonly kind: "channel"
      readonly group: string
      readonly channel: Channel
      readonly hits: readonly number[]
    }
  | {
      readonly kind: "terminal"
      readonly group: string
      readonly terminal: TerminalEntry
      readonly hits: readonly number[]
    }
  | {
      readonly kind: "new"
      readonly group: string
      readonly channelId: string
      /** The message to start with: what was typed, when anything was. */
      readonly text?: string
    }
  | {
      /** A new shell in the channel, as ⌘J opens. */
      readonly kind: "newTerminal"
      readonly group: string
      readonly channelId: string
    }

const recentCount = 6
const sessionMatchCount = 7
const channelMatchCount = 4

/**
 * What the switcher offers for `query`. With nothing typed: a new session in
 * the current channel, then whatever waits on the person, then the most
 * recent. With a query: the best sessions and channels, and starting a new
 * session with the query as its first message.
 */
export function switcherRows({
  query,
  sessions,
  channels,
  channelId,
  terminals = null,
}: {
  query: string
  sessions: readonly SessionSummary[]
  channels: readonly Channel[]
  /** The channel a new session would start in. */
  channelId: string
  /**
   * The person's shells, when the window has terminals; a new one is offered
   * only then. A Run's terminal belongs to its reply and is not listed.
   */
  terminals?: readonly TerminalEntry[] | null
}): SwitcherRow[] {
  const shells = (terminals ?? []).filter((terminal) => terminal.kind === "shell")
  const channelName = (id: string) =>
    channels.find((channel) => channel.id === id)?.name ?? ""
  const trimmed = query.trim()
  if (!trimmed) {
    const waiting = sessions.filter((session) => session.status === "needs-you")
    const recent = sessions
      .filter((session) => session.status !== "needs-you")
      .sort(byRecency)
      .slice(0, recentCount)
    return [
      { kind: "new", group: "", channelId },
      ...(terminals ? [{ kind: "newTerminal", group: "", channelId } as const] : []),
      ...waiting.map((session): SwitcherRow => ({
        kind: "session",
        group: statusLabels["needs-you"],
        session,
        hits: [],
      })),
      ...recent.map((session): SwitcherRow => ({
        kind: "session",
        group: "Recent",
        session,
        hits: [],
      })),
      ...[...shells]
        .sort((a, b) => b.startedAtMs - a.startedAtMs)
        .slice(0, recentCount)
        .map((terminal): SwitcherRow => ({
          kind: "terminal",
          group: "Terminals",
          terminal,
          hits: [],
        })),
    ]
  }
  const sessionMatches = sessions
    .map((session) => {
      const title = fuzzy(trimmed, session.title)
      const context = fuzzy(
        trimmed,
        `${session.title} ${channelName(session.channelId)} ${agentName(agentOf(session.model))}`,
      )
      if (!title && !context) return null
      return {
        session,
        score:
          (title ? title.score + 2 : (context?.score ?? 0)) +
          (session.status !== "idle" ? 1 : 0),
        hits: title ? title.hits : [],
      }
    })
    .filter((match) => match !== null)
    .sort((a, b) => b.score - a.score)
    .slice(0, sessionMatchCount)
  const channelMatches = channels
    .map((channel) => ({ channel, match: fuzzy(trimmed, channel.name) }))
    .filter((entry) => entry.match !== null)
    .sort((a, b) => (b.match?.score ?? 0) - (a.match?.score ?? 0))
    .slice(0, channelMatchCount)
  const terminalMatches = shells
    .map((terminal) => ({ terminal, match: fuzzy(trimmed, terminal.title) }))
    .filter((entry) => entry.match !== null)
    .sort((a, b) => (b.match?.score ?? 0) - (a.match?.score ?? 0))
    .slice(0, sessionMatchCount)
  return [
    ...sessionMatches.map((match): SwitcherRow => ({
      kind: "session",
      group: "Sessions",
      session: match.session,
      hits: match.hits,
    })),
    ...channelMatches.map((entry): SwitcherRow => ({
      kind: "channel",
      group: "Channels",
      channel: entry.channel,
      hits: entry.match?.hits ?? [],
    })),
    ...terminalMatches.map((entry): SwitcherRow => ({
      kind: "terminal",
      group: "Terminals",
      terminal: entry.terminal,
      hits: entry.match?.hits ?? [],
    })),
    { kind: "new", group: "Start", channelId, text: trimmed },
  ]
}
