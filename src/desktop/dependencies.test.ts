// @vitest-environment jsdom
/**
 * Composition registers the sample widget plugin only beside the sample
 * workspace, whose session its widgets belong to, and the fixture MCP App
 * there only where apps are drawn; a window on another source starts with
 * the plugins it is given, and two under one id stop it. With a gateway,
 * real servers' apps are registered as the views name them.
 */
import type { McpAppsApi } from "@nessa/client"
import { describe, expect, it, vi } from "vitest"
import { createDesktopDependencies } from "./dependencies"
import {
  appPluginId,
  fixtureServer,
  samplePlugin,
  samplePluginId,
  WidgetRegistryError,
} from "./widgets"
import { deferred, fakeGateway, view } from "./workspace/adapters/gateway/fake-gateway"
import { fakeSource } from "./workspace/testing"

describe("the window's widget plugins", () => {
  it("are the sample plugin's while the sample workspace is in use", () => {
    const { widgets } = createDesktopDependencies()
    expect(widgets.natives().map((plugin) => plugin.id)).toEqual([samplePluginId])
  })

  it("are none of the samples' on another source", () => {
    const { widgets } = createDesktopDependencies({ workspace: fakeSource() })
    expect(widgets.natives()).toEqual([])
    expect(widgets.plugin(samplePluginId)).toBeUndefined()
  })

  it("are the ones composition names, and two under one id stop the window", () => {
    const named = samplePlugin("a")
    const { widgets } = createDesktopDependencies({
      workspace: fakeSource(),
      widgets: [named],
    })
    expect(widgets.natives()).toEqual([named])
    expect(() =>
      createDesktopDependencies({ workspace: fakeSource(), widgets: [named, named] }),
    ).toThrow(WidgetRegistryError)
  })

  it("register the fixture MCP App beside the sample workspace, only where apps are drawn", () => {
    const fixtureApp = appPluginId(fixtureServer)
    expect(createDesktopDependencies().widgets.plugin(fixtureApp)).toBeUndefined()
    const { widgets } = createDesktopDependencies({
      apps: { sandbox: undefined, platform: "web" },
    })
    expect(widgets.plugin(fixtureApp)).toMatchObject({
      kind: "app",
      server: fixtureServer,
    })
    expect(
      createDesktopDependencies({
        workspace: fakeSource(),
        apps: { sandbox: undefined, platform: "web" },
      }).widgets.plugin(fixtureApp),
    ).toBeUndefined()
  })

  it("register a real server's app as the gateway's source reads it, only given apps (#384)", async () => {
    const apps = { sandbox: undefined, platform: "web" } as const
    const drawn = gatewayWithApp()
    const { workspace, widgets } = createDesktopDependencies({
      gateway: drawn.connect,
      apps,
    })
    expect(widgets.plugin(appPluginId("mcptest"))).toBeUndefined()
    await workspace.transcript(conversation)
    expect(widgets.plugin(appPluginId("mcptest"))).toMatchObject({
      kind: "app",
      server: "mcptest",
    })

    // Without apps, the same view registers none.
    const undrawn = gatewayWithApp()
    const plain = createDesktopDependencies({ gateway: undrawn.connect })
    await plain.workspace.transcript(conversation)
    expect(plain.widgets.plugin(appPluginId("mcptest"))).toBeUndefined()

    // Beside a named source, the gateway is not asked at all.
    const ignored = gatewayWithApp()
    const named = createDesktopDependencies({
      workspace: fakeSource(),
      gateway: ignored.connect,
      apps,
    })
    await named.workspace.index()
    expect(ignored.connects()).toBe(0)
    expect(named.widgets.plugin(appPluginId("mcptest"))).toBeUndefined()
  })

  it("give a real server's app mount ids of the protocol's form, whatever ids the workspace is given", async () => {
    const drawn = gatewayWithApp()
    const { workspace, widgets } = createDesktopDependencies({
      newId: () => "id-1",
      gateway: drawn.connect,
      apps: { sandbox: undefined, platform: "web" },
    })
    await workspace.transcript(conversation)
    const plugin = widgets.plugin(appPluginId("mcptest"))
    expect(plugin?.kind).toBe("app")
    if (plugin?.kind !== "app") return
    expect(plugin.ports.newId()).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/,
    )
  })

  it("make a real server's app's calls on the client the source holds", async () => {
    const drawn = gatewayWithApp()
    const { workspace, widgets } = createDesktopDependencies({
      gateway: drawn.connect,
      apps: { sandbox: undefined, platform: "web" },
    })
    await workspace.transcript(conversation)
    const plugin = widgets.plugin(appPluginId("mcptest"))
    if (plugin?.kind !== "app") throw new Error("no app plugin")
    const app = { executionId: "run", toolId: "call-1", instanceId: "mount" }
    await plugin.ports.server.release({ sessionId: conversation, server: "mcptest", app })
    expect(drawn.released).toEqual([
      [
        conversation,
        app,
        {
          requestId: expect.stringMatching(
            /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/,
          ),
        },
      ],
    ])
    // One connection: the app was asked on the client its conversation was read on.
    expect(drawn.connects()).toBe(1)
  })

  it("make an app's tool call through the source, which reads its conversation until the call is answered (#436)", async () => {
    vi.useFakeTimers()
    try {
      const drawn = gatewayWithApp()
      const { workspace, widgets } = createDesktopDependencies({
        gateway: drawn.connect,
        apps: { sandbox: undefined, platform: "web" },
      })
      await workspace.transcript(conversation)
      const stop = workspace.subscribe(() => {})
      const plugin = widgets.plugin(appPluginId("mcptest"))
      if (plugin?.kind !== "app") throw new Error("no app plugin")
      const app = { executionId: "run", toolId: "call-1", instanceId: "mount" }
      // At rest, the conversation is not read again.
      await vi.advanceTimersByTimeAsync(3_000)
      expect(drawn.reads()).toBe(1)
      const call = plugin.ports.server.callTool(
        { sessionId: conversation, server: "mcptest", app },
        "app_delete_row",
        {},
      )
      await vi.advanceTimersByTimeAsync(3_000)
      const whileAsked = drawn.reads()
      expect(whileAsked).toBeGreaterThan(1)
      drawn.called.resolve({ resultJson: '{"content":[]}' })
      await call
      await vi.advanceTimersByTimeAsync(3_000)
      expect(drawn.reads()).toBe(whileAsked)
      stop()
    } finally {
      vi.useRealTimers()
    }
  })

  it("leave the fixture app out beside a gateway, whose servers' apps it could stand in for", () => {
    const { widgets } = createDesktopDependencies({
      gateway: gatewayWithApp().connect,
      apps: { sandbox: undefined, platform: "web" },
    })
    expect(widgets.plugin(appPluginId(fixtureServer))).toBeUndefined()
  })
})

