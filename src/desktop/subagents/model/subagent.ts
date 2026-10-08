/**
 * What the panel shows of one agent a conversation has put to work. Activity
 * (`working`, `planning`, `stuck`, `idle`) is separate from the lifetime
 * (`starting`, `open`, `closing`, `closed`): a closed child is not shown as
 * idle (`rowStatus`). The list's order is `byActivity` (`subagent.test.ts`).
 */
import { counted } from "../../model/counts"
import type { Activity, Message } from "../../workspace/model/transcript"

/** What a subagent is doing, in the order the list reads them. */
export const subagentActivities = ["working", "planning", "stuck", "idle"] as const
export type SubagentActivity = (typeof subagentActivities)[number]

/**
 * Where a child stands in its lifetime. Kept apart from activity so closing
 * or closed is not reported as idle.
 */
export const subagentLifecycles = ["starting", "open", "closing", "closed"] as const
export type SubagentLifecycle = (typeof subagentLifecycles)[number]

/** A tag's series colour, 1–5 (`--nessa-chart-series-*`). */
export type TagHue = 1 | 2 | 3 | 4 | 5

/** A tag carried as data: a name and the series colour it is drawn in. */
export interface SubagentTag {
  readonly id: string
  readonly name: string
  readonly hue: TagHue
}

/** Measured progress, when the source has some. `total` of 0 is not a fraction. */
export interface SubagentProgress {
  readonly done: number
  readonly total: number
}

/** The model a subagent runs on. Absent, and the row draws none. */
export interface SubagentModel {
  readonly provider: string
  readonly modelId: string
}

/**
 * One subagent as its source published it. `id` is the source's own id until
 * a join rewrites it (`joinedSubagentId`); the conversation it holds is the
 * child's ordinary transcript.
 */
export interface Subagent {
  readonly id: string
  readonly name: string
  /** What its avatar and tagline are painted from. */
  readonly seed: string
  readonly tags: readonly SubagentTag[]
  readonly headline: string
  readonly activity: SubagentActivity
  readonly lifecycle: SubagentLifecycle
  /** When the current activity began, as a timestamp the caller supplies. */
  readonly since: number
  readonly model?: SubagentModel
  readonly progress?: SubagentProgress
  readonly conversation: {
    readonly messages: readonly Message[]
    readonly activity: Activity | null
  }
}

export const activityLabels: Record<SubagentActivity, string> = {
  working: "Working",
  planning: "Planning",
  stuck: "Stuck",
  idle: "Idle",
}

/** The tone a status label takes for an open child's activity. */
export const activityTones = {
  working: "active",
  planning: "neutral",
  stuck: "warning",
  idle: "neutral",
} as const

/**
 * How far `progress` is, from 0 to 1, or 0 when it is absent or its total
 * is not a positive number. Working children with a larger fraction lead
 * (`byActivity`).
 */
export function progressFraction(progress: SubagentProgress | undefined): number {
  if (!progress || progress.total <= 0 || !Number.isFinite(progress.done)) return 0
  return progress.done / progress.total
}

/**
 * Subagents in the order the list shows them: the index in
 * `subagentActivities` (working, then planning, stuck, and idle), and among
 * working children the furthest along first. A tie keeps the order they were
 * given. Lifetime does not reorder them; `rowStatus` says it. `summaryLine`
 * walks the same list.
 */
export function byActivity(subagents: readonly Subagent[]): readonly Subagent[] {
  return subagents
    .map((subagent, index) => ({ subagent, index }))
    .sort((a, b) => {
      const byState =
        subagentActivities.indexOf(a.subagent.activity) -
        subagentActivities.indexOf(b.subagent.activity)
      if (byState !== 0) return byState
      if (a.subagent.activity === "working") {
        const byProgress =
          progressFraction(b.subagent.progress) - progressFraction(a.subagent.progress)
        if (byProgress !== 0) return byProgress
      }
      return a.index - b.index
    })
    .map(({ subagent }) => subagent)
}

/** How many open children are in each activity. A child that is not open is not counted here. */
export function activityCounts(
  subagents: readonly Subagent[],
): Record<SubagentActivity, number> {
  const counts: Record<SubagentActivity, number> = {
    working: 0,
    planning: 0,
    stuck: 0,
    idle: 0,
  }
  for (const subagent of subagents) {
    if (subagent.lifecycle === "open") counts[subagent.activity] += 1
  }
  return counts
}

/** How many children are starting, closing, or closed. */
export function lifecycleCounts(
  subagents: readonly Subagent[],
): Record<Exclude<SubagentLifecycle, "open">, number> {
  const counts = { starting: 0, closing: 0, closed: 0 }
  for (const subagent of subagents) {
    if (subagent.lifecycle !== "open") counts[subagent.lifecycle] += 1
  }
  return counts
}

/**
 * The list's summary: open children by activity, then those starting,
 * closing, or closed, so a closed child is not called idle.
 */
export function summaryLine(subagents: readonly Subagent[]): string {
  const activity = activityCounts(subagents)
  const lifetime = lifecycleCounts(subagents)
  const parts: string[] = []
  for (const state of subagentActivities) {
    if (activity[state] > 0)
      parts.push(counted(activity[state], activityLabels[state].toLowerCase()))
  }
  if (lifetime.starting > 0) parts.push(counted(lifetime.starting, "starting"))
  if (lifetime.closing > 0) parts.push(counted(lifetime.closing, "closing"))
  if (lifetime.closed > 0) parts.push(counted(lifetime.closed, "closed"))
  return parts.join(" · ")
}

/** What a row says of a child: its lifetime when that is not open, otherwise its activity. */
export function rowStatus(subagent: Subagent): {
  readonly word: string
  readonly tone: "neutral" | "warning" | "active"
} {
  switch (subagent.lifecycle) {
    case "starting":
      return { word: "Starting", tone: "active" }
    case "closing":
      return { word: "Closing", tone: "warning" }
    case "closed":
      return { word: "Closed", tone: "neutral" }
    case "open":
      return {
        word: activityLabels[subagent.activity],
        tone: activityTones[subagent.activity],
      }
  }
}
