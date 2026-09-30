/**
 * An experiment: a swarm of agents hill-climbing one product against an eval.
 * Each agent works an area — the prompt, the tool descriptions, retrieval —
 * proposing one change at a time from the best version so far. A change is
 * evaluated on a train and a test split and kept only when both improve by
 * more than the noise; otherwise it is reverted, and the reason is recorded.
 *
 * Everything the views read is derived here from the runs: the best so far,
 * the path that reached it, how each area is going. Nothing here knows how
 * the runs are produced or drawn.
 */

/** Points on the eval's primary metric, 0–100. */
export interface Score {
  readonly mean: number
  /** Half-width of the 95% interval, in points. */
  readonly ci: number
}

export type Verdict =
  /** Both splits improved beyond the noise: the new best. */
  | "kept"
  /** Train improved, test did not: fitted to the train cases, reverted. */
  | "overfit"
  /** A split got worse beyond the noise, reverted. */
  | "regressed"
  /** Moved less than the noise: indistinguishable from its parent, reverted. */
  | "flat"
  /** Better, but costs more per task than the goal allows, reverted. */
  | "costly"
  /** Being evaluated now. */
  | "running"
  /** Written, waiting for an evaluator. */
  | "queued"

export type SettledVerdict = Exclude<Verdict, "running" | "queued">

/** How one case of the test split moved from the parent to this run. */
export type CaseMove = "fixed" | "broke" | "passing" | "failing"

/**
 * A slice of the test split — the cases about one kind of request — and how
 * many of them passed before the run and after it.
 */
export interface CaseSlice {
  readonly name: string
  readonly total: number
  readonly before: number
  readonly after: number
}

/** One case whose outcome changed: the few worth reading among thousands. */
export interface MovedCase {
  readonly id: string
  readonly title: string
  readonly slice: string
  readonly move: "fixed" | "broke"
}

/**
 * The test split against the parent, at any size: how many cases moved each
 * way, by slice, and which cases moved. Never one entry per case — a suite of
 * ten thousand is described in the same few numbers as one of sixty.
 */
export interface CaseResults {
  readonly counts: Readonly<Record<CaseMove, number>>
  readonly slices: readonly CaseSlice[]
  readonly moved: readonly MovedCase[]
}

export type FileStatus = "added" | "modified" | "deleted" | "renamed"

export interface FileChange {
  readonly path: string
  readonly status: FileStatus
  readonly added: number
  readonly removed: number
  /** What changed in it, in a line, where the agent said. */
  readonly note?: string
}

/**
 * What a run changed: in a sentence, and every file it touched. Never the
 * lines themselves — a change can be a thousand lines in a thousand files;
 * the diff is opened, not shown.
 */
export interface Change {
  readonly summary?: string
  readonly files: readonly FileChange[]
}

export interface Run {
  readonly id: string
  /** Order of proposal, from 1; the baseline is 0. */
  readonly number: number
  /** The run it was proposed from; null for the baseline. */
  readonly parentId: string | null
  readonly areaId: string
  readonly agentId: string
  /** The change, as a one-line headline. */
  readonly title: string
  /** Why the agent expected it to help. */
  readonly rationale: string
  readonly change: Change
  readonly startedAt: number
  readonly verdict: Verdict
  readonly train?: Score
  readonly test?: Score
  /** Dollars per task, averaged over the eval. */
  readonly cost?: number
  /** For a running run: cases graded so far, of the whole eval. */
  readonly progress?: { readonly done: number; readonly total: number }
  /** The test split against the parent, once it is settled. */
  readonly cases?: CaseResults
}

export interface Area {
  readonly id: string
  readonly name: string
  /** What the agents on it believe is worth trying. */
  readonly hypothesis: string
  /** Categorical slot, 1–5, fixed for the area's life. */
  readonly slot: 1 | 2 | 3 | 4 | 5
}

export type AgentActivity =
  | { readonly kind: "evaluating"; readonly runId: string }
  | { readonly kind: "drafting"; readonly note: string }
  | { readonly kind: "diagnosing"; readonly note: string }
  /** Nothing left to do: the budget is spent, or its area has run out of ideas. */
  | { readonly kind: "resting"; readonly note: string }

export interface Agent {
  readonly id: string
  readonly name: string
  readonly areaId: string
  readonly activity: AgentActivity
  readonly since: number
}

