/**
 * An experiment's swarm, read as its conversation's subagents: each agent a
 * subagent with a conversation of its own — the brief it was given, then a
 * turn for every change it tried — tagged with its area. Computed once per
 * snapshot of the experiment and of what the person has said to its agents,
 * so a view reading it on every render sees the same value until either
 * changes.
 */
import type {
  SessionSubagents,
  Subagent,
  SubagentSource,
  SubagentState,
} from "../../../subagents"
import type { Activity, Message, Part } from "../../../workspace"
import type { ExperimentSource } from "../../application/ports"
import {
  runById,
  verdictReason,
  type Agent,
  type Experiment,
  type Run,
} from "../../model/experiment"
import { areaGlyphPath } from "../../ui/area-glyphs"
import { count } from "../../../ui/format"

const states: Record<Agent["activity"]["kind"], SubagentState> = {
  evaluating: "working",
  drafting: "thinking",
  diagnosing: "stuck",
  resting: "resting",
}

/** The model the sample swarm runs on. */
const swarmModel = { provider: "anthropic", modelId: "claude-opus-5" }

/** The brief the lead agent gave an agent when it put it on its area. */
function brief(experiment: Experiment, agent: Agent): Message {
  const area = experiment.areas.find((each) => each.id === agent.areaId)
  return {
    id: `${agent.id}-brief`,
    role: "user",
    at: experiment.startedAt,
    parts: [
      {
        kind: "text",
        text: `You own ${area?.name.toLowerCase() ?? "your area"} for ${experiment.title}. ${area?.hypothesis ?? ""} Propose one change at a time from the best version. It is kept only if train and test both clear the ±${experiment.noise.toFixed(1)} noise.`,
      },
    ],
  }
}

/** One change it tried, as the turn it took: what it read and changed, the eval, and what came of it. */
function turnFor(experiment: Experiment, run: Run): Message {
  const files = [...run.change.files].sort(
    (a, b) => b.added + b.removed - (a.added + a.removed),
  )
  const parts: Part[] = [
    ...files.slice(0, 2).map((file): Part => ({
      kind: "step",
      step: "read",
      label: "Read",
      detail: file.path,
    })),
    ...files.slice(0, 3).map((file): Part => ({
      kind: "step",
      step: "edit",
      label: "Edited",
      detail: file.path,
      added: file.added,
      removed: file.removed,
    })),
    ...(files.length > 3
      ? [
          {
            kind: "step",
            step: "edit",
            label: "Edited",
            detail: `${count(files.length - 3)} more files`,
          } satisfies Part,
        ]
      : []),
    {
      kind: "step",
      step: "run",
      label: "Ran",
      detail: `nessa eval run ${experiment.suite.name.split(" ")[0]} --candidate ${run.id}`,
    },
    {
      kind: "text",
      text:
        run.verdict === "running" || run.verdict === "queued"
          ? `**#${run.number} ${run.title}.** ${run.rationale} Grading it now.`
          : `**#${run.number} ${run.title}.** ${verdictReason(experiment, run)}`,
    },
  ]
  return { id: `${run.agentId}-${run.id}`, role: "agent", at: run.startedAt, parts }
}

/** What it is doing now, as a chat's live line says it. */
function liveLine(experiment: Experiment, agent: Agent): Activity | null {
  const activity = agent.activity
  if (activity.kind === "evaluating") {
    const run = runById(experiment, activity.runId)
    const progress = run?.progress
    return {
      label: progress
        ? `Evaluating #${run.number} · ${count(progress.done)} / ${count(progress.total)} cases`
        : "Evaluating",
      since: agent.since,
    }
  }
  if (activity.kind === "drafting")
    return { label: "Drafting its next change", since: agent.since }
  if (activity.kind === "diagnosing")
    return { label: "Reading the failures by root cause", since: agent.since }
  return null
}

function subagentOf(
  experiment: Experiment,
  agent: Agent,
  said: readonly Message[],
): Subagent {
  const runs = experiment.runs.filter((run) => run.agentId === agent.id)
  const activity = agent.activity
  const current =
    activity.kind === "evaluating" ? runById(experiment, activity.runId) : undefined
  const turns = runs
    .filter((run) => run.verdict !== "queued")
    .map((run) => turnFor(experiment, run))
  return {
    id: agent.id,
    name: agent.name,
    // The seed the experiment paints the agent with, so it looks the same in both.
    seed: `xp-agent-${agent.id}`,
    tagId: agent.areaId,
    state: states[activity.kind],
    headline:
      activity.kind === "evaluating"
        ? `Evaluating #${current?.number ?? "?"} ${current?.title ?? ""}`.trim()
        : activity.note,
    since: agent.since,
    ...(current
      ? {
          work: {
            title: current.title,
            startedAt: current.startedAt,
            ...(current.progress ? { progress: current.progress } : {}),
          },
        }
      : {}),
    conversation: {
      messages: [brief(experiment, agent), ...turns, ...said].sort((a, b) => a.at - b.at),
      activity: liveLine(experiment, agent),
    },
    model: swarmModel,
  }
}

function subagentsOf(
  experiment: Experiment,
  said: ReadonlyMap<string, readonly Message[]>,
): SessionSubagents {
  return {
    sessionId: experiment.sessionId ?? experiment.id,
    asOf: experiment.asOf,
    tags: experiment.areas.map((area) => ({
      id: area.id,
      name: area.name,
      hue: area.slot,
      glyph: areaGlyphPath(area.id),
    })),
    subagents: experiment.agents.map((agent) =>
      subagentOf(experiment, agent, said.get(agent.id) ?? []),
    ),
  }
}

/**
 * The subagents of every conversation that runs an experiment. What the
 * person says to one is kept here, and answered after a moment: the sample
 * swarm acknowledges it; a real one reads it at its next turn.
 */
export function experimentSubagents(
  experiments: ExperimentSource,
  {
    now,
    after,
  }: {
    now: () => number
    /** Calls `run` once, `ms` from now. */
    after: (ms: number, run: () => void) => void
  },
): SubagentSource {
  const said = new Map<string, readonly Message[]>()
  let version = 0
  const listeners = new Set<() => void>()
  const read = new WeakMap<Experiment, { version: number; value: SessionSubagents }>()
  const told = () => {
    version += 1
    listeners.forEach((listener) => listener())
  }
  const add = (agentId: string, message: Message) => {
    said.set(agentId, [...(said.get(agentId) ?? []), message])
    told()
  }
  return {
    forSession(sessionId) {
      const experiment = experiments.bySession(sessionId)
      if (!experiment) return undefined
      const held = read.get(experiment)
      if (held && held.version === version) return held.value
      const value = subagentsOf(experiment, said)
      read.set(experiment, { version, value })
      return value
    },
    subscribe(listener) {
      listeners.add(listener)
      const stop = experiments.subscribe(listener)
      return () => {
        listeners.delete(listener)
        stop()
      }
    },
    send(sessionId, subagentId, text) {
      if (!experiments.bySession(sessionId) || !text.trim()) return
      const at = now()
      add(subagentId, {
        id: `${subagentId}-said-${at}`,
        role: "user",
        at,
        parts: [{ kind: "text", text: text.trim() }],
      })
      after(1200, () =>
        add(subagentId, {
          id: `${subagentId}-reply-${at}`,
          role: "agent",
          at: now(),
          parts: [
            {
              kind: "text",
              text: "Noted. I'll take that into my next change, and say here what came of it.",
            },
          ],
        }),
      )
    },
  }
}
