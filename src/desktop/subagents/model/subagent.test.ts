import { describe, expect, it } from "vitest"
import { byActivity, stateCounts, type Subagent, type SubagentState } from "./subagent"

function subagent(id: string, state: SubagentState, done?: number): Subagent {
  return {
    id,
    name: id,
    seed: id,
    state,
    headline: "",
    since: 0,
    ...(done === undefined
      ? {}
      : { work: { title: "", startedAt: 0, progress: { done, total: 100 } } }),
    conversation: { messages: [], activity: null },
  }
}

describe("a conversation's subagents", () => {
  const swarm = [
    subagent("resting", "resting"),
    subagent("slow", "working", 10),
    subagent("stuck", "stuck"),
    subagent("fast", "working", 90),
    subagent("thinking", "thinking"),
  ]

  it("read those at work first, furthest along leading, then thinking, stuck, resting", () => {
    expect(byActivity(swarm).map((each) => each.id)).toEqual([
      "fast",
      "slow",
      "thinking",
      "stuck",
      "resting",
    ])
  })

  it("are counted by state", () => {
    expect(stateCounts(swarm)).toEqual({ working: 2, thinking: 1, stuck: 1, resting: 1 })
  })
})
