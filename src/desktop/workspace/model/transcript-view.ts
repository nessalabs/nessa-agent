import type { Activity, Message } from "./transcript"
import { shownActivity } from "./transcript"

/**
 * How much of the agent's work a conversation shows. `detailed` shows every
 * tool run beside what the agent wrote; `compact` shows only what was said —
 * the person's messages and the agent's replies, with what the agent
 * published and how each turn ended — and, while a turn runs, one quiet
 * "Working" line that names no call.
 */
export const transcriptViews = [
  { id: "detailed", label: "Detailed" },
  { id: "compact", label: "Compact" },
] as const

export type TranscriptView = (typeof transcriptViews)[number]["id"]

export const defaultTranscriptView: TranscriptView = "detailed"

/** Reads a stored or requested view, falling back to the default for anything unknown. */
export function parseTranscriptView(value: unknown): TranscriptView {
  return transcriptViews.find((view) => view.id === value)?.id ?? defaultTranscriptView
}

/** What the compact view's live line says, whatever the agent is running. */
export const compactActivityLabel = "Working"

/**
 * The live row to draw after `messages` in `view`. Detailed draws `activity`
 * where no live tool run already shows it (`shownActivity`). Compact draws no
 * tool run, so the row always shows while the agent is at work, and says only
 * that it is working, from the turn's start.
 */
export function liveActivity(
  view: TranscriptView,
  messages: readonly Message[],
  activity: Activity | null,
): Activity | null {
  if (!activity) return null
  if (view === "compact") return { label: compactActivityLabel, since: activity.since }
  return shownActivity(messages, activity)
}
