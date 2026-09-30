import { describe, expect, it } from "vitest"
import { champion, settled } from "../../model/experiment"
import { liveExperimentSource } from "./live-experiment"
import { sampleExperimentId } from "./sample-experiment"

function steppedSource() {
  let clock = 1_000_000
  let tick: (() => void) | null = null
  const source = liveExperimentSource({
    now: () => clock,
    every: (_ms, run) => {
      tick = run
      return () => {
        tick = null
      }
    },
  })
  const step = (times: number) => {
    for (let index = 0; index < times; index += 1) {
      clock += 450
      tick?.()
    }
  }
  return { source, step, stopped: () => tick === null }
}

describe("the live sample experiment", () => {
  it("settles the run closest to done into a new best, and says so", () => {
    const { source, step } = steppedSource()
    const stop = source.subscribe(() => {})
    const before = source.get(sampleExperimentId)
    step(12)
    const after = source.get(sampleExperimentId)
    stop()
    expect(before && champion(before).test.mean).toBe(71.6)
    // #36, 150 of 180 cases in, is scripted to be kept at +1.6.
    expect(after && champion(after).title).toBe(
      "Validate refund amounts before calling the tool",
    )
    expect(after && champion(after).test.mean).toBe(73.2)
    expect(after?.notes.at(-1)?.tone).toBe("good")
  })

  it("never spends past its budget", () => {
    const { source, step } = steppedSource()
    const stop = source.subscribe(() => {})
    step(2_000)
    const experiment = source.get(sampleExperimentId)
    stop()
    const spent =
      experiment?.runs.filter((run) => settled(run) || run.verdict === "running")
        .length ?? 0
    expect(spent).toBeLessThanOrEqual(experiment?.budget ?? 0)
  })

  it("stops its timer when nothing follows it", () => {
    const { source, stopped } = steppedSource()
    const stop = source.subscribe(() => {})
    expect(stopped()).toBe(false)
    stop()
    expect(stopped()).toBe(true)
  })
})
