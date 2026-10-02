// @vitest-environment jsdom
/**
 * The `ui/*` bridge, in jsdom, through the real frame transport: an app's
 * frame is a jsdom iframe, what the app says is a `MessageEvent` from that
 * frame's window and the sandbox's origin, and what the host says is what
 * reaches the frame's `postMessage`. The messages are the spec's own
 * examples (MCP Apps 2026-01-26). Each test names the row or ordering of the
 * design table on #349 it holds.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { fixtureAppHtml } from "../fixture/fixture-app"
import { fixtureCall, fixtureServerPort } from "../fixture/fixture-plugin"
import { fixtureResourceUri } from "../fixture/fixture-widgets"
import { frameTransport, type FrameTransport } from "../adapters/dom/frame-transport"
import { appLines, type AppViewState } from "../model/app-view"
import { frameOn } from "../model/lifecycle"
import { errorCodes, type JsonObject, type Outgoing } from "../model/json-rpc"
import type { AppCall, CallPhase } from "../model/tool-call"
import type { HostContext, OpenPlace, WidgetPlace } from "../../model/widget-state"
import {
  createAppBridge,
  deadlines,
  pendingLimit,
  teardownId,
  type AppBridge,
} from "./bridge"
import type { McpAppPorts, McpAppServer, ServerAnswer } from "./ports"

const sandbox = {
  url: "http://127.0.0.1:9999/proxy.html",
  origin: "http://127.0.0.1:9999",
}
const context: HostContext = {
  theme: "light",
  locale: "en-US",
  size: { width: 400, height: 300 },
  safeArea: { top: 0, right: 0, bottom: 0, left: 0 },
}

/** A deferred answer, so a test decides when a port settles. */
function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((ok, fail) => {
    resolve = ok
    reject = fail
  })
  return { promise, resolve, reject }
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0))

interface Harness {
  readonly bridge: AppBridge
  readonly frame: HTMLIFrameElement
  /** Everything posted to the frame, with the origin it was addressed to. */
  readonly posted: { message: Outgoing; origin: string }[]
  readonly views: AppViewState[]
  readonly opened: OpenPlace[]
  readonly closed: { count: number }
  readonly timers: { ms: number; run: () => void; cancelled: boolean }[]
  /** The app (or anything else on the page) says something. */
  say(data: unknown, from?: { source?: Window | null; origin?: string }): void
  /** The posts since the last call, messages only. */
  take(): Outgoing[]
  /** Fires the deadline still armed. */
  deadline(): void
  transport: FrameTransport
}

let harnesses: Harness[] = []

function harness(
  options: {
    place?: WidgetPlace
    phase?: CallPhase
    server?: McpAppServer
    ports?: Partial<McpAppPorts>
  } = {},
): Harness {
  const frame = document.createElement("iframe")
  document.body.append(frame)
  const posted: { message: Outgoing; origin: string }[] = []
  const target = frame.contentWindow!
  target.postMessage = ((message: Outgoing, origin: string) =>
    void posted.push({ message, origin })) as typeof target.postMessage
  const timers: Harness["timers"] = []
  const views: AppViewState[] = []
  const opened: OpenPlace[] = []
  const closed = { count: 0 }
  const call: AppCall = {
    ...fixtureCall("session-a"),
    ...(options.phase ? { phase: options.phase } : {}),
  }
  const ports: McpAppPorts = {
    server: options.server ?? fixtureServerPort(),
    calls: { read: () => ({ kind: "missing" }), subscribe: () => () => {} },
    timers: {
      after: (ms, run) => {
        const timer = { ms, run, cancelled: false }
        timers.push(timer)
        return () => void (timer.cancelled = true)
      },
    },
    sandbox,
    hostInfo: { name: "Nessa", version: "test" },
    page: () => ({
      styles: { "--font-sans": "Geist" },
      timeZone: "UTC",
      platform: "desktop",
    }),
    ...options.ports,
  }
  // The transport is made before the bridge posts anything, as the view's frame is.
  const made: { bridge?: AppBridge } = {}
  const transport = frameTransport(frame, sandbox.origin, (message) =>
    made.bridge?.receive(message),
  )
  const bridge = createAppBridge({
    place: options.place ?? "pane",
    server: "weather",
    call,
    context,
    ports,
    post: (message) => transport.post(message),
    host: {
      open: (place) => void opened.push(place),
      close: () => void closed.count++,
    },
    onView: (view) => void views.push(view),
  })
  made.bridge = bridge
  let seen = 0
  const app: Harness = {
    bridge,
    frame,
    posted,
    views,
    opened,
    closed,
    timers,
    transport,
    say(data, from = {}) {
      window.dispatchEvent(
        new MessageEvent("message", {
          data,
          source: from.source === undefined ? frame.contentWindow : from.source,
          origin: from.origin ?? sandbox.origin,
        }),
      )
    },
    take() {
      const taken = posted.slice(seen).map((each) => each.message)
      seen = posted.length
      return taken
    },
    deadline() {
      const armed = timers.filter((timer) => !timer.cancelled)
      expect(armed).toHaveLength(1)
      armed[0]!.cancelled = true
      armed[0]!.run()
    },
  }
  harnesses.push(app)
  return app
}

