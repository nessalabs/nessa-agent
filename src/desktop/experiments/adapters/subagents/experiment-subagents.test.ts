import { describe, expect, it } from "vitest"
import { liveExperimentSource } from "../in-memory/live-experiment"
import { experimentSubagents } from "./experiment-subagents"

function sources() {
  let tick: (() => void) | null = null
  let clock = 1_000_000
  const experiments = liveExperimentSource({
    now: () => clock,
    every: (_ms, run) => {
      tick = run
      return () => {
        tick = null
      }
    },
  })
  return {
    subagents: experimentSubagents(experiments, { now: () => clock, after: () => {} }),
    step: () => {
      clock += 450
      tick?.()
    },
  }
}

describe("an experiment's swarm, read as its conversation's subagents", () => {
  it("is found by the conversation, and nowhere else", () => {
    const { subagents } = sources()
    expect(subagents.forSession("checkout-hillclimb")?.subagents).toHaveLength(6)
    expect(subagents.forSession("retry-budget")).toBeUndefined()
  })

  it("reads the same value until the experiment changes", () => {
    const { subagents, step } = sources()
    const stop = subagents.subscribe(() => {})
    const first = subagents.forSession("checkout-hillclimb")
    expect(subagents.forSession("checkout-hillclimb")).toBe(first)
    step()
    expect(subagents.forSession("checkout-hillclimb")).not.toBe(first)
    stop()
  })

  it("says what each agent is on, and what it has done", () => {
    const { subagents } = sources()
    const sable = subagents
      .forSession("checkout-hillclimb")
      ?.subagents.find((each) => each.id === "sable")
    expect(sable?.state).toBe("working")
    expect(sable?.work?.title).toBe("Validate refund amounts before calling the tool")
    expect(sable?.conversation.messages[0]?.role).toBe("user")
    expect(sable?.conversation.activity?.label).toMatch(/^Evaluating #36/)
  })
})
