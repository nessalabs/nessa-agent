/**
 * The calls an app's widgets name, from a gateway conversation's view: rows
 * C1–C9 of the state table on #384. That the widget ids are the transcript's
 * own is held beside the transcript's reader (`tool-widget.test.ts`).
 */
import type { ConversationTool, McpAppsApi } from "@nessa/client"
import { describe, expect, it, vi } from "vitest"
import { createWidgetRegistry } from "../../../application/registry"
import type { WidgetPlugin } from "../../../ui/plugin"
import { appPluginId } from "../../model/app-ref"
import { gatewayAppCall, gatewayAppCalls } from "./app-calls"
import { gatewayApps } from "./gateway-apps"

const conversationId = "0b9a3c1e-5d2f-4a7b-8c6d-1e2f3a4b5c6d"

function tool(overrides: Partial<ConversationTool> = {}): ConversationTool {
  return {
    executionId: "execution-1",
    toolId: "call-1",
    title: "show_chart",
    kind: "other",
    status: "running",
    details: "",
    input: "",
    mcp: {
      server: "mcptest",
      tool: "show_chart",
      resourceUri: "ui://nessa-test/chart.html",
    },
    ...overrides,
  }
}

describe("a view's tool as an app call", () => {
  it("C1: a tool with a UI is its server's call, under its widget's id", () => {
    const app = gatewayAppCall(conversationId, tool())
    expect(app).toEqual({
      server: "mcptest",
      widgetId: JSON.stringify([conversationId, "execution-1", "call-1"]),
      call: {
        sessionId: conversationId,
        executionId: "execution-1",
        toolId: "call-1",
        tool: "show_chart",
        resourceUri: "ui://nessa-test/chart.html",
        phase: { kind: "running", arguments: {} },
      },
    })
  })

  it("C2: a tool with no MCP server, or with no UI, is no app call", () => {
    const { mcp: _, ...harness } = tool()
    expect(gatewayAppCall(conversationId, harness)).toBeNull()
    expect(
      gatewayAppCall(conversationId, tool({ mcp: { server: "mcptest", tool: "rows" } })),
    ).toBeNull()
  })

  it.each(["pending", "running", "", "something-new"])(
    "C3: a call %j is still running",
    (status) => {
      expect(gatewayAppCall(conversationId, tool({ status }))?.call.phase.kind).toBe(
        "running",
      )
    },
  )

  it("C4: a completed call is done, with its text and its structured content", () => {
    const app = gatewayAppCall(
      conversationId,
      tool({
        status: "completed",
        details: "Three rows",
        structuredContent: '{"rows":3}',
        input: '{"title":"Chart"}',
      }),
    )
    expect(app?.call.phase).toEqual({
      kind: "done",
      arguments: { title: "Chart" },
      result: {
        content: [{ type: "text", text: "Three rows" }],
        structuredContent: { rows: 3 },
      },
    })
  })

  it("C5: a failed call is done with an error result", () => {
    expect(
      gatewayAppCall(conversationId, tool({ status: "failed", details: "No" }))?.call
        .phase,
    ).toEqual({
      kind: "done",
      arguments: {},
      result: { content: [{ type: "text", text: "No" }], isError: true },
    })
  })

  it("C6: arguments are the input when it is a JSON object, else none", () => {
    for (const input of ["", "not json", "[1]", "3", "null"])
      expect(gatewayAppCall(conversationId, tool({ input }))?.call.phase).toEqual({
        kind: "running",
        arguments: {},
      })
    expect(
      gatewayAppCall(conversationId, tool({ input: '{"a":1}' }))?.call.phase,
    ).toEqual({ kind: "running", arguments: { a: 1 } })
  })

  it("C4: structured content that is not a JSON object is left out", () => {
    expect(
      gatewayAppCall(
        conversationId,
        tool({ status: "completed", structuredContent: "[1]" }),
      )?.call.phase,
    ).toEqual({ kind: "done", arguments: {}, result: { content: [] } })
  })
})

