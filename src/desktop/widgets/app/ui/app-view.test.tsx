// @vitest-environment jsdom
/**
 * An app drawn by the widget hosts (#349, What a host draws — app rows): its
 * card and its body in a pane, through the real registry, answer, bridge and
 * transport; the app's frame is a jsdom iframe the test speaks for.
 */
import { act, StrictMode } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import { WidgetRegistryProvider } from "../../adapters/react/registry-context"
import { createWidgetRegistry } from "../../application/registry"
import type { OpenPlace } from "../../model/widget-state"
import { InlineWidget } from "../../ui/inline-widget"
import type { AppWidgetPlugin, WidgetHost, WidgetPlugin } from "../../ui/plugin"
import { WidgetAnswerOf } from "../../ui/widget-answer"
import { WidgetBody } from "../../ui/widget-body"
import type { McpAppServer } from "../application/ports"
import {
  fixtureAppPlugin,
  fixtureWidget,
  fixtureServerPort,
} from "../fixture/fixture-plugin"
import { appLines } from "../model/app-view"

const sandbox = {
  url: "http://127.0.0.1:9999/proxy.html",
  origin: "http://127.0.0.1:9999",
}
let root: Root
let container: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
})

afterEach(async () => {
  await act(async () => root.unmount())
  container.remove()
})

function app(
  options: { sandbox?: typeof sandbox | undefined; server?: McpAppServer } = {},
): AppWidgetPlugin {
  const plugin = fixtureAppPlugin({
    sessionId: "session-a",
    sandbox: "sandbox" in options ? options.sandbox : sandbox,
    timers: { after: () => () => {} },
    page: () => ({ styles: {}, timeZone: "UTC", platform: "web" }),
  })
  return options.server
    ? { ...plugin, ports: { ...plugin.ports, server: options.server } }
    : plugin
}

function fakeHost(): WidgetHost & { opened: OpenPlace[]; closed: number } {
  const fake = {
    opened: [] as OpenPlace[],
    closed: 0,
    open: (place: OpenPlace) => void fake.opened.push(place),
    close: () => void fake.closed++,
    openWidget: () => {},
    onEscape: () => () => {},
  }
  return fake
}

const widget = fixtureWidget

async function draw(
  plugin: AppWidgetPlugin,
  place: "inline" | "pane",
  host = fakeHost(),
) {
  const registry = createWidgetRegistry<WidgetPlugin>([])
  registry.register(plugin)
  await act(async () =>
    root.render(
      <StrictMode>
        <WidgetRegistryProvider registry={registry}>
          {place === "inline" ? (
            <InlineWidget widget={widget} host={host} />
          ) : (
            <WidgetAnswerOf widget={widget}>
              {(answer, found) => (
                <WidgetBody
                  id={widget.id}
                  place="pane"
                  answer={answer}
                  plugin={found}
                  host={host}
                />
              )}
            </WidgetAnswerOf>
          )}
        </WidgetRegistryProvider>
      </StrictMode>,
    ),
  )
  return host
}

const frame = () => container.querySelector<HTMLIFrameElement>("iframe[data-app-frame]")

/** Says `data` as the proxy in the frame on the page now. */
async function say(data: unknown) {
  await act(async () =>
    window.dispatchEvent(
      new MessageEvent("message", {
        data,
        source: frame()?.contentWindow,
        origin: sandbox.origin,
      }),
    ),
  )
}

/** Collects what the host posts to the frame on the page now. */
function listen() {
  const posted: unknown[] = []
  const target = frame()?.contentWindow
  if (!target) throw new Error("no app frame on the page")
  target.postMessage = ((message: unknown) =>
    void posted.push(message)) as typeof target.postMessage
  return posted
}

async function toLive() {
  const posted = listen()
  await say({
    jsonrpc: "2.0",
    method: "ui/notifications/sandbox-proxy-ready",
    params: {},
  })
  await say({
    jsonrpc: "2.0",
    id: 1,
    method: "ui/initialize",
    params: {
      appInfo: { name: "F", version: "1" },
      appCapabilities: {},
      protocolVersion: "2026-01-26",
    },
  })
  await say({ jsonrpc: "2.0", method: "ui/notifications/initialized" })
  return posted
}

