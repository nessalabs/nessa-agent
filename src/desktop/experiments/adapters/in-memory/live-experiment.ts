/**
 * The sample experiment, played live: evaluators grade cases, runs settle into
 * the verdicts scripted for them, a kept change becomes the new best, and the
 * agent that made it drafts its next idea and starts again. The clock and the
 * timer are injected, so a test can step it by hand.
 */
import type { ExperimentSource } from "../../application/ports"
import {
  champion,
  runById,
  settled,
  type Agent,
  type Experiment,
  type Run,
} from "../../model/experiment"
import {
  fileCount,
  liveOutcomes,
  nextIdeas,
  sampleExperiment,
  type Scripted,
} from "./sample-experiment"
import { caseResults, changeFor } from "./sample-scale"

const tick = 450
/** How long an agent drafts before its next change starts evaluating. */
const drafting = 7_000
/** How long an agent diagnosing a stalled area takes before it tries again. */
const diagnosing = 24_000

const round = (value: number) => Math.round(value * 10) / 10

export function liveExperimentSource({
  now,
  every,
}: {
  now: () => number
  /** Calls `run` every `ms` until the returned function is called. */
  every: (ms: number, run: () => void) => () => void
}): ExperimentSource {
  let experiment = sampleExperiment(now())
  const listeners = new Set<() => void>()
  const scripted = new Map<string, Scripted>()
  for (const run of experiment.runs) {
    const outcome = liveOutcomes[run.title.toLowerCase()]
    if (outcome) scripted.set(run.id, outcome)
  }
  const ideasTaken = new Map<string, number>()
  let stop: (() => void) | null = null
  // A little variety in the evaluators' pace, the same on every launch.
  let pace = 7

  const nextPace = () => {
    pace = (pace * 16807) % 2147483647
    return pace % 4
  }

  const settle = (current: Experiment, run: Run, outcome: Scripted): Experiment => {
    const best = champion(current)
    const at = now()
    const parent =
      outcome.verdict === "kept" ? best : (runById(current, run.parentId ?? "") ?? best)
    const settledRun: Run = {
      ...run,
      parentId: parent.id,
      verdict: outcome.verdict,
      progress: undefined,
      train: { mean: round((parent.train?.mean ?? 0) + outcome.train), ci: 2.2 },
      test: { mean: round((parent.test?.mean ?? 0) + outcome.test), ci: 1.8 },
      cost: Math.round((parent.cost ?? 0.05) * 1.01 * 1000) / 1000,
      cases: caseResults(
        run.id,
        run.areaId,
        parent.test?.mean ?? 0,
        outcome.test,
        current.suite.test,
      ),
    }
    const agents = current.agents.map((agent): Agent =>
      agent.id === run.agentId
        ? {
            ...agent,
            activity: {
              kind: "drafting",
              note: `Its next ${areaName(current, agent.areaId)} change, after #${run.number}.`,
            },
            since: at,
          }
        : agent,
    )
    const notes =
      outcome.verdict === "kept"
        ? [
            ...current.notes,
            {
              id: `n-${run.id}`,
              tone: "good" as const,
              text: `New best: #${run.number} ${run.title} added ${outcome.test.toFixed(1)} points on test.`,
              at,
              runId: run.id,
            },
          ]
        : current.notes
    return {
      ...current,
      runs: current.runs.map((each) => (each.id === run.id ? settledRun : each)),
      agents,
      notes,
    }
  }

  const rest = (current: Experiment, agent: Agent, note: string): Experiment => ({
    ...current,
    agents: current.agents.map((each) =>
      each.id === agent.id
        ? { ...each, activity: { kind: "resting", note }, since: now() }
        : each,
    ),
  })

  const startNext = (current: Experiment, agent: Agent): Experiment => {
    const at = now()
    const total = current.suite.train + current.suite.test
    const spent = current.runs.filter(
      (run) => settled(run) || run.verdict === "running",
    ).length
    if (spent >= current.budget) return rest(current, agent, "Done: the budget is spent.")
    const best = champion(current)
    const waiting = current.runs.find(
      (run) => run.agentId === agent.id && run.verdict === "queued",
    )
    let runs = current.runs
    let started: Run
    if (waiting) {
      started = {
        ...waiting,
        parentId: best.id,
        verdict: "running",
        startedAt: at,
        progress: { done: 0, total },
      }
      runs = runs.map((run) => (run.id === waiting.id ? started : run))
    } else {
      // An area's ideas are shared by the agents in it: each is tried once.
      const ideas = nextIdeas[agent.areaId] ?? []
      const taken = ideasTaken.get(agent.areaId) ?? 0
      const idea = ideas[taken]
      if (!idea)
        return rest(current, agent, "Out of ideas here; waiting for a new direction.")
      ideasTaken.set(agent.areaId, taken + 1)
      const number = Math.max(...runs.map((run) => run.number)) + 1
      started = {
        id: `r${number}`,
        number,
        parentId: best.id,
        areaId: agent.areaId,
        agentId: agent.id,
        title: idea[0],
        rationale: idea[1],
        change: changeFor(
          `r${number}`,
          agent.areaId,
          undefined,
          fileCount(number, agent.areaId),
        ),
        startedAt: at,
        verdict: "running",
        progress: { done: 0, total },
      }
      scripted.set(started.id, idea[2])
      // A new run takes its number after the queued ones it jumped.
      runs = [...runs, started].sort((a, b) => a.number - b.number)
    }
    return {
      ...current,
      runs,
      agents: current.agents.map((each) =>
        each.id === agent.id
          ? { ...each, activity: { kind: "evaluating", runId: started.id }, since: at }
          : each,
      ),
    }
  }

  const step = () => {
    let next = experiment
    // Grade a few more cases on every live run; settle the ones that finish.
    for (const run of next.runs) {
      const progress = run.progress
      if (run.verdict !== "running" || !progress) continue
      // About a minute of grading a run, whatever the suite's size.
      const step = Math.ceil(progress.total * (0.011 + nextPace() * 0.003))
      const done = Math.min(progress.total, progress.done + step)
      const outcome = scripted.get(run.id)
      next =
        done >= progress.total && outcome
          ? settle(next, run, outcome)
          : {
              ...next,
              runs: next.runs.map((each) =>
                each.id === run.id ? { ...run, progress: { ...progress, done } } : each,
              ),
            }
    }
    // Agents that have drafted or diagnosed long enough start their next change.
    const at = now()
    for (const agent of next.agents) {
      const waitFor =
        agent.activity.kind === "drafting"
          ? drafting
          : agent.activity.kind === "diagnosing"
            ? diagnosing
            : Infinity
      if (at - agent.since >= waitFor) next = startNext(next, agent)
    }
    if (next !== experiment) {
      experiment = { ...next, asOf: at }
      listeners.forEach((listener) => listener())
    }
  }

  return {
    get: (id) => (id === experiment.id ? experiment : undefined),
    bySession: (sessionId) =>
      experiment.sessionId === sessionId ? experiment : undefined,
    // The sample has no repository to open; the gateway's source hands the
    // target to the editor the person chose.
    openInEditor: () => {},
    subscribe(listener) {
      listeners.add(listener)
      stop ??= every(tick, step)
      return () => {
        listeners.delete(listener)
        if (listeners.size > 0) return
        stop?.()
        stop = null
      }
    },
  }
}

function areaName(experiment: Experiment, areaId: string): string {
  return experiment.areas.find((area) => area.id === areaId)?.name.toLowerCase() ?? "next"
}
