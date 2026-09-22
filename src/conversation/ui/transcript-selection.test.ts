import { expect, it } from "vitest"
import { selectedWork } from "./tool-selection"
import { workSummary } from "./tool-activity"

it("drops selected working when its replacement snapshot removes the segment", () => {
  const selected = { key: "work", tools: [{ callId: "tool" }] }
  expect(selectedWork([selected], selected.key)).toBe(selected)
  expect(selectedWork([], selected.key)).toBeUndefined()
  expect(selectedWork([{ key: selected.key }], selected.key)).toBeUndefined()
})

it("keeps a thought-only segment selectable", () => {
  const thought = { key: "work", thought: "Considering." }
  expect(selectedWork([thought], thought.key)).toBe(thought)
})

it("counts a turn's tools once, and never its failures", () => {
  const tools = [{ status: "failed" }, { status: "completed" }, { status: "failed" }]
  expect(workSummary({ tools, thought: "Trying." } as never)).toBe("Ran 3 tools")
  expect(workSummary({ tools: [{ status: "completed" }] } as never)).toBe("Ran 1 tool")
})

it("says a turn is working without listing the steps", () => {
  expect(
    workSummary({ tools: [{ status: "completed" }, { status: "running" }] } as never),
  ).toBe("Running…")
  expect(workSummary({ thought: "Weighing it up." })).toBe("Thought")
  expect(workSummary({})).toBe("")
})
