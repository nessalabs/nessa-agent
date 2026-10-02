import type { ConversationTool } from "@nessa/client"
import { describe, expect, it } from "vitest"
import { gatewayToolWidget } from "./tool-widget"

const tool: ConversationTool = {
  executionId: "run",
  toolId: "call-1",
  title: "mcp.mcptest.show_chart",
  kind: "execute",
  status: "completed",
  details: "Chart of two rows.",
  input: "",
}

describe("a gateway tool call as a widget part", () => {
  it("is the tool's app when the gateway read a UI resource for it", () => {
    const shown = {
      ...tool,
      mcp: {
        server: "mcptest",
        tool: "show_chart",
        resourceUri: "ui://nessa-test/chart.html",
      },
    }
    expect(gatewayToolWidget(shown)).toEqual({
      kind: "widget",
      widget: { plugin: "mcp:mcptest", id: JSON.stringify(["run", "call-1"]) },
    })
  })

  it("is none for a call without MCP identity, or an MCP tool without UI", () => {
    expect(gatewayToolWidget(tool)).toBeNull()
    expect(
      gatewayToolWidget({ ...tool, mcp: { server: "mcptest", tool: "report_rows" } }),
    ).toBeNull()
  })
})