const initializeRequest = {
  jsonrpc: "2.0",
  id: 1,
  method: "ui/initialize",
  params: {
    appInfo: { name: "My UI", version: "1.0.0" },
    appCapabilities: { availableDisplayModes: ["inline", "fullscreen"] },
    protocolVersion: "2026-01-26",
  },
}

/** Through the handshake to `live`; the posts so far are taken. */
async function live(app: Harness) {
  await flush()
  app.say({ jsonrpc: "2.0", method: "ui/notifications/sandbox-proxy-ready", params: {} })
  app.say(initializeRequest)
  app.say({ jsonrpc: "2.0", method: "ui/notifications/initialized" })
  await flush()
  app.take()
}

beforeEach(() => {
  harnesses = []
})

afterEach(() => {
  for (const each of harnesses) {
    each.bridge.remove()
    each.transport.close()
    each.frame.remove()
  }
  vi.restoreAllMocks()
})

describe("the handshake", () => {
  it("L1, L4: reads the resource over its session, then hands the proxy the document, policy first", async () => {
    const read = vi.fn(fixtureServerPort().readResource)
    const app = harness({ server: { ...fixtureServerPort(), readResource: read } })
    expect(frameOn(app.bridge.view().lifecycle)).toBe(false)
    await flush()
    expect(read).toHaveBeenCalledWith(
      { sessionId: "session-a", server: "weather" },
      fixtureResourceUri,
    )
    expect(app.bridge.view()).toMatchObject({ lifecycle: { kind: "proxy" } })
    expect(app.take()).toEqual([])
    app.say({
      jsonrpc: "2.0",
      method: "ui/notifications/sandbox-proxy-ready",
      params: {},
    })
    const [ready] = app.take()
    expect(ready).toMatchObject({
      jsonrpc: "2.0",
      method: "ui/notifications/sandbox-resource-ready",
    })
    const html = (ready as unknown as { params: { html: string } }).params.html
    expect(
      html.startsWith('<!doctype html><meta http-equiv="Content-Security-Policy"'),
    ).toBe(true)
    expect(html).toContain("connect-src 'none'")
    expect(html.endsWith(fixtureAppHtml)).toBe(true)
    // The proxy is handed the same policy, for its own document.
    const policy = (ready as unknown as { params: { policy: string } }).params.policy
    expect(policy.startsWith("default-src 'none'; ")).toBe(true)
    expect(html).toContain(`content="${policy}"`)
    // And how long the app's frame has, after each of its loads, to answer
    // the proxy's check (L32): as long as the host waits for the app to
    // initialize.
    expect(
      (ready as unknown as { params: { checkWithin: number } }).params.checkWithin,
    ).toBe(deadlines.initialize)
    expect(app.posted.every((each) => each.origin === sandbox.origin)).toBe(true)
  })

  it("L6: answers ui/initialize with the spec's result: version, host, capabilities, context", async () => {
    const app = harness()
    await flush()
    app.say({
      jsonrpc: "2.0",
      method: "ui/notifications/sandbox-proxy-ready",
      params: {},
    })
    app.take()
    app.say(initializeRequest)
    expect(app.take()).toEqual([
      {
        jsonrpc: "2.0",
        id: 1,
        result: {
          protocolVersion: "2026-01-26",
          hostInfo: { name: "Nessa", version: "test" },
          hostCapabilities: {
            serverTools: {},
            serverResources: {},
            logging: {},
            sandbox: { permissions: {}, csp: {} },
          },
          hostContext: {
            toolInfo: { tool: { name: "show_fixture", inputSchema: { type: "object" } } },
            theme: "light",
            styles: { variables: { "--font-sans": "Geist" } },
            displayMode: "fullscreen",
            availableDisplayModes: ["inline", "fullscreen"],
            containerDimensions: { width: 400, height: 300 },
            locale: "en-US",
            timeZone: "UTC",
            platform: "desktop",
            safeAreaInsets: { top: 0, right: 0, bottom: 0, left: 0 },
          },
        },
      },
    ])
    expect(app.bridge.view().lifecycle.kind).toBe("initializing")
  })

  it("declares a capability only when its port is supplied", async () => {
    const answered = vi.fn(async () => "done" as const)
    const app = harness({
      ports: {
        links: { open: answered },
        downloads: { download: answered },
        conversation: { sendMessage: answered, updateModelContext: answered },
      },
    })
    await flush()
    app.say({
      jsonrpc: "2.0",
      method: "ui/notifications/sandbox-proxy-ready",
      params: {},
    })
    app.take()
    app.say(initializeRequest)
    const [answer] = app.take() as unknown as {
      result: { hostCapabilities: JsonObject }
    }[]
    expect(answer!.result.hostCapabilities).toEqual({
      serverTools: {},
      serverResources: {},
      logging: {},
      sandbox: { permissions: {}, csp: {} },
      openLinks: {},
      downloadFile: {},
      message: { text: {} },
      updateModelContext: { text: {}, structuredContent: {} },
    })
  })

  it("L8, O1: after initialized, the call it was made for: input, then result", async () => {
    const app = harness()
    await flush()
    app.say({
      jsonrpc: "2.0",
      method: "ui/notifications/sandbox-proxy-ready",
      params: {},
    })
    app.say(initializeRequest)
    app.take()
    app.say({ jsonrpc: "2.0", method: "ui/notifications/initialized" })
    expect(app.take()).toEqual([
      {
        jsonrpc: "2.0",
        method: "ui/notifications/tool-input",
        params: { arguments: { title: "Fixture" } },
      },
      {
        jsonrpc: "2.0",
        method: "ui/notifications/tool-result",
        params: {
          content: [{ type: "text", text: "Three rows" }],
          structuredContent: { rows: 3 },
        },
      },
    ])
    expect(app.bridge.view().lifecycle.kind).toBe("live")
  })

  it("L7: an initialize it cannot read is invalid params, and it keeps waiting", async () => {
    const app = harness()
    await flush()
    app.say({
      jsonrpc: "2.0",
      method: "ui/notifications/sandbox-proxy-ready",
      params: {},
    })
    app.take()
    app.say({
      jsonrpc: "2.0",
      id: 1,
      method: "ui/initialize",
      params: { protocolVersion: 1 },
    })
    expect(app.take()).toEqual([
      {
        jsonrpc: "2.0",
        id: 1,
        error: { code: errorCodes.invalidParams, message: "Invalid params" },
      },
    ])
    app.say(initializeRequest)
    expect(app.bridge.view().lifecycle.kind).toBe("initializing")
  })

  it("L9: before initialized, nothing is sent to the view, and requests but ping are refused", async () => {
    const app = harness({ phase: { kind: "running", arguments: { a: 1 } } })
    await flush()
    app.say({
      jsonrpc: "2.0",
      method: "ui/notifications/sandbox-proxy-ready",
      params: {},
    })
    app.take()
    for (const step of ["loading", "initializing"] as const) {
      if (step === "initializing") {
        app.say(initializeRequest)
        app.take()
      }
      app.say({ jsonrpc: "2.0", id: 7, method: "tools/call", params: { name: "x" } })
      app.say({ jsonrpc: "2.0", id: 8, method: "ping" })
      app.bridge.setCall({
        ...fixtureCall("session-a"),
        phase: { kind: "running", arguments: { a: 2 } },
      })
      app.bridge.setContext({ ...context, theme: "dark" })
      expect(app.take(), step).toEqual([
        {
          jsonrpc: "2.0",
          id: 7,
          error: { code: errorCodes.notInitialized, message: "Not initialized" },
        },
        { jsonrpc: "2.0", id: 8, result: {} },
      ])
    }
  })

  it("L9: ui/initialize before the document is handed over is not initialized, not 'already'", async () => {
    const app = harness()
    await flush()
    expect(app.bridge.view().lifecycle.kind).toBe("proxy")
    app.say(initializeRequest)
    expect(app.take()).toEqual([
      {
        jsonrpc: "2.0",
        id: 1,
        error: { code: errorCodes.notInitialized, message: "Not initialized" },
      },
    ])
    expect(app.bridge.view().lifecycle.kind).toBe("proxy")
  })

  it("L10: ui/initialize again is refused", async () => {
    const app = harness()
    await live(app)
    app.say({ ...initializeRequest, id: 2 })
    expect(app.take()).toEqual([
      {
        jsonrpc: "2.0",
        id: 2,
        error: { code: errorCodes.invalidRequest, message: "Already initialized" },
      },
    ])
  })

  it("O3: a context change while initializing reaches the app after initialized, changed fields only", async () => {
    const app = harness()
    await flush()
    app.say({
      jsonrpc: "2.0",
      method: "ui/notifications/sandbox-proxy-ready",
      params: {},
    })
    app.say(initializeRequest)
    app.take()
    app.bridge.setContext({ ...context, theme: "dark" })
    expect(app.take()).toEqual([])
    app.say({ jsonrpc: "2.0", method: "ui/notifications/initialized" })
    expect(app.take()[0]).toEqual({
      jsonrpc: "2.0",
      method: "ui/notifications/host-context-changed",
      params: { theme: "dark" },
    })
  })
})

