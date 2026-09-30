import { describe, expect, it } from "vitest"
import {
  bestSoFar,
  champion,
  lineage,
  pathToBest,
  summarizeArea,
  testDelta,
  totals,
  verdictReason,
  type Area,
  type Experiment,
  type Run,
  type SettledVerdict,
} from "./experiment"

const area = (id: string, slot: Area["slot"]): Area => ({
  id,
  name: id,
  hypothesis: "",
  slot,
})

const baseline: Run = {
  id: "r0",
  number: 0,
  parentId: null,
  areaId: "prompt",
  agentId: "a",
  title: "Baseline",
  rationale: "",
  change: { files: [] },
  startedAt: 0,
  verdict: "kept",
  train: { mean: 50, ci: 2 },
  test: { mean: 50, ci: 2 },
}

function settledRun(
  number: number,
  parentId: string,
  areaId: string,
  test: number,
  verdict: SettledVerdict,
): Run {
  return {
    ...baseline,
    id: `r${number}`,
    number,
    parentId,
    areaId,
    title: `Change ${number}`,
    verdict,
    train: { mean: test + 1, ci: 2 },
    test: { mean: test, ci: 2 },
  }
}

function experiment(runs: Run[]): Experiment {
  return {
    id: "x",
    title: "x",
    goal: "",
    metric: "",
    suite: { name: "", train: 10, test: 10, grader: "" },
    noise: 1,
    ceiling: 90,
    budget: 20,
    startedAt: 0,
    asOf: 0,
    baseline,
    runs,
    areas: [area("prompt", 1), area("tools", 2)],
    agents: [],
    notes: [],
  }
}

describe("an experiment's best so far", () => {
  const runs = [
    settledRun(1, "r0", "prompt", 53, "kept"),
    settledRun(2, "r1", "tools", 52, "regressed"),
    settledRun(3, "r1", "tools", 56, "kept"),
    { ...settledRun(4, "r3", "prompt", 57, "overfit"), train: { mean: 63, ci: 2 } },
    {
      ...settledRun(5, "r3", "prompt", 0, "flat"),
      verdict: "running" as const,
      test: undefined,
      train: undefined,
    },
  ]
  const x = experiment(runs)

  it("is the last kept change, not the highest score", () => {
    expect(champion(x).id).toBe("r3")
  })

  it("climbs only on kept changes and skips runs still evaluating", () => {
    expect(bestSoFar(x)).toEqual([
      { number: 0, best: 50 },
      { number: 1, best: 53 },
      { number: 2, best: 53 },
      { number: 3, best: 56 },
      { number: 4, best: 56 },
    ])
  })

  it("is reached by the kept changes, each with what it added", () => {
    expect(pathToBest(x).map((step) => [step.run.id, step.gain])).toEqual([
      ["r1", 3],
      ["r3", 3],
    ])
    expect(lineage(x, runs[3]).map((run) => run.id)).toEqual(["r0", "r1", "r3", "r4"])
  })

  it("counts what was spent, kept and reverted", () => {
    expect(totals(x)).toEqual({
      spent: 5,
      settled: 4,
      kept: 2,
      reverted: 2,
      overfit: 1,
      live: 1,
      queued: 0,
    })
  })

  it("measures a change against its parent", () => {
    expect(testDelta(x, runs[1])).toBe(-1)
    expect(verdictReason(x, runs[3])).toContain("Train +6.0 but test only +1.0")
  })
})

describe("an area's state", () => {
  it("is early with fewer than two attempts", () => {
    const x = experiment([settledRun(1, "r0", "tools", 49, "flat")])
    expect(summarizeArea(x, x.areas[1]).state).toBe("early")
  })

  it("stalls after three settled attempts without a keep, and is exhausted after five", () => {
    const tries = (count: number) =>
      experiment([
        settledRun(1, "r0", "tools", 52, "kept"),
        ...Array.from({ length: count }, (_, index) =>
          settledRun(index + 2, "r1", "tools", 51, "flat"),
        ),
      ])
    const three = tries(3)
    const five = tries(5)
    expect(summarizeArea(three, three.areas[1]).state).toBe("stalling")
    expect(summarizeArea(five, five.areas[1]).state).toBe("exhausted")
    expect(summarizeArea(five, five.areas[1]).gain).toBe(2)
  })
})
