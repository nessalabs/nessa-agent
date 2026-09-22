import { expect, it } from "vitest"
import { workSummary } from "./work-activity"
import type { AgentToolView } from "./agent-transcript-view"

function tool(status: string): AgentToolView {
  return { callId: status, title: "Shell", status, input: "{}", details: "" }
}

it("counts a turn's tools once, and never its failures", () => {
  const work = [
    { key: "a", thought: "Trying." },
    { key: "b", tool: tool("failed") },
    { key: "c", tool: tool("completed") },
    { key: "d", tool: tool("failed") },
  ]
  expect(workSummary({ work, running: false })).toBe("Ran 3 tools")
  expect(
    workSummary({ work: [{ key: "a", tool: tool("completed") }], running: false }),
  ).toBe("Ran 1 tool")
})

it("says a turn is working without listing the steps it takes", () => {
  const thinking = [{ key: "a", thought: "Weighing it up." }]
  expect(workSummary({ work: thinking, running: true })).toBe("Thinking")
  expect(workSummary({ work: thinking, running: false })).toBe("Thought")
  expect(
    workSummary({ work: [{ key: "a", tool: tool("completed") }], running: true }),
  ).toBe("Running…")
  expect(workSummary({ work: [], running: false })).toBe("")
})

it("does not claim a stopped tool ran to completion", () => {
  const work = [
    { key: "a", tool: tool("stopped") },
    { key: "b", tool: tool("completed") },
  ]
  expect(workSummary({ work, running: false })).toBe("2 tools")
})
