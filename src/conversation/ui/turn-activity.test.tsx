import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it, vi } from "vitest"
import type { AgentTurnActivityItem } from "./agent-transcript-view"
import { TurnActivity } from "./turn-activity"
import TurnActivityRows from "./turn-activity-rows"

const items: AgentTurnActivityItem[] = [
  { key: "thought", kind: "thought", text: "Inspect the project." },
  {
    key: "failed-tool",
    kind: "tool",
    tool: {
      callId: "failed-tool",
      title: "Search",
      status: "failed",
      input: "needle",
      details: "No matches",
    },
  },
  {
    key: "completed-tool",
    kind: "tool",
    tool: {
      callId: "completed-tool",
      title: "Read",
      status: "completed",
      input: "README.md",
      details: "Project notes",
    },
  },
]

describe("turn activity summary", () => {
  it("collapses thoughts and tools into one neutral completed disclosure", () => {
    const markup = renderToStaticMarkup(
      <TurnActivity items={items} running={false} onOpen={vi.fn()} />,
    )
    expect(markup.match(/data-slot="agent-activity"/g)).toHaveLength(1)
    expect(markup).toContain('data-status="complete"')
    expect(markup).toContain("Ran 2 tools")
    expect(markup).not.toContain("failed")
    expect(markup).not.toContain('data-status="error"')
  })

  it("shows one running disclosure without exposing an interim tool count", () => {
    const markup = renderToStaticMarkup(
      <TurnActivity items={items} running={true} onOpen={vi.fn()} />,
    )
    expect(markup.match(/data-slot="agent-activity"/g)).toHaveLength(1)
    expect(markup).toContain("Running…")
    expect(markup).toContain('aria-busy="true"')
    expect(markup).not.toContain("tool")
  })
})

it("keeps ordered thought and failed tool detail inside the expansion", () => {
  const markup = renderToStaticMarkup(<TurnActivityRows items={items} />)
  expect(markup.indexOf("Inspect the project.")).toBeLessThan(markup.indexOf("Search"))
  expect(markup.indexOf("Search")).toBeLessThan(markup.indexOf("Read"))
  expect(markup).toContain('data-status="error"')
})
