/** How an app's widgets are named: one statement, for the plugin and its calls (#349). */
import { describe, expect, it } from "vitest"
import { sameWidget } from "../../model/widget-ref"
import { appPlugin } from "../ui/app-plugin"
import { fixtureAppPlugin } from "../fixture/fixture-plugin"
import { appPluginId, appWidget } from "./app-ref"

describe("an app's widgets", () => {
  it("are its server's app, by the call's two identities", () => {
    expect(appWidget("charts", "run", "call-1")).toEqual({
      plugin: "mcp:charts",
      id: JSON.stringify(["run", "call-1"]),
    })
  })

  it("never make two calls one widget, however their identities are spelt", () => {
    expect(sameWidget(appWidget("s", "a:b", "c"), appWidget("s", "a", "b:c"))).toBe(false)
    expect(sameWidget(appWidget("s", "a:b", "c"), appWidget("s", "a:b", "c"))).toBe(true)
  })

  it("are looked up under the plugin id its server derives, so id and server agree", () => {
    const ports = fixtureAppPlugin({
      sessionId: "a",
      sandbox: undefined,
      timers: { after: () => () => {} },
      page: () => ({ styles: {}, timeZone: "UTC", platform: "web" }),
      mountId: () => crypto.randomUUID(),
    }).ports
    const plugin = appPlugin({ server: "charts", name: "Charts", ports })
    expect(plugin.id).toBe(appPluginId(plugin.server))
    expect(appWidget(plugin.server, "run", "call").plugin).toBe(plugin.id)
  })
})