const conversation = "0b9a3c1e-5d2f-4a7b-8c6d-1e2f3a4b5c6d"

/**
 * A gateway holding one conversation whose view names an MCP App's call, its
 * client's `mcpApps` recording each release.
 */
function gatewayWithApp() {
  const gateway = fakeGateway()
  gateway.views.set(
    conversation,
    view(conversation, {
      tools: [
        {
          executionId: "run",
          toolId: "call-1",
          title: "show_chart",
          kind: "other",
          status: "running",
          details: "",
          input: "",
          mcp: {
            server: "mcptest",
            tool: "show_chart",
            resourceUri: "ui://t/chart.html",
          },
        },
      ],
    }),
  )
  const released: unknown[][] = []
  // Each tool call waits until the test answers it.
  const called = deferred<unknown>()
  const mcpApps = {
    releaseApp: (...args: unknown[]) => {
      released.push(args)
      return Promise.resolve()
    },
    callTool: () => called.promise,
  } as unknown as McpAppsApi
  const { client } = gateway
  let connects = 0
  return {
    released,
    called,
    reads: () => gateway.count("read"),
    connects: () => connects,
    connect: () => {
      connects++
      return Promise.resolve({
        conversation: client.conversation,
        get connectionState() {
          return client.connectionState
        },
        onConnectionStateChange: client.onConnectionStateChange,
        close: client.close,
        mcpApps,
      })
    },
  }
}

describe("the window's workspace", () => {
  it("is the gateway's when composition can connect to one, which it does on first need", async () => {
    const gateway = fakeGateway()
    let connects = 0
    const { workspace, widgets } = createDesktopDependencies({
      gateway: () => {
        connects++
        return Promise.resolve({ ...gateway.client, mcpApps: {} as McpAppsApi })
      },
    })
    // Not the sample: none of its plugins.
    expect(widgets.natives()).toEqual([])
    expect(connects).toBe(0)
    await workspace.index()
    expect(connects).toBe(1)
    expect(gateway.count("list")).toBe(1)
  })

  it("is the source composition names, ahead of a gateway", () => {
    const named = fakeSource()
    expect(
      createDesktopDependencies({
        workspace: named,
        gateway: () => Promise.reject(new Error("never asked")),
      }).workspace,
    ).toBe(named)
  })
})