describe("what fails", () => {
  it("L2: a resource it cannot load fails it, with no frame", async () => {
    const app = harness({
      server: {
        ...fixtureServerPort(),
        readResource: async () => ({ kind: "ok", result: {} }),
      },
    })
    await flush()
    expect(app.bridge.view()).toMatchObject({
      lifecycle: { kind: "failed", reason: "load" },
    })
    for (const answer of [
      { kind: "failed" },
      { kind: "refused", reason: "no" },
    ] as ServerAnswer[]) {
      const other = harness({
        server: { ...fixtureServerPort(), readResource: async () => answer },
      })
      await flush()
      expect(other.bridge.view(), answer.kind).toMatchObject({
        lifecycle: { kind: "failed", reason: "load" },
      })
    }
  })

  it("L3: a server gone when it is read fails it as gone", async () => {
    const app = harness({
      server: {
        ...fixtureServerPort(),
        readResource: async () => ({ kind: "server-gone" }),
      },
    })
    await flush()
    expect(app.bridge.view()).toMatchObject({
      lifecycle: { kind: "failed", reason: "server-gone" },
    })
  })

  it("a port that rejects is a failure to load, logged", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {})
    const app = harness({
      server: {
        ...fixtureServerPort(),
        readResource: () => Promise.reject(new Error("socket closed")),
      },
    })
    await flush()
    expect(app.bridge.view()).toMatchObject({
      lifecycle: { kind: "failed", reason: "load" },
    })
    expect(logged).toHaveBeenCalled()
  })

  it("a window with no sandbox reads nothing and loads nothing", async () => {
    const read = vi.fn(fixtureServerPort().readResource)
    const app = harness({
      server: { ...fixtureServerPort(), readResource: read },
      ports: { sandbox: undefined },
    })
    await flush()
    expect(read).not.toHaveBeenCalled()
    expect(app.bridge.view()).toMatchObject({
      lifecycle: { kind: "failed", reason: "load" },
    })
  })

  it("L5, O6: each stage's deadline fails it and takes the frame; one met is cleared", async () => {
    for (const stage of ["proxy", "loading", "initializing"] as const) {
      const app = harness()
      await flush()
      if (stage !== "proxy")
        app.say({
          jsonrpc: "2.0",
          method: "ui/notifications/sandbox-proxy-ready",
          params: {},
        })
      if (stage === "initializing") app.say(initializeRequest)
      expect(
        app.timers.filter((t) => !t.cancelled).map((t) => t.ms),
        stage,
      ).toEqual([stage === "proxy" ? deadlines.proxy : deadlines.initialize])
      app.deadline()
      expect(app.bridge.view(), stage).toMatchObject({
        lifecycle: { kind: "failed", reason: "load" },
      })
      // Nothing more reaches a frame that is gone.
      app.take()
      app.say({ jsonrpc: "2.0", id: 9, method: "ping" })
      expect(app.take(), stage).toEqual([])
    }
    const met = harness()
    await live(met)
    expect(met.timers.every((timer) => timer.cancelled)).toBe(true)
    // A stale deadline that fires anyway changes nothing.
    met.timers[0]!.run()
    expect(met.bridge.view().lifecycle.kind).toBe("live")
  })

  it("L11: the proxy ready again under a running app fails it", async () => {
    const app = harness()
    await live(app)
    app.say({
      jsonrpc: "2.0",
      method: "ui/notifications/sandbox-proxy-ready",
      params: {},
    })
    expect(app.bridge.view()).toMatchObject({
      lifecycle: { kind: "failed", reason: "load" },
    })
    expect(app.take()).toEqual([])
  })
})

