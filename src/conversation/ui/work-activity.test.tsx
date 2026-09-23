import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it, vi } from "vitest"
import { WorkActivity, workSummary } from "./work-activity"
import WorkSteps from "./work-steps"
import type { AgentToolView } from "./agent-transcript-view"

function tool(status: string, kind = "other"): AgentToolView {
  return {
    callId: `${kind}:${status}`,
    title: "Shell",
    kind,
    status,
    input: "{}",
    details: "",
  }
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
  ).toBe("Running")
  expect(workSummary({ work: [], running: false })).toBe("")
})

it("does not claim a stopped tool ran to completion", () => {
  const work = [
    { key: "a", tool: tool("stopped") },
    { key: "b", tool: tool("completed") },
  ]
  expect(workSummary({ work, running: false })).toBe("2 tools")
})

it("says what the turn read and what it wrote, so a reader knows their files were touched", () => {
  const work = [
    { key: "a", tool: tool("completed", "file_read") },
    { key: "b", tool: tool("failed", "file_read") },
    { key: "c", tool: tool("completed", "file_edit") },
    { key: "d", tool: tool("completed", "shell") },
  ]
  expect(workSummary({ work, running: false })).toBe("Ran 4 tools · 2 read, 1 written")
})

it("counts only the kinds the provider named, never guessing at the rest", () => {
  // An unsaid kind arrives as `other`; calling it a read would put a number
  // on the line that is not true.
  const work = [
    { key: "a", tool: tool("completed") },
    { key: "b", tool: tool("completed", "search") },
  ]
  expect(workSummary({ work, running: false })).toBe("Ran 2 tools")
})

describe("the rendered line and sheet", () => {
  const work = [
    { key: "thought", thought: "Inspect the project." },
    {
      key: "failed-tool",
      tool: {
        callId: "failed-tool",
        title: "Search",
        kind: "search",
        status: "failed",
        input: "needle",
        details: "No matches",
      },
    },
    {
      key: "completed-tool",
      tool: {
        callId: "completed-tool",
        title: "Read",
        kind: "file_read",
        status: "completed",
        input: "README.md",
        details: "Project notes",
      },
    },
  ]
  const line = (running: boolean) =>
    renderToStaticMarkup(
      <WorkActivity
        work={work}
        running={running}
        seed="chat"
        expanded={false}
        sheetId="sheet"
        onOpen={vi.fn()}
      />,
    )

  it("collapses thoughts and tools into one neutral line once the turn is done", () => {
    const markup = line(false)
    expect(markup.match(/data-slot="agent-activity"/g)).toHaveLength(1)
    expect(markup).toContain('data-status="complete"')
    expect(markup).toContain("Ran 2 tools")
    expect(markup).not.toContain("failed")
    expect(markup).not.toContain('data-status="error"')
  })

  it("shows one working status with no interim count and nothing to open", () => {
    const markup = line(true)
    expect(markup.match(/data-slot="agent-activity"/g)).toHaveLength(1)
    expect(markup).toContain("Running")
    expect(markup).toContain('aria-busy="true"')
    expect(markup).not.toContain("tool")
    expect(markup).not.toContain("<button")
  })

  it("keeps the thought and the failed call, in order, inside the sheet", () => {
    const markup = renderToStaticMarkup(<WorkSteps work={work} />)
    expect(markup.indexOf("Inspect the project.")).toBeLessThan(markup.indexOf("Search"))
    expect(markup.indexOf("Search")).toBeLessThan(markup.indexOf("Read"))
    expect(markup).toContain('data-status="error"')
  })
})