describe("the calls of each server", () => {
  const id = JSON.stringify([conversationId, "execution-1", "call-1"])

  it("C1, C7: a call is read once observed; an unchanged view keeps its value and tells no one", () => {
    const calls = gatewayAppCalls()
    const port = calls.forServer("mcptest")
    const told = vi.fn()
    port.subscribe(id, told)
    expect(port.read(id)).toEqual({ kind: "missing" })
    expect(calls.observe({ conversationId: conversationId, tools: [tool()] })).toEqual([
      "mcptest",
    ])
    const first = port.read(id)
    expect(first).toMatchObject({ kind: "known", call: { tool: "show_chart" } })
    expect(told).toHaveBeenCalledTimes(1)
    calls.observe({ conversationId: conversationId, tools: [tool()] })
    expect(port.read(id)).toBe(first)
    expect(told).toHaveBeenCalledTimes(1)
    calls.observe({
      conversationId: conversationId,
      tools: [tool({ status: "completed" })],
    })
    expect(port.read(id)).toMatchObject({
      kind: "known",
      call: { phase: { kind: "done" } },
    })
    expect(told).toHaveBeenCalledTimes(2)
  })

  it("C9: a widget id read through another server's plugin is missing", () => {
    const calls = gatewayAppCalls()
    calls.observe({
      conversationId: conversationId,
      tools: [
        tool(),
        tool({
          toolId: "call-2",
          mcp: { server: "other", tool: "x", resourceUri: "ui://o" },
        }),
      ],
    })
    expect(calls.forServer("other").read(id)).toEqual({ kind: "missing" })
    expect(
      calls
        .forServer("mcptest")
        .read(JSON.stringify([conversationId, "execution-1", "call-2"])),
    ).toEqual({ kind: "missing" })
  })

  it("C10: the same execution and tool ids in two conversations are two calls, each read as its own", () => {
    const other = "9f8e7d6c-5b4a-4c3d-8e2f-1a0b9c8d7e6f"
    const calls = gatewayAppCalls()
    const port = calls.forServer("mcptest")
    calls.observe({ conversationId: conversationId, tools: [tool()] })
    calls.observe({ conversationId: other, tools: [tool({ status: "completed" })] })
    calls.observe({ conversationId: conversationId, tools: [tool()] })
    expect(port.read(id)).toMatchObject({
      kind: "known",
      call: { sessionId: conversationId, phase: { kind: "running" } },
    })
    expect(port.read(JSON.stringify([other, "execution-1", "call-1"]))).toMatchObject({
      kind: "known",
      call: { sessionId: other, phase: { kind: "done" } },
    })
  })

  it("C11: a call the view no longer reports — evicted by later tools — keeps its last state, and no one is told", () => {
    const calls = gatewayAppCalls()
    const port = calls.forServer("mcptest")
    const told = vi.fn()
    port.subscribe(id, told)
    calls.observe({ conversationId: conversationId, tools: [tool()] })
    const first = port.read(id)
    // The gateway keeps its latest tools: sixteen more push the app's out.
    const later = Array.from({ length: 16 }, (_, n) => {
      const { mcp: _mcp, ...plain } = tool({ toolId: `later-${n}` })
      return plain
    })
    expect(calls.observe({ conversationId: conversationId, tools: later })).toEqual([])
    expect(port.read(id)).toBe(first)
    expect(told).toHaveBeenCalledTimes(1)
  })

  it("C11: a conversation forgotten takes its calls, and their readers are told; another's stay", () => {
    const other = "9f8e7d6c-5b4a-4c3d-8e2f-1a0b9c8d7e6f"
    const calls = gatewayAppCalls()
    const port = calls.forServer("mcptest")
    const told = vi.fn()
    port.subscribe(id, told)
    calls.observe({ conversationId: conversationId, tools: [tool()] })
    calls.observe({ conversationId: other, tools: [tool()] })
    calls.forget(other)
    expect(port.read(id).kind).toBe("known")
    expect(told).toHaveBeenCalledTimes(1)
    calls.forget(conversationId)
    calls.forget(conversationId)
    expect(port.read(id)).toEqual({ kind: "missing" })
    expect(told).toHaveBeenCalledTimes(2)
    // A view of it arriving after it went does not bring it back.
    expect(calls.observe({ conversationId: conversationId, tools: [tool()] })).toEqual([])
    expect(port.read(id)).toEqual({ kind: "missing" })
    expect(told).toHaveBeenCalledTimes(2)
  })

  it("C11: each view's state is read as it comes: the order of views is the source's (#248)", () => {
    const calls = gatewayAppCalls()
    const port = calls.forServer("mcptest")
    calls.observe({
      conversationId,
      tools: [tool({ status: "completed", details: "Done" })],
    })
    calls.observe({ conversationId, tools: [tool({ status: "running" })] })
    expect(port.read(id)).toMatchObject({ call: { phase: { kind: "running" } } })
  })

  it("C7: a stopped subscription is not told", () => {
    const calls = gatewayAppCalls()
    const told = vi.fn()
    const stop = calls.forServer("mcptest").subscribe(id, told)
    stop()
    calls.observe({ conversationId: conversationId, tools: [tool()] })
    expect(told).not.toHaveBeenCalled()
  })
})