export type NoteTone = "good" | "warning" | "info"

/** Something the hill-climber noticed and says in a sentence. */
export interface Note {
  readonly id: string
  readonly tone: NoteTone
  readonly text: string
  readonly at: number
  readonly runId?: string
}

export interface Experiment {
  readonly id: string
  readonly title: string
  readonly goal: string
  readonly metric: string
  readonly suite: {
    readonly name: string
    readonly train: number
    readonly test: number
    readonly grader: string
  }
  /** Run-to-run spread of the baseline on test, in points: changes inside it are flat. */
  readonly noise: number
  /** The strongest reference model at its highest effort, on test: the headroom's top. */
  readonly ceiling: number
  /** Runs the experiment may spend. */
  readonly budget: number
  readonly startedAt: number
  /** The conversation that started it and reports on it, if there is one. */
  readonly sessionId?: string
  /** When the source last spoke of it: the "now" its ages are read against. */
  readonly asOf: number
  readonly baseline: Run
  readonly runs: readonly Run[]
  readonly areas: readonly Area[]
  readonly agents: readonly Agent[]
  readonly notes: readonly Note[]
}

/** Every case of a run's test split. */
export function caseTotal(results: CaseResults): number {
  const { fixed, broke, passing, failing } = results.counts
  return fixed + broke + passing + failing
}

/** Lines a change added and removed, over all its files. */
export function changeTotals(change: Change): { added: number; removed: number } {
  return change.files.reduce(
    (sum, file) => ({
      added: sum.added + file.added,
      removed: sum.removed + file.removed,
    }),
    { added: 0, removed: 0 },
  )
}

export const settled = (run: Run): run is Run & { test: Score; train: Score } =>
  run.test !== undefined && run.train !== undefined

/** Every run in proposal order, the baseline first. */
export function allRuns(experiment: Experiment): readonly Run[] {
  return [experiment.baseline, ...experiment.runs]
}

export function runById(experiment: Experiment, id: string): Run | undefined {
  return allRuns(experiment).find((run) => run.id === id)
}

/** The best version so far: the last change kept, or the baseline. */
export function champion(experiment: Experiment): Run & { test: Score; train: Score } {
  const kept = experiment.runs.filter(
    (run): run is Run & { test: Score; train: Score } =>
      run.verdict === "kept" && settled(run),
  )
  const last = kept[kept.length - 1]
  if (last) return last
  const baseline = experiment.baseline
  if (!settled(baseline)) throw new Error("An experiment's baseline is always scored")
  return baseline
}

/** From the baseline to `run`, following each run's parent. */
export function lineage(experiment: Experiment, run: Run): readonly Run[] {
  const chain: Run[] = []
  let at: Run | undefined = run
  while (at) {
    chain.unshift(at)
    at = at.parentId === null ? undefined : runById(experiment, at.parentId)
  }
  return chain
}

export interface Step {
  readonly run: Run & { test: Score; train: Score }
  /** Test points this step added over its parent. */
  readonly gain: number
}

/** The kept changes that make up the best so far, each with what it added. */
export function pathToBest(experiment: Experiment): readonly Step[] {
  const chain = lineage(experiment, champion(experiment)).filter(settled)
  return chain.slice(1).map((run, index) => ({
    run,
    gain: run.test.mean - chain[index].test.mean,
  }))
}

/** Test points `run` moved from its parent; undefined until both are scored. */
export function testDelta(experiment: Experiment, run: Run): number | undefined {
  const parent = run.parentId === null ? undefined : runById(experiment, run.parentId)
  if (!parent?.test || !run.test) return undefined
  return run.test.mean - parent.test.mean
}

export function trainDelta(experiment: Experiment, run: Run): number | undefined {
  const parent = run.parentId === null ? undefined : runById(experiment, run.parentId)
  if (!parent?.train || !run.train) return undefined
  return run.train.mean - parent.train.mean
}

export interface ClimbPoint {
  readonly number: number
  readonly best: number
}

/** The best test score after each settled run: the line the experiment climbs. */
export function bestSoFar(experiment: Experiment): readonly ClimbPoint[] {
  const baseline = experiment.baseline.test?.mean ?? 0
  let best = baseline
  const points: ClimbPoint[] = [{ number: 0, best }]
  for (const run of experiment.runs) {
    if (!settled(run)) continue
    if (run.verdict === "kept") best = run.test.mean
    points.push({ number: run.number, best })
  }
  return points
}

