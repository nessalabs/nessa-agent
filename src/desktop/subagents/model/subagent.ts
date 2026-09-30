/**
 * The agents a conversation has put to work: who each is, what it is doing
 * now, and what it has done. A conversation's subagents may come from an
 * experiment's swarm or anywhere else; this is what the panel shows of them,
 * whatever their source. Each is an agent with a conversation of its own —
 * read like any chat, and written to like one.
 */
import type { Activity, Message } from "../../workspace/model/transcript"

/** What a subagent is doing, in the few states a person can act on. */
export type SubagentState =
  /** Doing the work now; `work` says what, and how far along. */
  | "working"
  /** Between pieces of work: writing the next one. */
  | "thinking"
  /** Its line of work has stalled; it is working out why. */
  | "stuck"
  /** Nothing left for it to do. */
  | "resting"

/**
 * A tag a subagent is spun up with, saying what it is for — an experiment
 * gives each agent its area. Optional: a subagent may have none.
 */
export interface SubagentTag {
  readonly id: string
  readonly name: string
  /** The window's series colour it is marked with, 1–5 (`--desktop-series-*`). */
  readonly hue: 1 | 2 | 3 | 4 | 5
  /** Its mark, as one SVG path on a 16-unit grid. */
  readonly glyph: string
}

/** The piece of work a subagent is on now. */
export interface SubagentWork {
  readonly title: string
  readonly startedAt: number
  readonly progress?: { readonly done: number; readonly total: number }
}

export interface Subagent {
  readonly id: string
  readonly name: string
  /** What its generative mark is painted from; the same wherever it appears. */
  readonly seed: string
  /** The tag it was spun up with, if any. */
  readonly tagId?: string
  readonly state: SubagentState
  /** What it is doing now, in a sentence. */
  readonly headline: string
  readonly since: number
  readonly work?: SubagentWork
  /** Its own conversation: the brief it was given, and everything since. */
  readonly conversation: {
    readonly messages: readonly Message[]
    readonly activity: Activity | null
  }
  /** The model it runs on, for its composer. */
  readonly model?: { readonly provider: string; readonly modelId: string }
}

/** A conversation's subagents, as their source knows them now. */
export interface SessionSubagents {
  readonly sessionId: string
  /** The "now" its times are read against. */
  readonly asOf: number
  readonly tags: readonly SubagentTag[]
  readonly subagents: readonly Subagent[]
}

export const stateLabels: Record<SubagentState, string> = {
  working: "Working",
  thinking: "Thinking",
  stuck: "Stuck",
  resting: "Resting",
}

const stateOrder: Record<SubagentState, number> = {
  working: 0,
  thinking: 1,
  stuck: 2,
  resting: 3,
}

/**
 * Subagents in the order a person reads them: those at work first — the
 * furthest along leading — then those thinking, stuck, and resting.
 */
export function byActivity(subagents: readonly Subagent[]): readonly Subagent[] {
  const along = (subagent: Subagent) =>
    subagent.work?.progress
      ? subagent.work.progress.done / subagent.work.progress.total
      : 0
  return [...subagents].sort(
    (a, b) => stateOrder[a.state] - stateOrder[b.state] || along(b) - along(a),
  )
}

/** How many subagents are in each state. */
export function stateCounts(
  subagents: readonly Subagent[],
): Record<SubagentState, number> {
  const counts: Record<SubagentState, number> = {
    working: 0,
    thinking: 0,
    stuck: 0,
    resting: 0,
  }
  for (const subagent of subagents) counts[subagent.state] += 1
  return counts
}