describe("the servers' app plugins", () => {
  const mcpApps = {} as McpAppsApi
  const ports = {
    timers: { after: () => () => {} },
    newId: () => crypto.randomUUID(),
    hostInfo: { name: "Nessa", version: "test" },
    page: () => ({ styles: {}, timeZone: "UTC", platform: "web" as const }),
  }

  it("C8: a server seen for the first time has its app registered, once, reading its own calls", () => {
    const registry = createWidgetRegistry<WidgetPlugin>([])
    const changes = vi.fn()
    registry.subscribe(changes)
    const apps = gatewayApps({ registry, mcpApps, ports })
    apps.observe({ conversationId: conversationId, tools: [tool()] })
    apps.observe({
      conversationId: conversationId,
      tools: [tool({ status: "completed" })],
    })
    expect(changes).toHaveBeenCalledTimes(1)
    const plugin = registry.plugin(appPluginId("mcptest"))
    expect(plugin).toMatchObject({ kind: "app", server: "mcptest", name: "mcptest" })
    if (plugin?.kind !== "app") return
    expect(
      plugin.ports.calls.read(JSON.stringify([conversationId, "execution-1", "call-1"])),
    ).toMatchObject({
      kind: "known",
      call: { phase: { kind: "done" } },
    })
    expect(plugin.ports.server.callWithin).toBeGreaterThan(300_000)
  })

  it("C8: a server's app waits on the host's own clock before asking a release again", async () => {
    const registry = createWidgetRegistry<WidgetPlugin>([])
    const waits: number[] = []
    const releaseApp = vi
      .fn<McpAppsApi["releaseApp"]>()
      .mockRejectedValueOnce(new Error("socket closed"))
      .mockResolvedValue({ requestId: "r", applied: true })
    gatewayApps({
      registry,
      mcpApps: { ...mcpApps, releaseApp },
      ports: {
        ...ports,
        timers: {
          after: (ms, run) => {
            waits.push(ms)
            run()
            return () => {}
          },
        },
      },
    }).observe({ conversationId: conversationId, tools: [tool()] })
    const plugin = registry.plugin(appPluginId("mcptest"))
    if (plugin?.kind !== "app") throw new Error("no app plugin")
    await plugin.ports.server.release({
      sessionId: conversationId,
      server: "mcptest",
      app: {
        executionId: "execution-1",
        toolId: "call-1",
        instanceId: crypto.randomUUID(),
      },
    } as Parameters<typeof plugin.ports.server.release>[0])
    expect(releaseApp).toHaveBeenCalledTimes(2)
    expect(waits).toEqual([500])
  })

  it("C2, C8: a view with no app calls registers nothing", () => {
    const registry = createWidgetRegistry<WidgetPlugin>([])
    gatewayApps({ registry, mcpApps, ports }).observe({
      conversationId: conversationId,
      tools: [tool({ mcp: { server: "mcptest", tool: "rows" } })],
    })
    expect(registry.plugin(appPluginId("mcptest"))).toBeUndefined()
  })
})
