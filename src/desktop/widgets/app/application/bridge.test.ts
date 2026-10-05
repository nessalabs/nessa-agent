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
  busyReads,
  pendingLimit,
  teardownId,
  type AppBridge,
} from "./bridge"
import type {
  AppAddress,
  ConversationAnswer,
  McpAppConversation,
  McpAppPorts,
  McpAppServer,
  ServerAnswer,
} from "./ports"

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

// Each harness's mounts are numbered, so a test can tell one from another.
let mounts = 0
const mountId = (n: number) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`

/** The address the fixture's call is reached at from mount `instanceId`. */
const addressOf = (instanceId: string): AppAddress => ({
  sessionId: "session-a",
  server: "weather",
  app: { executionId: "fixture-execution", toolId: "fixture-call", instanceId },
})

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
    newId: () => mountId(++mounts),
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
  mounts = 0
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
      addressOf(mountId(1)),
      fixtureResourceUri,
      expect.any(AbortSignal),
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
    const taken = vi.fn(async () => ({ kind: "ok", result: {} }) as const)
    const app = harness({
      ports: {
        links: { open: answered },
        downloads: { download: answered },
        conversation: {
          sendMessage: taken,
          updateModelContext: taken,
          within: deadlines.request,
        },
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
  it("L14, M1: tools/call goes over the view's own conversation, server and mount, never the app's say (forged identity)", async () => {
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
        _meta: {
          server: "other",
          sessionId: "session-b",
          conversationId: "session-b",
          app: { executionId: "e", toolId: "t", instanceId: mountId(9) },
        },
        app: { executionId: "e", toolId: "t", instanceId: mountId(9) },
        instanceId: mountId(9),
      },
    })
    await flush()
    expect(calls).toEqual([
      [addressOf(mountId(1)), "get_weather", { location: "New York" }],
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

  it("L14, A10: a server's own JSON-RPC error reaches the app as it came, its code of either sign", async () => {
    const answers: ServerAnswer[] = [
      { kind: "failed", error: { code: -32002, message: "Resource not found" } },
      { kind: "failed", error: { code: 42, message: "Positive codes are codes too" } },
    ]
    const app = harness({
      server: { ...fixtureServerPort(), callTool: async () => answers.shift()! },
    })
    await live(app)
    for (const id of [1, 2])
      app.say({ jsonrpc: "2.0", id, method: "tools/call", params: { name: "t" } })
    await flush()
    expect(app.take()).toEqual([
      {
        jsonrpc: "2.0",
        id: 1,
        error: { code: -32002, message: "Resource not found" },
      },
      {
        jsonrpc: "2.0",
        id: 2,
        error: { code: 42, message: "Positive codes are codes too" },
      },
    ])
  })

  it("L14, L31: resources/read reads over the same mount", async () => {
    const read = vi.fn(fixtureServerPort().readResource)
    const app = harness({ server: { ...fixtureServerPort(), readResource: read } })
    await live(app)
    app.say({
      jsonrpc: "2.0",
      id: 2,
      method: "resources/read",
      params: { uri: "ui://elsewhere", app: { instanceId: mountId(9) } },
    })
    await flush()
    expect(read.mock.calls).toEqual([
      [addressOf(mountId(1)), fixtureResourceUri, expect.any(AbortSignal)],
      [addressOf(mountId(1)), "ui://elsewhere", expect.any(AbortSignal)],
    ])
  })
})

describe("how long a tools/call is waited for", () => {
  it("D1: tools/call waits the server's callWithin, so a review the person is deciding is not cut off", async () => {
    const answer = deferred<ServerAnswer>()
    const app = harness({
      server: {
        ...fixtureServerPort(),
        callTool: () => answer.promise,
        callWithin: 370_000,
      },
    })
    await live(app)
    app.say({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "t" } })
    app.say({
      jsonrpc: "2.0",
      id: 2,
      method: "resources/read",
      params: { uri: "ui://x" },
    })
    expect(app.timers.filter((t) => !t.cancelled).map((t) => t.ms)).toEqual([
      370_000,
      deadlines.request,
    ])
    answer.resolve({ kind: "ok", result: { content: [] } })
    await flush()
    expect(app.take()).toContainEqual({ jsonrpc: "2.0", id: 1, result: { content: [] } })
  })
})

describe("a gateway too busy to read the app (L1b)", () => {
  it("L1b: the first read answered busy is made again, retryRead apart, and the app loads", async () => {
    const answers: ServerAnswer[] = [{ kind: "busy" }, { kind: "busy" }]
    const read = vi.fn(
      async (address: AppAddress, uri: string, signal: AbortSignal) =>
        answers.shift() ?? fixtureServerPort().readResource(address, uri, signal),
    )
    const app = harness({ server: { ...fixtureServerPort(), readResource: read } })
    await flush()
    expect(app.bridge.view().lifecycle.kind).toBe("reading")
    expect(app.timers.filter((t) => !t.cancelled).map((t) => t.ms)).toEqual([
      deadlines.retryRead,
    ])
    app.deadline()
    await flush()
    expect(read).toHaveBeenCalledTimes(2)
    app.deadline()
    await flush()
    expect(read).toHaveBeenCalledTimes(3)
    expect(app.bridge.view().lifecycle.kind).toBe("proxy")
  })

  it("L1b: busy for every one of busyReads reads, the view fails", async () => {
    const read = vi.fn(async (): Promise<ServerAnswer> => ({ kind: "busy" }))
    const app = harness({ server: { ...fixtureServerPort(), readResource: read } })
    await flush()
    for (let n = 1; n < busyReads; n++) {
      app.deadline()
      await flush()
    }
    expect(read).toHaveBeenCalledTimes(busyReads)
    expect(app.bridge.view().lifecycle).toEqual({ kind: "failed", reason: "load" })
    expect(app.timers.filter((t) => !t.cancelled)).toEqual([])
  })

  it("L1b, L24: removed while waiting to read again, it reads no more", async () => {
    const read = vi.fn(async (): Promise<ServerAnswer> => ({ kind: "busy" }))
    const app = harness({ server: { ...fixtureServerPort(), readResource: read } })
    await flush()
    app.bridge.remove()
    expect(app.timers.every((t) => t.cancelled)).toBe(true)
    expect(read).toHaveBeenCalledTimes(1)
  })

  it("A8: an app's own request answered busy is refused, too many at once", async () => {
    const app = harness({
      server: { ...fixtureServerPort(), callTool: async () => ({ kind: "busy" }) },
    })
    await live(app)
    app.say({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "t" } })
    await flush()
    expect(app.take()).toEqual([
      {
        jsonrpc: "2.0",
        id: 1,
        error: { code: errorCodes.refused, message: "Too many requests at once" },
      },
    ])
  })
})

describe("the mount and its release (#384)", () => {
  function releasing(server: Partial<McpAppServer> = {}) {
    const released: AppAddress[] = []
    return {
      released,
      server: {
        ...fixtureServerPort(),
        release: async (address: AppAddress) => void released.push(address),
        ...server,
      } satisfies McpAppServer,
    }
  }

  it("M2, L24, M3: removing the place releases this mount, once", async () => {
    const { released, server } = releasing()
    const app = harness({ server })
    await live(app)
    expect(released).toEqual([])
    app.bridge.remove()
    app.bridge.remove()
    app.say({ jsonrpc: "2.0", id: 1, method: "ping" })
    await flush()
    expect(released).toEqual([addressOf(mountId(1))])
  })

  it("M2, L23: a teardown the app answered releases it, and removing the place after does not again", async () => {
    const { released, server } = releasing()
    const app = harness({ server })
    await live(app)
    app.say({ jsonrpc: "2.0", method: "ui/notifications/request-teardown" })
    expect(released).toEqual([])
    app.say({ jsonrpc: "2.0", id: teardownId, result: {} })
    app.bridge.remove()
    await flush()
    expect(released).toEqual([addressOf(mountId(1))])
    expect(app.closed.count).toBe(1)
  })

  it("M2, L23: a teardown past its deadline releases it", async () => {
    const { released, server } = releasing()
    const app = harness({ server })
    await live(app)
    app.say({ jsonrpc: "2.0", method: "ui/notifications/request-teardown" })
    app.deadline()
    await flush()
    expect(released).toEqual([addressOf(mountId(1))])
  })

  it("M2, M3: a view whose read failed is released then, once, and not again when removed", async () => {
    const { released, server } = releasing({
      readResource: async () => ({ kind: "refused", reason: "No" }),
    })
    const app = harness({ server })
    await flush()
    expect(app.bridge.view().lifecycle).toEqual({ kind: "failed", reason: "load" })
    expect(released).toEqual([addressOf(mountId(1))])
    app.bridge.remove()
    await flush()
    expect(released).toEqual([addressOf(mountId(1))])
  })

  it("M2: a live view whose app left its frame, with a call waiting, is released at once", async () => {
    const { released, server } = releasing({ callTool: () => new Promise(() => {}) })
    const app = harness({ server })
    await live(app)
    app.say({
      jsonrpc: "2.0",
      id: 1,
      method: "tools/call",
      params: { name: "delete_all" },
    })
    app.say({ jsonrpc: "2.0", method: "ui/notifications/sandbox-app-left", params: {} })
    await flush()
    expect(app.bridge.view().lifecycle).toEqual({ kind: "failed", reason: "load" })
    expect(released).toEqual([addressOf(mountId(1))])
  })

  it("M2: a live view failed by its deadline, or by the proxy loading again, is released", async () => {
    for (const fail of [
      (app: Harness) => app.deadline(),
      (app: Harness) =>
        app.say({
          jsonrpc: "2.0",
          method: "ui/notifications/sandbox-proxy-ready",
          params: {},
        }),
    ]) {
      const { released, server } = releasing()
      const app = harness({ server })
      await flush()
      app.say({
        jsonrpc: "2.0",
        method: "ui/notifications/sandbox-proxy-ready",
        params: {},
      })
      fail(app)
      await flush()
      expect(app.bridge.view().lifecycle.kind).toBe("failed")
      expect(released).toHaveLength(1)
    }
  })

  it("R6: the release aborts the signal every read of this mount was given", async () => {
    const signals: AbortSignal[] = []
    const { server } = releasing({
      readResource: async (_address, uri, signal) => {
        signals.push(signal)
        return fixtureServerPort().readResource(addressOf(mountId(1)), uri, signal)
      },
    })
    const app = harness({ server })
    await live(app)
    app.say({
      jsonrpc: "2.0",
      id: 2,
      method: "resources/read",
      params: { uri: "ui://x" },
    })
    await flush()
    expect(signals).toHaveLength(2)
    expect(signals.every((signal) => !signal.aborted)).toBe(true)
    app.bridge.remove()
    expect(signals.every((signal) => signal.aborted)).toBe(true)
  })

  it("M2: a view that never asked its server anything — no sandbox — has no mount to release", async () => {
    const { released, server } = releasing()
    const app = harness({ server, ports: { sandbox: undefined } })
    await flush()
    expect(app.bridge.view().lifecycle).toEqual({ kind: "failed", reason: "load" })
    app.bridge.remove()
    await flush()
    expect(released).toEqual([])
  })

  it("M5: a release that fails is logged, and the view stays gone", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const { server } = releasing({ release: () => Promise.reject(new Error("closed")) })
    const app = harness({ server })
    await live(app)
    app.bridge.remove()
    await flush()
    expect(error).toHaveBeenCalledWith("An MCP App's release failed", expect.any(Error))
    expect(app.bridge.view().lifecycle.kind).toBe("gone")
  })

  it("M6: two mounts of one call are two instances: each calls, and is released, as itself (the wrong mount)", async () => {
    const { released, server } = releasing()
    const calls: AppAddress[] = []
    const both = {
      ...server,
      callTool: async (address: AppAddress) => {
        calls.push(address)
        return { kind: "ok", result: {} } as const
      },
    }
    const inline = harness({ server: both, place: "inline" })
    const pane = harness({ server: both, place: "pane" })
    await live(inline)
    await live(pane)
    inline.bridge.remove()
    pane.say({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "t" } })
    await flush()
    expect(released).toEqual([addressOf(mountId(1))])
    expect(calls).toEqual([addressOf(mountId(2))])
    expect(pane.take()).toEqual([{ jsonrpc: "2.0", id: 1, result: {} }])
  })

  it("M7, L25: a call the release withdrew answers after the view is gone, and nothing is posted", async () => {
    const answer = deferred<ServerAnswer>()
    const { released, server } = releasing({ callTool: () => answer.promise })
    const app = harness({ server })
    await live(app)
    app.say({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "t" } })
    app.bridge.remove()
    await flush()
    expect(released).toHaveLength(1)
    answer.resolve({ kind: "refused", reason: "The request was withdrawn" })
    await flush()
    expect(app.take()).toEqual([])
  })

  it("L12: a call of another execution or tool call is not this view's", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    const app = harness({ phase: { kind: "running", arguments: {} } })
    await live(app)
    const done: CallPhase = { kind: "done", arguments: {}, result: { content: [] } }
    app.bridge.setCall({ ...fixtureCall("session-a"), executionId: "other", phase: done })
    app.bridge.setCall({ ...fixtureCall("session-a"), toolId: "other", phase: done })
    expect(app.take()).toEqual([])
    expect(warn).toHaveBeenCalledTimes(2)
  })
})

/**
 * An app in its conversation (#390): `ui/message` and
 * `ui/update-model-context` through the conversation port, each answered as
 * the desktop's table on #390 says (rows D1–D16; the adapter's own rows are
 * `app-messages.test.ts`'s).
 */
describe("an app in its conversation (#390)", () => {
  const messageRequest = (
    id: number,
    content: unknown = [{ type: "text", text: "Hi" }],
  ) => ({
    jsonrpc: "2.0",
    id,
    method: "ui/message",
    params: { role: "user", content },
  })
  const contextRequest = (id: number, params: JsonObject = {}) => ({
    jsonrpc: "2.0",
    id,
    method: "ui/update-model-context",
    params,
  })

  /** A conversation port answering each request with the next of `answers`. */
  function speaking(answers: ConversationAnswer[] = [], within = deadlines.request) {
    const asked: { method: string; address: AppAddress; sent: unknown }[] = []
    const next = async () => answers.shift() ?? ({ kind: "ok", result: {} } as const)
    const conversation: McpAppConversation = {
      sendMessage: (address, content) => {
        asked.push({ method: "message", address, sent: content })
        return next()
      },
      updateModelContext: (address, context) => {
        asked.push({ method: "context", address, sent: context })
        return next()
      },
      within,
    }
    return { asked, conversation }
  }

  it("D1: a message of text blocks goes to the conversation at the view's own address, and is answered {}", async () => {
    const { asked, conversation } = speaking()
    const app = harness({ ports: { conversation } })
    await live(app)
    app.say(
      messageRequest(2, [
        { type: "text", text: "Plot May" },
        { type: "text", text: "next to April" },
      ]),
    )
    await flush()
    expect(asked).toEqual([
      {
        method: "message",
        // The app's own call and this mount, never anything the app said.
        address: addressOf(mountId(1)),
        sent: [
          { type: "text", text: "Plot May" },
          { type: "text", text: "next to April" },
        ],
      },
    ])
    expect(app.take()).toEqual([{ jsonrpc: "2.0", id: 2, result: {} }])
  })

  it("D2: a non-text block, no role user, or an empty list is refused invalidParams when read, and nothing is sent", async () => {
    const { asked, conversation } = speaking()
    const app = harness({ ports: { conversation } })
    await live(app)
    app.say(messageRequest(2, [{ type: "image", data: "AA==", mimeType: "image/png" }]))
    app.say(messageRequest(3, []))
    app.say({
      jsonrpc: "2.0",
      id: 4,
      method: "ui/message",
      params: { role: "assistant", content: [{ type: "text", text: "Hi" }] },
    })
    app.say(messageRequest(5, [{ type: "text", text: "Hi" }, { type: "text" }]))
    await flush()
    expect(asked).toEqual([])
    expect(app.take()).toEqual(
      [2, 3, 4, 5].map((id) => ({
        jsonrpc: "2.0",
        id,
        error: { code: errorCodes.invalidParams, message: "Invalid params" },
      })),
    )
  })

  it("D3, D13: what the client's bounds refuse is invalidParams, in the client's words", async () => {
    const { conversation } = speaking([
      { kind: "invalid", reason: "Message must contain 1 character to 8192 UTF-8 bytes" },
      { kind: "invalid", reason: "Context text must contain at most 8192 UTF-8 bytes" },
    ])
    const app = harness({ ports: { conversation } })
    await live(app)
    app.say(messageRequest(2))
    await flush()
    app.say(contextRequest(3, { content: [{ type: "text", text: "x" }] }))
    await flush()
    expect(app.take()).toEqual([
      {
        jsonrpc: "2.0",
        id: 2,
        error: {
          code: errorCodes.invalidParams,
          message: "Message must contain 1 character to 8192 UTF-8 bytes",
        },
      },
      {
        jsonrpc: "2.0",
        id: 3,
        error: {
          code: errorCodes.invalidParams,
          message: "Context text must contain at most 8192 UTF-8 bytes",
        },
      },
    ])
  })

  it("D4, D5, D9: a message the gateway refused — denied, a turn running, busy — is isError, and nothing else is said", async () => {
    const { conversation } = speaking([
      { kind: "refused", reason: "The person declined this action" },
      { kind: "refused", reason: "The conversation is busy" },
      { kind: "busy" },
    ])
    const app = harness({ ports: { conversation } })
    await live(app)
    for (const id of [2, 3, 4]) {
      app.say(messageRequest(id))
      await flush()
    }
    expect(app.take()).toEqual(
      [2, 3, 4].map((id) => ({ jsonrpc: "2.0", id, result: { isError: true } })),
    )
    expect(app.bridge.view().serverGone).toBeFalsy()
  })

  it("D6: a message to a conversation gone is isError, and the view says the server has stopped", async () => {
    const { conversation } = speaking([{ kind: "server-gone" }])
    const app = harness({ ports: { conversation } })
    await live(app)
    app.say(messageRequest(2))
    await flush()
    expect(app.take()).toEqual([{ jsonrpc: "2.0", id: 2, result: { isError: true } }])
    expect(app.bridge.view().serverGone).toBe(true)
  })

  it("D7: a failure, or a port that rejects, is internal: the request failed, and a fault is logged", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const { conversation } = speaking([{ kind: "failed" }, { kind: "failed" }])
    const app = harness({
      ports: {
        conversation: {
          ...conversation,
          updateModelContext: async () => Promise.reject(new Error("adapter fault")),
        },
      },
    })
    await live(app)
    app.say(messageRequest(2))
    app.say(contextRequest(3))
    await flush()
    const failed = (id: number) => ({
      jsonrpc: "2.0",
      id,
      error: { code: errorCodes.internal, message: "The request failed" },
    })
    expect(app.take()).toEqual([failed(2), failed(3)])
    expect(error).toHaveBeenCalledWith("An MCP App port failed", expect.any(Error))
  })

  it("D8: either request waits the conversation's within, then is answered that it timed out, and a late answer is dropped", async () => {
    const answer = deferred<ConversationAnswer>()
    const app = harness({
      ports: {
        conversation: {
          sendMessage: () => answer.promise,
          updateModelContext: () => answer.promise,
          within: 370_000,
        },
      },
    })
    await live(app)
    app.say(messageRequest(2))
    app.say(contextRequest(3))
    const armed = app.timers.filter((timer) => !timer.cancelled)
    expect(armed.map((timer) => timer.ms)).toEqual([370_000, 370_000])
    for (const timer of armed) timer.run()
    answer.resolve({ kind: "ok", result: {} })
    await flush()
    const timedOut = (id: number) => ({
      jsonrpc: "2.0",
      id,
      error: { code: errorCodes.internal, message: "The request timed out" },
    })
    expect(app.take()).toEqual([timedOut(2), timedOut(3)])
  })

  it("D10, D-G: a mount whose message waits in review is released when its view ends, once, so the gateway withdraws the review", async () => {
    const released: AppAddress[] = []
    const review = deferred<ConversationAnswer>()
    const app = harness({
      server: {
        ...fixtureServerPort(),
        release: async (address) => void released.push(address),
      },
      ports: {
        conversation: {
          sendMessage: () => review.promise,
          updateModelContext: async () => ({ kind: "ok", result: {} }),
          within: deadlines.request,
        },
      },
    })
    await live(app)
    app.say(messageRequest(2))
    app.say(contextRequest(3, { content: [{ type: "text", text: "Showing April" }] }))
    await flush()
    expect(released).toEqual([])
    app.bridge.remove()
    app.bridge.remove()
    await flush()
    expect(released).toEqual([addressOf(mountId(1))])
    // Withdrawn, its answer reaches no app.
    review.resolve({ kind: "refused", reason: "The request was withdrawn" })
    await flush()
    expect(app.take().filter((message) => "id" in message && message.id === 2)).toEqual(
      [],
    )
  })

  it("D11, D12: a context goes to the conversation as the app gave it — or with neither part — and is answered {}", async () => {
    const { asked, conversation } = speaking()
    const app = harness({ ports: { conversation } })
    await live(app)
    app.say(
      contextRequest(2, {
        content: [{ type: "text", text: "Selected" }],
        structuredContent: { row: 2 },
      }),
    )
    app.say(contextRequest(3))
    app.say(contextRequest(4, { content: [] }))
    await flush()
    expect(asked.map(({ method, address, sent }) => [method, address, sent])).toEqual([
      [
        "context",
        addressOf(mountId(1)),
        { content: [{ type: "text", text: "Selected" }], structuredContent: { row: 2 } },
      ],
      ["context", addressOf(mountId(1)), {}],
      ["context", addressOf(mountId(1)), { content: [] }],
    ])
    expect(app.take()).toEqual(
      [2, 3, 4].map((id) => ({ jsonrpc: "2.0", id, result: {} })),
    )
  })

  it("D14: a context the gateway refused is an error in the words a tools/call is told; a conversation gone shows the notice", async () => {
    const { conversation } = speaking([
      { kind: "refused", reason: "The request is larger than the gateway accepts" },
      { kind: "busy" },
      { kind: "server-gone" },
    ])
    const app = harness({ ports: { conversation } })
    await live(app)
    for (const id of [2, 3, 4]) {
      app.say(contextRequest(id, { content: [{ type: "text", text: "x" }] }))
      await flush()
    }
    expect(app.take()).toEqual([
      {
        jsonrpc: "2.0",
        id: 2,
        error: {
          code: errorCodes.refused,
          message: "The request is larger than the gateway accepts",
        },
      },
      {
        jsonrpc: "2.0",
        id: 3,
        error: { code: errorCodes.refused, message: "Too many requests at once" },
      },
      {
        jsonrpc: "2.0",
        id: 4,
        error: { code: errorCodes.refused, message: "The app's server has stopped" },
      },
    ])
    expect(app.bridge.view().serverGone).toBe(true)
  })

  it("D16: with no conversation port, both are not offered, and the capabilities say neither", async () => {
    const without = harness()
    await flush()
    without.say({
      jsonrpc: "2.0",
      method: "ui/notifications/sandbox-proxy-ready",
      params: {},
    })
    without.take()
    without.say(initializeRequest)
    const [answer] = without.take() as unknown as {
      result: { hostCapabilities: JsonObject }
    }[]
    expect(answer!.result.hostCapabilities).not.toHaveProperty("message")
    expect(answer!.result.hostCapabilities).not.toHaveProperty("updateModelContext")
    without.say({ jsonrpc: "2.0", method: "ui/notifications/initialized" })
    await flush()
    without.take()
    without.say(messageRequest(5))
    without.say(contextRequest(6))
    expect(without.take()).toEqual(
      [5, 6].map((id) => ({
        jsonrpc: "2.0",
        id,
        error: { code: errorCodes.methodNotFound, message: "Not offered by this host" },
      })),
    )
  })
})

describe("a live app's other requests", () => {
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
        addressOf(mountId(1)),
        fixtureResourceUri,
        new AbortController().signal,
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