export type AreaState =
  /** Kept something among its last three settled attempts. */
  | "climbing"
  /** Three or more settled attempts in a row without a keep. */
  | "stalling"
  /** Five or more in a row: the agents should look elsewhere. */
  | "exhausted"
  /** Too few attempts to say. */
  | "early"

export interface AreaSummary {
  readonly area: Area
  readonly attempts: number
  readonly kept: number
  readonly live: number
  /** Test points its kept changes added to the best so far. */
  readonly gain: number
  readonly outcomes: readonly Verdict[]
  readonly state: AreaState
  readonly agents: readonly Agent[]
  /** Its most recent run, settled or not. */
  readonly latest?: Run
}

export function summarizeArea(experiment: Experiment, area: Area): AreaSummary {
  const runs = experiment.runs.filter((run) => run.areaId === area.id)
  const settledRuns = runs.filter(settled)
  const gain = pathToBest(experiment)
    .filter((step) => step.run.areaId === area.id)
    .reduce((sum, step) => sum + step.gain, 0)
  let dry = 0
  for (let index = settledRuns.length - 1; index >= 0; index -= 1) {
    if (settledRuns[index].verdict === "kept") break
    dry += 1
  }
  const state: AreaState =
    settledRuns.length < 2
      ? "early"
      : dry >= 5
        ? "exhausted"
        : dry >= 3
          ? "stalling"
          : "climbing"
  return {
    area,
    attempts: settledRuns.length,
    kept: settledRuns.filter((run) => run.verdict === "kept").length,
    live: runs.filter((run) => run.verdict === "running").length,
    gain,
    outcomes: runs.map((run) => run.verdict),
    state,
    agents: experiment.agents.filter((agent) => agent.areaId === area.id),
    latest: runs[runs.length - 1],
  }
}

export interface ExperimentTotals {
  readonly spent: number
  readonly settled: number
  readonly kept: number
  readonly reverted: number
  readonly overfit: number
  readonly live: number
  readonly queued: number
}

export function totals(experiment: Experiment): ExperimentTotals {
  const count = (verdict: Verdict) =>
    experiment.runs.filter((run) => run.verdict === verdict).length
  const settledCount = experiment.runs.filter(settled).length
  return {
    spent: settledCount + count("running"),
    settled: settledCount,
    kept: count("kept"),
    reverted: settledCount - count("kept"),
    overfit: count("overfit"),
    live: count("running"),
    queued: count("queued"),
  }
}

export const verdictLabels: Record<Verdict, string> = {
  kept: "Kept",
  overfit: "Overfit",
  regressed: "Regressed",
  flat: "Within noise",
  costly: "Too costly",
  running: "Evaluating",
  queued: "Queued",
}

export const areaStateLabels: Record<AreaState, string> = {
  climbing: "Climbing",
  stalling: "Stalling",
  exhausted: "Exhausted",
  early: "Early",
}

/** Why a settled run was kept or reverted, in the hill-climber's words. */
export function verdictReason(experiment: Experiment, run: Run): string {
  const test = testDelta(experiment, run)
  const train = trainDelta(experiment, run)
  const noise = experiment.noise.toFixed(1)
  const pts = (value: number | undefined) =>
    value === undefined ? "–" : `${value >= 0 ? "+" : "−"}${Math.abs(value).toFixed(1)}`
  switch (run.verdict) {
    case "kept":
      return `Train ${pts(train)} and test ${pts(test)}, both clear of the ±${noise} noise. This is the new best.`
    case "overfit":
      return `Train ${pts(train)} but test only ${pts(test)}: it learned the train cases, not the task. Reverted.`
    case "regressed":
      return `Test ${pts(test)}, past the ±${noise} noise in the wrong direction. Reverted.`
    case "flat":
      return `Test ${pts(test)} sits inside the ±${noise} noise: no detectable change. Reverted.`
    case "costly":
      return `Test ${pts(test)}, but cost per task rose beyond the goal's limit. Reverted.`
    case "running":
      return "Being graded now. It is compared with its parent when the last case is in."
    case "queued":
      return "Written and waiting for a free evaluator."
  }
}