describe("the app leaving its frame", () => {
  it("L32: the proxy saying the app left fails the view and takes the frame; nothing reaches it after", async () => {
    const app = harness()
    await live(app)
    app.say({ jsonrpc: "2.0", method: "ui/notifications/sandbox-app-left", params: {} })
    expect(app.bridge.view()).toMatchObject({
      lifecycle: { kind: "failed", reason: "load" },
    })
    app.say({ jsonrpc: "2.0", id: 1, method: "ping" })
    expect(app.take()).toEqual([])
  })
})

describe("what reaches the bridge", () => {
  it("L27: only messages from this frame's window, with the sandbox's origin", async () => {
    const app = harness()
    await flush()
    const other = document.createElement("iframe")
    document.body.append(other)
    const ready = {
      jsonrpc: "2.0",
      method: "ui/notifications/sandbox-proxy-ready",
      params: {},
    }
    app.say(ready, { source: other.contentWindow })
    app.say(ready, { source: window })
    app.say(ready, { source: null })
    app.say(ready, { origin: "http://127.0.0.1:1420" })
    app.say(ready, { origin: "null" })
    expect(app.bridge.view().lifecycle.kind).toBe("proxy")
    expect(app.take()).toEqual([])
    app.say(ready)
    expect(app.bridge.view().lifecycle.kind).toBe("loading")
    other.remove()
  })

  it("a message is read before it is acted on: data that is not JSON is never seen", async () => {
    const app = harness()
    await live(app)
    const getter = vi.fn(() => "tools/call")
    const hostile = { jsonrpc: "2.0", id: 3 }
    Object.defineProperty(hostile, "method", { get: getter, enumerable: true })
    app.say(hostile)
    // Read once into the copy; acted on from the copy alone.
    expect(getter).toHaveBeenCalledTimes(1)
  })

  it("L28: malformed messages are refused under their id, or ignored", async () => {
    const app = harness()
    await live(app)
    app.say({ jsonrpc: "2.0", id: 4, method: "nope/nope" })
    app.say({ jsonrpc: "2.0", id: 5, method: "ping", params: [1] })
    app.say({ jsonrpc: "2.0", method: "ui/notifications/nope" })
    app.say("garbage")
    expect(app.take()).toEqual([
      {
        jsonrpc: "2.0",
        id: 4,
        error: { code: errorCodes.methodNotFound, message: "Method not found" },
      },
      {
        jsonrpc: "2.0",
        id: 5,
        error: { code: errorCodes.invalidRequest, message: "Invalid request" },
      },
    ])
  })
})

