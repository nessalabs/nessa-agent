/** How an app's widgets are named: one statement, for the plugin and its calls (#349). */
import { describe, expect, it } from "vitest"
import { sameWidget } from "../../model/widget-ref"
import { appPlugin } from "../ui/app-plugin"
import { fixtureAppPlugin } from "../fixture/fixture-plugin"
import { appPluginId, appWidget } from "./app-ref"

describe("an app's widgets", () => {
  it("are its server's app, by the call's session and its two identities", () => {
    expect(appWidget("charts", "k", "run", "call-1")).toEqual({
      plugin: "mcp:charts",
      id: JSON.stringify(["k", "run", "call-1"]),
    })
  })

  it("never make two calls one widget, however their identities are spelt", () => {
    expect(
      sameWidget(appWidget("s", "k", "a:b", "c"), appWidget("s", "k", "a", "b:c")),
    ).toBe(false)
    expect(
      sameWidget(appWidget("s", "k", "a:b", "c"), appWidget("s", "k", "a:b", "c")),
    ).toBe(true)
  })

  it("C10: the same execution and tool ids in two sessions are two widgets", () => {
    expect(
      sameWidget(appWidget("s", "a", "run", "call"), appWidget("s", "b", "run", "call")),
    ).toBe(false)
  })

  it("are looked up under the plugin id its server derives, so id and server agree", () => {
    const ports = fixtureAppPlugin({
      sessionId: "a",
      sandbox: undefined,
      timers: { after: () => () => {} },
      newId: () => crypto.randomUUID(),
      page: () => ({ styles: {}, timeZone: "UTC", platform: "web" }),
    }).ports
    const plugin = appPlugin({ server: "charts", name: "Charts", ports })
    expect(plugin.id).toBe(appPluginId(plugin.server))
    expect(appWidget(plugin.server, "k", "run", "call").plugin).toBe(plugin.id)
  })
})
