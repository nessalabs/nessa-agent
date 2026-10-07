/**
 * Subagents for the sample workspace, on the injected clock. Independent of
 * experiments. Joined under `sample` from composition. A conversation other
 * than the retry-budget sample has none. Three lines are added to Mara's
 * conversation after the source is first read, far enough apart that a
 * transcript can follow one and then stay put for the next
 * (`sample-source.test.ts`, `verification/desktop/scripts/subagents.mjs`).
 */
import type { Message } from "../../../workspace/model/transcript"
import { retryBudgetSession } from "../../../workspace/adapters/in-memory/sample-labs"
import type { Subagent, SubagentTag } from "../../model/subagent"
import type { SubagentRead, SubagentSource } from "../../application/ports"

export interface SampleSubagentSchedule {
  now(): number
  after(ms: number, run: () => void): () => void
}

export interface SampleSubagentSource extends SubagentSource {
  /** Cancels the lines still waiting to be added. */
  dispose(): void
}

const none: SubagentRead = { kind: "ready", subagents: [], unreadable: [] }

/** Lines added to Mara's conversation, and when, after the source is first read. */
const followUps = [
  ["The bucket holds.", 15_000],
  ["The wait stays bounded.", 25_000],
  ["The budget is the bound.", 35_000],
] as const

const tags = {
  reconnects: { id: "reconnects", name: "Reconnects", hue: 1 },
  budget: { id: "budget", name: "Budget", hue: 2 },
  timeouts: { id: "timeouts", name: "Timeouts", hue: 3 },
  history: { id: "history", name: "History", hue: 4 },
} as const satisfies Record<string, SubagentTag>

/** The sample source. `dispose` cancels a follow-up that has not run. */
export function sampleSubagentSource(
  schedule: SampleSubagentSchedule,
): SampleSubagentSource {
  const listeners = new Set<() => void>()
  const stops: Array<() => void> = []
  let started = false
  let extras: Message[] = []
  let snapshot: SubagentRead = build(schedule.now(), extras)

  const publish = () => {
    snapshot = build(schedule.now(), extras)
    for (const listener of listeners) listener()
  }

  const ensure = () => {
    if (started) return
    started = true
    followUps.forEach(([text, at], index) => {
      stops.push(
        schedule.after(at, () => {
          extras = [
            ...extras,
            {
              id: `mara-follow-${index}`,
              role: "agent",
              at: schedule.now(),
              parts: [{ kind: "text", text }],
            },
          ]
          publish()
        }),
      )
    })
  }

  return {
    forSession(sessionId) {
      if (sessionId !== retryBudgetSession) return none
      ensure()
      return snapshot
    },
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    dispose() {
      for (const stop of stops) stop()
      stops.length = 0
    },
  }
}

function build(now: number, extras: readonly Message[]): SubagentRead {
  const brief = (id: string, text: string, at: number): Message => ({
    id,
    role: "user",
    at,
    parts: [{ kind: "text", text }],
  })
  const said = (id: string, text: string, at: number): Message => ({
    id,
    role: "agent",
    at,
    parts: [{ kind: "text", text }],
  })
  const mara: Subagent = {
    id: "mara",
    name: "Mara",
    seed: "mara-reconnect",
    tags: [tags.reconnects],
    headline: "Measuring how long a reconnect waits",
    activity: "working",
    lifecycle: "open",
    since: now - 4 * 60_000,
    model: { provider: "anthropic", modelId: "claude-sonnet-5" },
    progress: { done: 3, total: 5 },
    conversation: {
      messages: [
        brief("mara-1", "Give the reconnects a retry budget.", now - 6 * 60_000),
        said(
          "mara-2",
          "I'll bound the wait and count what is still in flight.",
          now - 5 * 60_000,
        ),
        said(
          "mara-3",
          "Three of the five cases hold. Two are still running.",
          now - 4 * 60_000,
        ),
        ...Array.from({ length: 8 }, (_, index) =>
          said(
            `mara-pad-${index}`,
            `Case ${index + 1} stays inside the budget.`,
            now - (3 * 60_000 - index * 1000),
          ),
        ),
        ...extras,
      ],
      activity: { label: "Measuring the bucket", since: now - 12_000 },
    },
  }
  const idris: Subagent = {
    id: "idris",
    name: "Idris",
    seed: "idris-budget",
    tags: [tags.budget],
    headline: "Writing the next case",
    activity: "planning",
    lifecycle: "open",
    since: now - 2 * 60_000,
    conversation: {
      messages: [brief("idris-1", "Plan the budget-exhaustion case.", now - 2 * 60_000)],
      activity: null,
    },
  }
  const nia: Subagent = {
    id: "nia",
    name: "Nia",
    seed: "nia-timeout",
    tags: [tags.timeouts],
    headline: "A reconnect never came back",
    activity: "stuck",
    lifecycle: "open",
    since: now - 9 * 60_000,
    conversation: {
      messages: [
        brief("nia-1", "Find the reconnect that does not return.", now - 9 * 60_000),
        said("nia-2", "The harness accepted it and then went quiet.", now - 8 * 60_000),
      ],
      activity: null,
    },
  }
  const sol: Subagent = {
    id: "sol",
    name: "Sol",
    seed: "sol-history",
    tags: [tags.history],
    headline: "The earlier budget is recorded",
    activity: "idle",
    lifecycle: "closed",
    since: now - 30 * 60_000,
    conversation: {
      messages: [said("sol-1", "Recorded, and closed.", now - 30 * 60_000)],
      activity: null,
    },
  }
  return { kind: "ready", subagents: [mara, idris, nia, sol], unreadable: [] }
}
