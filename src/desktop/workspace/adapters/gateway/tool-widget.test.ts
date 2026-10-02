import type { ConversationTool, McpAppsApi } from "@nessa/client"
import { describe, expect, it } from "vitest"
import { createWidgetRegistry, gatewayApps, type WidgetPlugin } from "../../../widgets"
import { gatewayToolWidget } from "./tool-widget"

const conversation = "0b9a3c1e-5d2f-4a7b-8c6d-1e2f3a4b5c6d"

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
    expect(gatewayToolWidget(conversation, shown)).toEqual({
      kind: "widget",
      widget: {
        plugin: "mcp:mcptest",
        id: JSON.stringify([conversation, "run", "call-1"]),
      },
    })
  })

  it("is none for a call without MCP identity, or an MCP tool without UI", () => {
    expect(gatewayToolWidget(conversation, tool)).toBeNull()
    expect(
      gatewayToolWidget(conversation, {
        ...tool,
        mcp: { server: "mcptest", tool: "report_rows" },
      }),
    ).toBeNull()
  })

  it("names the call the tool's app reads from the same view (#384, C1)", () => {
    const shown = {
      ...tool,
      mcp: {
        server: "mcptest",
        tool: "show_chart",
        resourceUri: "ui://nessa-test/chart.html",
      },
    }
    const registry = createWidgetRegistry<WidgetPlugin>([])
    gatewayApps({
      registry,
      mcpApps: {} as McpAppsApi,
      ports: {
        timers: { after: () => () => {} },
        newId: () => crypto.randomUUID(),
        hostInfo: { name: "Nessa", version: "test" },
        page: () => ({ styles: {}, timeZone: "UTC", platform: "web" }),
      },
    }).observe(conversation, [shown])
    const part = gatewayToolWidget(conversation, shown)
    const plugin = part ? registry.plugin(part.widget.plugin) : undefined
    expect(plugin?.kind).toBe("app")
    if (plugin?.kind !== "app" || !part) return
    expect(plugin.ports.calls.read(part.widget.id)).toMatchObject({
      kind: "known",
      call: { executionId: "run", toolId: "call-1", tool: "show_chart" },
    })
  })
})