describe("a live app's requests", () => {
  it("L14: tools/call goes over the view's own session and server, never the app's say", async () => {
    const calls: unknown[] = []
    const app = harness({
      server: {
        ...fixtureServerPort(),
        callTool: async (address, tool, args) => {
          calls.push([address, tool, args])
          return { kind: "ok", result: { content: [{ type: "text", text: "72" }] } }
        },
      },
    })
    await live(app)
    app.say({
      jsonrpc: "2.0",
      id: 11,
      method: "tools/call",
      params: {
        name: "get_weather",
        arguments: { location: "New York" },
        _meta: { server: "other", sessionId: "session-b" },
      },
    })
    await flush()
    expect(calls).toEqual([
      [
        { sessionId: "session-a", server: "weather" },
        "get_weather",
        { location: "New York" },
      ],
    ])
    expect(app.take()).toEqual([
      { jsonrpc: "2.0", id: 11, result: { content: [{ type: "text", text: "72" }] } },
    ])
  })

  it("L14: a refusal is the gateway's reason; a failure is internal; a server gone also says so", async () => {
    const answers: ServerAnswer[] = [
      { kind: "refused", reason: "fixture_secret is not available to apps" },
      { kind: "failed" },
      { kind: "server-gone" },
    ]
    const app = harness({
      server: { ...fixtureServerPort(), callTool: async () => answers.shift()! },
    })
    await live(app)
    for (const id of [1, 2, 3])
      app.say({ jsonrpc: "2.0", id, method: "tools/call", params: { name: "t" } })
    await flush()
    expect(app.take()).toEqual([
      {
        jsonrpc: "2.0",
        id: 1,
        error: {
          code: errorCodes.refused,
          message: "fixture_secret is not available to apps",
        },
      },
      {
        jsonrpc: "2.0",
        id: 2,
        error: { code: errorCodes.internal, message: "The request failed" },
      },
      {
        jsonrpc: "2.0",
        id: 3,
        error: { code: errorCodes.refused, message: "The app's server has stopped" },
      },
    ])
    expect(app.bridge.view()).toMatchObject({
      lifecycle: { kind: "live" },
      serverGone: true,
    })
  })

  it("L14: resources/read goes the same way", async () => {
    const app = harness()
    await live(app)
    app.say({
      jsonrpc: "2.0",
      id: 2,
      method: "resources/read",
      params: { uri: fixtureResourceUri },
    })
    await flush()
    const [answer] = app.take()
    expect(answer).toMatchObject({
      id: 2,
      result: { contents: [{ uri: fixtureResourceUri }] },
    })
  })

  it("L15: ui/message and ui/update-model-context go to the conversation, or are not offered", async () => {
    const sent: unknown[] = []
    const conversation = {
      sendMessage: async (session: string, content: readonly JsonObject[]) => {
        sent.push(["message", session, content])
        return sent.length > 1 ? ("refused" as const) : ("done" as const)
      },
      updateModelContext: async (session: string, update: JsonObject) => {
        sent.push(["context", session, update])
        return "refused" as const
      },
    }
    const app = harness({ ports: { conversation } })
    await live(app)
    const message = {
      jsonrpc: "2.0",
      method: "ui/message",
      params: { role: "user", content: [{ type: "text", text: "Hi" }] },
    }
    app.say({ ...message, id: 2 })
    await flush()
    app.say({ ...message, id: 3 })
    app.say({
      jsonrpc: "2.0",
      id: 4,
      method: "ui/update-model-context",
      params: {
        content: [{ type: "text", text: "Selected" }],
        structuredContent: { row: 2 },
      },
    })
    await flush()
    expect(sent).toEqual([
      ["message", "session-a", [{ type: "text", text: "Hi" }]],
      ["message", "session-a", [{ type: "text", text: "Hi" }]],
      [
        "context",
        "session-a",
        { content: [{ type: "text", text: "Selected" }], structuredContent: { row: 2 } },
      ],
    ])
    expect(app.take()).toEqual([
      { jsonrpc: "2.0", id: 2, result: {} },
      { jsonrpc: "2.0", id: 3, result: { isError: true } },
      {
        jsonrpc: "2.0",
        id: 4,
        error: { code: errorCodes.refused, message: "Context update denied" },
      },
    ])
    const without = harness()
    await live(without)
    without.say({ ...message, id: 5 })
    without.say({ jsonrpc: "2.0", id: 6, method: "ui/update-model-context", params: {} })
    expect(without.take()).toEqual([
      {
        jsonrpc: "2.0",
        id: 5,
        error: { code: errorCodes.methodNotFound, message: "Not offered by this host" },
      },
      {
        jsonrpc: "2.0",
        id: 6,
        error: { code: errorCodes.methodNotFound, message: "Not offered by this host" },
      },
    ])
  })

  it("L16: ui/open-link opens only http and https, as parsed; ui/download-file goes to its port", async () => {
    const opened: string[] = []
    const app = harness({
      ports: {
        links: { open: async (url) => (opened.push(url), "done") },
        downloads: { download: async () => "refused" },
      },
    })
    await live(app)
    for (const [id, url] of [
      [1, "https://example.com/a b"],
      [2, "javascript:alert(1)"],
      [3, "file:///etc/passwd"],
      [4, "not a url"],
    ] as const)
      app.say({ jsonrpc: "2.0", id, method: "ui/open-link", params: { url } })
    app.say({
      jsonrpc: "2.0",
      id: 5,
      method: "ui/download-file",
      params: {
        contents: [{ type: "resource", resource: { uri: "file:///a.txt", text: "a" } }],
      },
    })
    await flush()
    expect(opened).toEqual(["https://example.com/a%20b"])
    expect(app.take()).toEqual([
      ...[2, 3, 4].map((id) => ({
        jsonrpc: "2.0",
        id,
        error: { code: errorCodes.invalidParams, message: "Invalid URL" },
      })),
      { jsonrpc: "2.0", id: 1, result: {} },
      { jsonrpc: "2.0", id: 5, result: { isError: true } },
    ])
  })

  it("L16: without their ports, links and downloads are not offered", async () => {
    const app = harness()
    await live(app)
    app.say({
      jsonrpc: "2.0",
      id: 1,
      method: "ui/open-link",
      params: { url: "https://a.example" },
    })
    expect(app.take()).toEqual([
      {
        jsonrpc: "2.0",
        id: 1,
        error: { code: errorCodes.methodNotFound, message: "Not offered by this host" },
      },
    ])
  })

  it("L17: inline, fullscreen opens a pane and the view stays inline; pip opens nothing", async () => {
    const app = harness({ place: "inline" })
    await live(app)
    const ask = (id: number, mode: string) =>
      app.say({ jsonrpc: "2.0", id, method: "ui/request-display-mode", params: { mode } })
    ask(3, "fullscreen")
    ask(4, "pip")
    expect(app.opened).toEqual(["pane"])
    expect(app.take()).toEqual([
      { jsonrpc: "2.0", id: 3, result: { mode: "inline" } },
      { jsonrpc: "2.0", id: 4, result: { mode: "inline" } },
    ])
    const pane = harness({ place: "pane" })
    await live(pane)
    pane.say({
      jsonrpc: "2.0",
      id: 1,
      method: "ui/request-display-mode",
      params: { mode: "inline" },
    })
    expect(pane.opened).toEqual([])
    expect(pane.take()).toEqual([
      { jsonrpc: "2.0", id: 1, result: { mode: "fullscreen" } },
    ])
  })

  it("L18: inline, the frame follows the app's height, clamped; a pane ignores it", async () => {
    const app = harness({ place: "inline" })
    await live(app)
    app.say({
      jsonrpc: "2.0",
      method: "ui/notifications/size-changed",
      params: { width: 400, height: 241.2 },
    })
    expect(app.bridge.view().height).toBe(242)
    app.say({
      jsonrpc: "2.0",
      method: "ui/notifications/size-changed",
      params: { height: 100_000 },
    })
    expect(app.bridge.view().height).toBe(600)
    const pane = harness()
    await live(pane)
    pane.say({
      jsonrpc: "2.0",
      method: "ui/notifications/size-changed",
      params: { height: 50 },
    })
    expect(pane.bridge.view().height).toBeUndefined()
  })

  it("L18, L26: an app saying the same thing again draws nothing again", async () => {
    const app = harness({ place: "inline" })
    await live(app)
    const before = app.views.length
    for (let i = 0; i < 50; i++)
      app.say({
        jsonrpc: "2.0",
        method: "ui/notifications/size-changed",
        params: { height: 120 },
      })
    expect(app.views.length).toBe(before + 1)
    for (let i = 0; i < 50; i++)
      app.say({
        jsonrpc: "2.0",
        method: "ui/notifications/sandbox-csp-violation",
        params: { origin: "https://example.com" },
      })
    expect(app.views.length).toBe(before + 2)
  })

  it("L26: a report whose origin is not a web origin names none", async () => {
    const app = harness()
    await live(app)
    app.say({
      jsonrpc: "2.0",
      method: "ui/notifications/sandbox-csp-violation",
      params: { origin: "Your session expired - sign in at https://evil.example" },
    })
    expect(app.bridge.view()).toMatchObject({ anyBlocked: true, blocked: [] })
  })

  it("L19, L20: ping is answered; a log is noted, not answered", async () => {
    const info = vi.spyOn(console, "info").mockImplementation(() => {})
    const app = harness()
    await live(app)
    app.say({ jsonrpc: "2.0", id: 1, method: "ping" })
    app.say({
      jsonrpc: "2.0",
      method: "notifications/message",
      params: { level: "info", data: "x" },
    })
    expect(app.take()).toEqual([{ jsonrpc: "2.0", id: 1, result: {} }])
    expect(info).toHaveBeenCalledTimes(1)
  })

  it("L29, L30: a pending id again is not answered twice; too many pending are refused", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    const answers = Array.from({ length: pendingLimit }, () => deferred<ServerAnswer>())
    let next = 0
    const app = harness({
      server: { ...fixtureServerPort(), callTool: () => answers[next++]!.promise },
    })
    await live(app)
    for (let id = 0; id < pendingLimit; id++)
      app.say({ jsonrpc: "2.0", id, method: "tools/call", params: { name: "t" } })
    app.say({ jsonrpc: "2.0", id: 0, method: "tools/call", params: { name: "t" } })
    app.say({ jsonrpc: "2.0", id: "x", method: "tools/call", params: { name: "t" } })
    expect(app.take()).toEqual([
      {
        jsonrpc: "2.0",
        id: "x",
        error: { code: errorCodes.refused, message: "Too many requests" },
      },
    ])
    expect(next).toBe(pendingLimit)
    expect(warn).toHaveBeenCalledTimes(1)
    answers[0]!.resolve({ kind: "ok", result: { content: [] } })
    await flush()
    expect(app.take()).toEqual([{ jsonrpc: "2.0", id: 0, result: { content: [] } }])
  })

  it("L31: a port that does not answer in time is a timeout, its slot freed, its late answer dropped", async () => {
    const answer = deferred<ServerAnswer>()
    const app = harness({
      server: { ...fixtureServerPort(), callTool: () => answer.promise },
    })
    await live(app)
    app.say({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "t" } })
    expect(app.timers.filter((t) => !t.cancelled).map((t) => t.ms)).toEqual([
      deadlines.request,
    ])
    app.deadline()
    expect(app.take()).toEqual([
      {
        jsonrpc: "2.0",
        id: 1,
        error: { code: errorCodes.internal, message: "The request timed out" },
      },
    ])
    answer.resolve({ kind: "ok", result: {} })
    await flush()
    expect(app.take()).toEqual([])
    // The id is free again: a new request under it is handled.
    app.say({ jsonrpc: "2.0", id: 1, method: "ping" })
    expect(app.take()).toEqual([{ jsonrpc: "2.0", id: 1, result: {} }])
  })

  it("L31: a port that answers in time cancels its deadline", async () => {
    const app = harness()
    await live(app)
    app.say({
      jsonrpc: "2.0",
      id: 1,
      method: "tools/call",
      params: { name: "fixture_refresh" },
    })
    await flush()
    expect(app.timers.every((timer) => timer.cancelled)).toBe(true)
  })
})

