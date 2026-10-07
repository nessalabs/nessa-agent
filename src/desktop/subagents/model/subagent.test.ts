import { describe, expect, it } from "vitest"
import {
  activityCounts,
  byActivity,
  progressFraction,
  rowStatus,
  summaryLine,
  type Subagent,
} from "./subagent"

function child(over: Partial<Subagent> & Pick<Subagent, "id" | "activity">): Subagent {
  return {
    name: over.id,
    seed: over.id,
    tags: [],
    headline: "",
    lifecycle: "open",
    since: 0,
    conversation: { messages: [], activity: null },
    ...over,
  }
}

describe("subagent order", () => {
  it("lists working, then planning, stuck, and idle", () => {
    const ordered = byActivity([
      child({ id: "idle", activity: "idle" }),
      child({ id: "stuck", activity: "stuck" }),
      child({ id: "planning", activity: "planning" }),
      child({ id: "working", activity: "working" }),
    ])
    expect(ordered.map((each) => each.id)).toEqual([
      "working",
      "planning",
      "stuck",
      "idle",
    ])
  })

  it("orders working children by measured progress, furthest first", () => {
    const ordered = byActivity([
      child({ id: "none", activity: "working" }),
      child({ id: "quarter", activity: "working", progress: { done: 1, total: 4 } }),
      child({ id: "half", activity: "working", progress: { done: 1, total: 2 } }),
      child({ id: "empty", activity: "working", progress: { done: 1, total: 0 } }),
    ])
    expect(ordered.map((each) => each.id)).toEqual(["half", "quarter", "none", "empty"])
    expect(progressFraction({ done: 1, total: 0 })).toBe(0)
  })

  it("keeps a tie in the order it was given", () => {
    const ordered = byActivity([
      child({ id: "a", activity: "planning" }),
      child({ id: "b", activity: "planning" }),
    ])
    expect(ordered.map((each) => each.id)).toEqual(["a", "b"])
  })
})

describe("subagent counts", () => {
  it("counts open children by activity and says the others by lifetime", () => {
    const subagents = [
      child({ id: "w1", activity: "working" }),
      child({ id: "w2", activity: "working" }),
      child({ id: "p", activity: "planning" }),
      child({ id: "gone", activity: "idle", lifecycle: "closed" }),
      child({ id: "soon", activity: "working", lifecycle: "starting" }),
    ]
    expect(activityCounts(subagents)).toEqual({
      working: 2,
      planning: 1,
      stuck: 0,
      idle: 0,
    })
    expect(summaryLine(subagents)).toBe("2 working · 1 planning · 1 starting · 1 closed")
  })

  it("does not call a closed child idle", () => {
    const closed = child({ id: "gone", activity: "idle", lifecycle: "closed" })
    expect(rowStatus(closed).word).toBe("Closed")
    expect(rowStatus(child({ id: "open", activity: "idle" })).word).toBe("Idle")
    expect(
      rowStatus(child({ id: "end", activity: "working", lifecycle: "closing" })).word,
    ).toBe("Closing")
  })
})
