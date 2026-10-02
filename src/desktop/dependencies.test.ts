/**
 * Composition registers the sample widget plugin only beside the sample
 * workspace, whose session its widgets belong to, and the fixture MCP App
 * there only where apps are drawn; a window on another source starts with
 * the plugins it is given, and two under one id stop it. With a gateway,
 * real servers' apps are registered as the views name them.
 */
import type { McpAppsApi } from "@nessa/client"
import { describe, expect, it } from "vitest"
import { createDesktopDependencies } from "./dependencies"
import {
  appPluginId,
  fixtureServer,
  samplePlugin,
  samplePluginId,
  WidgetRegistryError,
} from "./widgets"
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

  it("register a real server's app through the gateway, only with a gateway and where apps are drawn (#384)", () => {
    const gateway = { mcpApps: {} as McpAppsApi }
    const apps = { sandbox: undefined, platform: "web" } as const
    expect(
      createDesktopDependencies({ workspace: fakeSource(), apps }).gatewayApps,
    ).toBeUndefined()
    expect(
      createDesktopDependencies({ workspace: fakeSource(), gateway }).gatewayApps,
    ).toBeUndefined()
    const { widgets, gatewayApps } = createDesktopDependencies({
      workspace: fakeSource(),
      gateway,
      apps,
    })
    gatewayApps?.observe("0b9a3c1e-5d2f-4a7b-8c6d-1e2f3a4b5c6d", [
      {
        executionId: "run",
        toolId: "call-1",
        title: "show_chart",
        kind: "other",
        status: "running",
        details: "",
        input: "",
        mcp: { server: "mcptest", tool: "show_chart", resourceUri: "ui://t/chart.html" },
      },
    ])
    expect(widgets.plugin(appPluginId("mcptest"))).toMatchObject({
      kind: "app",
      server: "mcptest",
    })
  })
})