describe("the call and the context, live", () => {
  it("L12, O2: partials until the input, then the result, each once", async () => {
    const app = harness({ phase: { kind: "streaming", partial: { a: 1 } } })
    await flush()
    app.say({
      jsonrpc: "2.0",
      method: "ui/notifications/sandbox-proxy-ready",
      params: {},
    })
    app.say(initializeRequest)
    app.take()
    app.say({ jsonrpc: "2.0", method: "ui/notifications/initialized" })
    const base = fixtureCall("session-a")
    app.bridge.setCall({ ...base, phase: { kind: "streaming", partial: { a: 1, b: 2 } } })
    app.bridge.setCall({ ...base, phase: { kind: "running", arguments: { a: 1, b: 2 } } })
    app.bridge.setCall({ ...base, phase: { kind: "streaming", partial: { a: 9 } } })
    app.bridge.setCall({
      ...base,
      phase: { kind: "done", arguments: { a: 1, b: 2 }, result: { content: [] } },
    })
    app.bridge.setCall({ ...base, phase: { kind: "cancelled" } })
    expect(app.take().map((m) => ("method" in m ? [m.method, m.params] : m))).toEqual([
      ["ui/notifications/tool-input-partial", { arguments: { a: 1 } }],
      ["ui/notifications/tool-input-partial", { arguments: { a: 1, b: 2 } }],
      ["ui/notifications/tool-input", { arguments: { a: 1, b: 2 } }],
      ["ui/notifications/tool-result", { content: [] }],
    ])
  })

  it("a call of another session or resource is not this view's, and is not told to it", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    const app = harness({ phase: { kind: "running", arguments: { a: 1 } } })
    await live(app)
    const done = { kind: "done", arguments: { a: 1 }, result: { content: [] } } as const
    app.bridge.setCall({ ...fixtureCall("session-b"), phase: done })
    app.bridge.setCall({
      ...fixtureCall("session-a"),
      resourceUri: "ui://other",
      phase: done,
    })
    expect(app.take()).toEqual([])
    expect(warn).toHaveBeenCalledTimes(2)
    app.bridge.setCall({ ...fixtureCall("session-a"), phase: done })
    expect(app.take()).toEqual([
      {
        jsonrpc: "2.0",
        method: "ui/notifications/tool-result",
        params: { content: [] },
      },
    ])
  })

  it("L13: a context change says only what changed", async () => {
    const app = harness()
    await live(app)
    app.bridge.setContext(context)
    app.bridge.setContext({ ...context, size: { width: 500, height: 300 } })
    expect(app.take()).toEqual([
      {
        jsonrpc: "2.0",
        method: "ui/notifications/host-context-changed",
        params: { containerDimensions: { width: 500, height: 300 } },
      },
    ])
  })

  it("L26: a blocked load puts its origin in a notice, each origin once, a bounded few", async () => {
    const app = harness()
    await flush()
    const report = (origin?: string) =>
      app.say({
        jsonrpc: "2.0",
        method: "ui/notifications/sandbox-csp-violation",
        params: origin ? { origin } : {},
      })
    report("https://early.example")
    expect(app.bridge.view().anyBlocked).toBe(false)
    app.say({
      jsonrpc: "2.0",
      method: "ui/notifications/sandbox-proxy-ready",
      params: {},
    })
    report("https://example.com")
    report("https://example.com")
    report()
    for (let i = 0; i < 20; i++) report(`https://h${i}.example`)
    expect(app.bridge.view().anyBlocked).toBe(true)
    expect(app.bridge.view().blocked).toHaveLength(8)
    expect(app.bridge.view().blocked[0]).toBe("https://example.com")
    expect(
      app.take().filter((m) => !("method" in m && m.method.includes("resource-ready"))),
    ).toEqual([])
    expect(appLines.blocked).toBe("Blocked a connection this app didn't declare")
  })
})