describe("an app in a card", () => {
  it("draws its name over the proxy's hidden frame while it loads, then the frame", async () => {
    await draw(app(), "inline")
    const loading = frame()
    expect(loading?.getAttribute("src")).toBe(sandbox.url)
    expect(loading?.getAttribute("sandbox")).toBe("allow-scripts allow-same-origin")
    expect(loading?.getAttribute("referrerpolicy")).toBe("no-referrer")
    expect(loading?.hasAttribute("data-hidden")).toBe(true)
    expect(container.querySelector('[role="status"]')?.textContent).toBe("Fixture")
    await toLive()
    expect(frame()?.hasAttribute("data-hidden")).toBe(false)
    expect(container.querySelector('[role="status"]')).toBeNull()
    expect(
      container.querySelector("[data-app-view]")?.getAttribute("data-app-view"),
    ).toBe("live")
  })

  it("is as tall as the app says, up to the card's limit", async () => {
    await draw(app(), "inline")
    await toLive()
    await say({
      jsonrpc: "2.0",
      method: "ui/notifications/size-changed",
      params: { height: 222 },
    })
    expect(frame()?.style.height).toBe("222px")
  })

  it("asks the host for a pane when the app asks for fullscreen", async () => {
    const host = await draw(app(), "inline")
    const posted = await toLive()
    await say({
      jsonrpc: "2.0",
      id: 4,
      method: "ui/request-display-mode",
      params: { mode: "fullscreen" },
    })
    expect(host.opened).toEqual(["pane"])
    expect(posted.at(-1)).toEqual({ jsonrpc: "2.0", id: 4, result: { mode: "inline" } })
  })

  it("says it cannot be loaded, with no close, in a window with no sandbox", async () => {
    await draw(app({ sandbox: undefined }), "inline")
    expect(frame()).toBeNull()
    expect(container.textContent).toBe(appLines.load)
  })

  it("says its server stopped when the server is gone when read", async () => {
    await draw(
      app({
        server: {
          ...fixtureServerPort(),
          readResource: async () => ({ kind: "server-gone" }),
        },
      }),
      "inline",
    )
    await act(async () => {})
    expect(container.textContent).toBe(appLines.serverGone)
  })

  it("shows a notice above the running app for a blocked connection", async () => {
    await draw(app(), "inline")
    await toLive()
    await say({
      jsonrpc: "2.0",
      method: "ui/notifications/sandbox-csp-violation",
      params: { origin: "https://example.com" },
    })
    expect(container.querySelector(".widget-app-notice")?.textContent).toBe(
      `${appLines.blocked}: https://example.com`,
    )
    expect(frame()).not.toBeNull()
  })
})

describe("an app in a pane", () => {
  it("says it cannot be loaded, with close, which closes its pane", async () => {
    const host = await draw(app({ sandbox: undefined }), "pane")
    expect(container.querySelector('[data-slot="empty-state-title"]')?.textContent).toBe(
      appLines.load,
    )
    await act(async () => container.querySelector("button")?.click())
    expect(host.closed).toBe(1)
  })

  it("closes its pane when the app asks to go and answers its teardown", async () => {
    const host = await draw(app(), "pane")
    const posted = await toLive()
    await say({ jsonrpc: "2.0", method: "ui/notifications/request-teardown" })
    expect(posted.at(-1)).toEqual({
      jsonrpc: "2.0",
      id: "nessa-teardown",
      method: "ui/resource-teardown",
      params: {},
    })
    expect(host.closed).toBe(0)
    await say({ jsonrpc: "2.0", id: "nessa-teardown", result: {} })
    expect(host.closed).toBe(1)
  })

  it("takes the frame, and its listener, with it when its place goes", async () => {
    await draw(app(), "pane")
    const posted = await toLive()
    const gone = frame()?.contentWindow ?? null
    await act(async () => root.render(<></>))
    expect(frame()).toBeNull()
    const before = posted.length
    await act(async () =>
      window.dispatchEvent(
        new MessageEvent("message", {
          data: { jsonrpc: "2.0", id: 9, method: "ping" },
          source: gone,
          origin: sandbox.origin,
        }),
      ),
    )
    expect(posted.length).toBe(before)
  })
})