describe("the end", () => {
  it("L21, L23: a pane's app asks to go: teardown is sent, and its answer closes the place", async () => {
    const app = harness()
    await live(app)
    app.say({ jsonrpc: "2.0", method: "ui/notifications/request-teardown" })
    expect(app.take()).toEqual([
      { jsonrpc: "2.0", id: teardownId, method: "ui/resource-teardown", params: {} },
    ])
    expect(app.closed.count).toBe(0)
    // O7: asked again while ending, nothing more.
    app.say({ jsonrpc: "2.0", method: "ui/notifications/request-teardown" })
    expect(app.take()).toEqual([])
    app.say({ jsonrpc: "2.0", id: "another", result: {} })
    expect(app.closed.count).toBe(0)
    app.say({ jsonrpc: "2.0", id: teardownId, result: {} })
    expect(app.closed.count).toBe(1)
    expect(app.bridge.view().lifecycle.kind).toBe("gone")
    // O7: an answer after it is gone changes nothing.
    app.say({ jsonrpc: "2.0", id: teardownId, result: {} })
    expect(app.closed.count).toBe(1)
  })

  it("L23: an app that never answers is closed at the teardown deadline", async () => {
    const app = harness({ place: "window" })
    await live(app)
    app.say({ jsonrpc: "2.0", method: "ui/notifications/request-teardown" })
    expect(app.timers.filter((t) => !t.cancelled).map((t) => t.ms)).toEqual([
      deadlines.teardown,
    ])
    app.deadline()
    expect(app.closed.count).toBe(1)
  })

  it("L22: an inline app asking to go is declined", async () => {
    const app = harness({ place: "inline" })
    await live(app)
    app.say({ jsonrpc: "2.0", method: "ui/notifications/request-teardown" })
    expect(app.take()).toEqual([])
    expect(app.bridge.view().lifecycle.kind).toBe("live")
  })

  it("L24, O4: removed while the resource is read: its answer makes no frame", async () => {
    const read = deferred<ServerAnswer>()
    const app = harness({
      server: { ...fixtureServerPort(), readResource: () => read.promise },
    })
    app.bridge.remove()
    read.resolve(
      await fixtureServerPort().readResource(
        { sessionId: "", server: "" },
        fixtureResourceUri,
      ),
    )
    await flush()
    expect(app.bridge.view()).toMatchObject({ lifecycle: { kind: "gone" } })
  })

  it("L24, L25, O5: removed while a call is pending: its answer is not posted, nor anything after", async () => {
    const answer = deferred<ServerAnswer>()
    const app = harness({
      server: { ...fixtureServerPort(), callTool: () => answer.promise },
    })
    await live(app)
    app.say({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "t" } })
    app.bridge.remove()
    answer.resolve({ kind: "ok", result: {} })
    await flush()
    app.bridge.setCall(fixtureCall("session-a"))
    app.bridge.setContext({ ...context, theme: "dark" })
    app.say({ jsonrpc: "2.0", id: 2, method: "ping" })
    expect(app.take()).toEqual([])
    expect(app.timers.every((timer) => timer.cancelled)).toBe(true)
  })
})
