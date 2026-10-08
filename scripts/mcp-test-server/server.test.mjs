import { strict as assert } from "node:assert"
import { spawn } from "node:child_process"
import { once } from "node:events"
import { test } from "node:test"
import { fileURLToPath } from "node:url"
import {
  APP_CALLS,
  APP_MIME_TYPE,
  CHART_URI,
  PROTOCOL_VERSION,
  REVIEW_URI,
  TOOLS,
  answer,
  initializeDelayMs,
} from "./server.mjs"

const call = (name, args) =>
  answer({
    jsonrpc: "2.0",
    id: 1,
    method: "tools/call",
    params: { name, arguments: args },
  }).result

test("initialize names the protocol with structured results, and tools and resources", () => {
  const { result } = answer({ jsonrpc: "2.0", id: 1, method: "initialize", params: {} })
  assert.equal(result.protocolVersion, PROTOCOL_VERSION)
  assert.deepEqual(Object.keys(result.capabilities).sort(), ["resources", "tools"])
})

test("tools/list declares every tool without its implementation, and the app tool's UI", () => {
  const { tools } = answer({ jsonrpc: "2.0", id: 1, method: "tools/list" }).result
  assert.deepEqual(
    tools.map((tool) => tool.name),
    [
      "report_rows",
      "link_resources",
      "rows.get",
      "always_fails",
      "show_chart",
      "review_rows",
      "app_delete_row",
      "model_only_note",
      "model_only_chart",
    ],
  )
  assert.ok(
    tools.every((tool) => !("call" in tool) && tool.inputSchema.type === "object"),
  )
  const named = (name) => tools.find((tool) => tool.name === name)
  assert.equal(named("show_chart")._meta.ui.resourceUri, CHART_URI)
  assert.equal(named("review_rows")._meta.ui.resourceUri, REVIEW_URI)
  // The tools before the app's own declare nothing new.
  assert.deepEqual(
    tools.filter((tool) => tool._meta).map((tool) => tool.name),
    [
      "show_chart",
      "review_rows",
      "app_delete_row",
      "model_only_note",
      "model_only_chart",
    ],
  )
})

test("the review app's tools: one destructive for apps only, two hidden from apps", () => {
  const { tools } = answer({ jsonrpc: "2.0", id: 1, method: "tools/list" }).result
  const named = (name) => tools.find((tool) => tool.name === name)
  const destructive = named(APP_CALLS.destructive)
  assert.deepEqual(destructive._meta.ui, { visibility: ["app"] })
  assert.deepEqual(destructive.annotations, {
    readOnlyHint: false,
    destructiveHint: true,
  })
  // No UI of its own: `visibility` alone hides it from apps.
  assert.deepEqual(named(APP_CALLS.hiddenNoUi)._meta.ui, { visibility: ["model"] })
  assert.deepEqual(named(APP_CALLS.hiddenWithUi)._meta.ui, {
    resourceUri: CHART_URI,
    visibility: ["model"],
  })
  for (const name of [APP_CALLS.hiddenNoUi, APP_CALLS.hiddenWithUi, "review_rows"])
    assert.equal(named(name).annotations.readOnlyHint, true, name)

  assert.equal(call(APP_CALLS.destructive, { id: 2 }).content[0].text, "Deleted row 2.")
  assert.equal(call(APP_CALLS.destructive, { id: 9 }).isError, true)
  assert.equal(call(APP_CALLS.destructive, {}).isError, true)
  assert.equal(call(APP_CALLS.hiddenNoUi, {}).isError, undefined)
  assert.equal(call(APP_CALLS.hiddenWithUi, {}).isError, undefined)
  assert.deepEqual(call("review_rows", {}).structuredContent, { rows: [1, 2] })
})

test("each tool answers with the result shape it exists to exercise", () => {
  const report = call("report_rows", {})
  assert.equal(report.content[0].type, "text")
  assert.equal(report.structuredContent.total, 30)
  assert.equal(report.isError, undefined)

  const links = call("link_resources", {})
  assert.deepEqual(
    links.content.map((block) => block.type),
    ["text", "resource_link", "resource"],
  )
  assert.equal(links.content[2].resource.text, "Embedded note from nessa-test.")

  assert.deepEqual(call("rows.get", { id: 2 }).structuredContent, {
    id: 2,
    name: "beta",
    value: 20,
  })
  assert.equal(call("rows.get", { id: 9 }).isError, true)
  assert.equal(call("always_fails", {}).isError, true)
  assert.ok(call("show_chart").structuredContent.series.length === 2)
})

test("arguments outside a tool's schema are an error result, never echoed", () => {
  for (const [name, args] of [
    ["report_rows", { extra: "<script>" }],
    ["rows.get", {}],
    ["rows.get", { id: "1" }],
    ["rows.get", { id: 1.5 }],
    ["report_rows", []],
  ]) {
    const result = call(name, args)
    assert.equal(result.isError, true, `${name} ${JSON.stringify(args)}`)
    assert.ok(!JSON.stringify(result).includes("<script>"))
  }
})

test("the app tool's UI resource is listed and read with the MCP Apps MIME type", () => {
  const { resources } = answer({ jsonrpc: "2.0", id: 1, method: "resources/list" }).result
  assert.deepEqual(resources, [
    { uri: CHART_URI, name: "chart", mimeType: APP_MIME_TYPE },
    { uri: REVIEW_URI, name: "review", mimeType: APP_MIME_TYPE },
  ])
  for (const uri of [CHART_URI, REVIEW_URI]) {
    const { contents } = answer({
      jsonrpc: "2.0",
      id: 1,
      method: "resources/read",
      params: { uri },
    }).result
    assert.equal(contents[0].uri, uri)
    assert.equal(contents[0].mimeType, APP_MIME_TYPE)
    assert.match(contents[0].text, /^<!doctype html>/)
    assert.deepEqual(contents[0]._meta.ui.csp, {
      connectDomains: [],
      resourceDomains: [],
    })
  }
})

/**
 * The chart app's own script, run against a stub host. The host answers
 * `ui/initialize`, then sends the tool result. Returns what the app drew
 * and the messages it posted.
 */
async function runChartApp() {
  const { text } = answer({
    jsonrpc: "2.0",
    id: 1,
    method: "resources/read",
    params: { uri: CHART_URI },
  }).result.contents[0]
  const script = text.match(/<script>([\s\S]*)<\/script>/)[1]
  const posted = []
  const parent = { postMessage: (message) => posted.push(message) }
  const listeners = []
  const attributes = {}
  const chart = { textContent: "" }
  const document = {
    body: { setAttribute: (name, value) => (attributes[name] = value) },
    documentElement: { scrollWidth: 320, scrollHeight: 80 },
    getElementById: () => chart,
  }
  const window = {
    parent,
    addEventListener: (type, listener) => type === "message" && listeners.push(listener),
  }
  const { runInNewContext } = await import("node:vm")
  runInNewContext(script, {
    window,
    document,
    Promise,
    ResizeObserver: class {
      observe() {}
    },
  })
  const deliver = (data) =>
    listeners.forEach((listener) => listener({ source: parent, data }))
  return { posted, attributes, chart, deliver }
}

test("the chart app handshakes, then draws the tool result it was sent", async () => {
  const { text } = answer({
    jsonrpc: "2.0",
    id: 1,
    method: "resources/read",
    params: { uri: CHART_URI },
  }).result.contents[0]
  for (const said of [
    '"ui/initialize"',
    '"ui/notifications/initialized"',
    '"ui/notifications/size-changed"',
    '"ui/notifications/tool-result"',
  ])
    assert.ok(text.includes(said), said)
  assert.ok(!text.includes("${"))
  // A static page would already say something. This one draws only the result.
  assert.ok(!text.includes("chart for nessa-test"))

  const { posted, attributes, chart, deliver } = await runChartApp()
  const initialize = posted.find((message) => message.method === "ui/initialize")
  assert.equal(initialize.params.protocolVersion, "2026-01-26")
  assert.equal(chart.textContent, "")
  deliver({
    jsonrpc: "2.0",
    id: initialize.id,
    result: { hostContext: { displayMode: "inline" } },
  })
  await new Promise((done) => setImmediate(done))
  assert.equal(attributes["data-chart-state"], "live")
  assert.ok(posted.some((message) => message.method === "ui/notifications/initialized"))
  assert.ok(posted.some((message) => message.method === "ui/notifications/size-changed"))
  assert.equal(chart.textContent, "")
  deliver({
    jsonrpc: "2.0",
    method: "ui/notifications/tool-result",
    params: {
      structuredContent: {
        series: [
          { name: "alpha", value: 10 },
          { name: "beta", value: 20 },
        ],
      },
    },
  })
  assert.equal(chart.textContent, "alpha 10, beta 20")
})

test("the review app speaks the ui/* bridge and calls each of its tools by name", () => {
  const { text } = answer({
    jsonrpc: "2.0",
    id: 1,
    method: "resources/read",
    params: { uri: REVIEW_URI },
  }).result.contents[0]
  for (const said of [
    '"ui/initialize"',
    '"ui/notifications/initialized"',
    '"ui/notifications/tool-result"',
    '"ui/notifications/size-changed"',
    '"ui/request-display-mode"',
    '"tools/call"',
    '"ui/message"',
    '"ui/update-model-context"',
    ...Object.values(APP_CALLS).map((name) => JSON.stringify(name)),
  ])
    assert.ok(text.includes(said), said)
  // The names are written in by the server, never left as placeholders.
  assert.ok(!text.includes("${"))
})

/**
 * The review app's own script, run against a stub host: the frame's parent,
 * whose posts are recorded, and a document with just what the app touches.
 * The host answers the app's `ui/initialize` with `hostContext`, then sends
 * the tool result. Returns the tools the app called and its body's attributes.
 */
async function runReviewApp(hostContext) {
  const { text } = answer({
    jsonrpc: "2.0",
    id: 1,
    method: "resources/read",
    params: { uri: REVIEW_URI },
  }).result.contents[0]
  const script = text.match(/<script>([\s\S]*)<\/script>/)[1]
  const posted = []
  const parent = { postMessage: (message) => posted.push(message) }
  const listeners = []
  const attributes = {}
  const element = () => ({ textContent: "", addEventListener() {} })
  const document = {
    body: { setAttribute: (name, value) => (attributes[name] = value) },
    documentElement: { scrollWidth: 0, scrollHeight: 0 },
    getElementById: element,
    querySelector: element,
  }
  const window = {
    parent,
    addEventListener: (type, listener) => type === "message" && listeners.push(listener),
  }
  const { runInNewContext } = await import("node:vm")
  runInNewContext(script, {
    window,
    document,
    Promise,
    ResizeObserver: class {
      observe() {}
    },
  })
  const deliver = (data) =>
    listeners.forEach((listener) => listener({ source: parent, data }))
  const initialize = posted.find((message) => message.method === "ui/initialize")
  deliver({ jsonrpc: "2.0", id: initialize.id, result: { hostContext } })
  await new Promise((done) => setImmediate(done))
  deliver({
    jsonrpc: "2.0",
    method: "ui/notifications/tool-result",
    params: { structuredContent: { rows: [1, 2] } },
  })
  return {
    calls: posted
      .filter((message) => message.method === "tools/call")
      .map((message) => message.params.name),
    attributes,
  }
}

test("the review app, inline, calls each of its tools once it has its tool result", async () => {
  const want = [APP_CALLS.destructive, APP_CALLS.hiddenNoUi, APP_CALLS.hiddenWithUi]
  const told = await runReviewApp({ displayMode: "inline" })
  assert.deepEqual(told.calls, want)
  assert.equal(told.attributes["data-review-mode"], "inline")
  // A host context with no display mode is the spec's default: inline.
  const untold = await runReviewApp({})
  assert.deepEqual(untold.calls, want)
  assert.equal(untold.attributes["data-review-mode"], "inline")
  assert.equal(untold.attributes["data-review-state"], "live")
  // Nor with no host context at all.
  const none = await runReviewApp(undefined)
  assert.deepEqual(none.calls, want)
  assert.equal(none.attributes["data-review-mode"], "inline")
})

test("the review app, fullscreen, makes no calls of its own", async () => {
  const { calls, attributes } = await runReviewApp({ displayMode: "fullscreen" })
  assert.deepEqual(calls, [])
  assert.equal(attributes["data-review-mode"], "fullscreen")
})

test("unknown tools, resources and methods are JSON-RPC errors; notifications get no answer", () => {
  const error = (message) => answer({ jsonrpc: "2.0", id: 7, ...message }).error.code
  assert.equal(error({ method: "tools/call", params: { name: "nope" } }), -32602)
  assert.equal(error({ method: "tools/call", params: { name: "constructor" } }), -32602)
  assert.equal(error({ method: "resources/read", params: { uri: "ui://other" } }), -32002)
  assert.equal(error({ method: "nope" }), -32601)
  assert.equal(answer({ jsonrpc: "2.0", method: "notifications/initialized" }), null)
  assert.equal(answer({ jsonrpc: "2.0", id: 3, result: {} }), null)
  assert.equal(answer({ id: 1, method: "ping" }).error.code, -32600)
})

test("the stdio loop answers each line, including a parse error, in order", async () => {
  const child = spawn(process.execPath, [
    fileURLToPath(new URL("./server.mjs", import.meta.url)),
  ])
  const lines = []
  child.stdout.setEncoding("utf8")
  let buffered = ""
  child.stdout.on("data", (chunk) => {
    buffered += chunk
    const parts = buffered.split("\n")
    buffered = parts.pop()
    lines.push(...parts)
    if (lines.length === 3) child.stdin.end()
  })
  child.stdin.write('{"jsonrpc":"2.0","id":1,"method":"ping"}\n')
  child.stdin.write("not json\n")
  child.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
  child.stdin.write('{"jsonrpc":"2.0","id":2,"method":"tools/list"}\n')
  await once(child, "close")
  assert.deepEqual(
    lines
      .map((line) => JSON.parse(line))
      .map((reply) => [reply.id, Boolean(reply.error)]),
    [
      [1, false],
      [null, true],
      [2, false],
    ],
  )
  assert.equal(Object.keys(TOOLS).length, 9)
})

test("--initialize-delay-ms holds initialize's answer, and keeps every answer in order", async () => {
  assert.equal(initializeDelayMs([]), 0)
  assert.equal(initializeDelayMs(["--initialize-delay-ms", "250"]), 250)
  assert.throws(() => initializeDelayMs(["--initialize-delay-ms", "soon"]))
  assert.throws(() => initializeDelayMs(["--initialize-delay-ms"]))
  const child = spawn(process.execPath, [
    fileURLToPath(new URL("./server.mjs", import.meta.url)),
    "--initialize-delay-ms",
    "400",
  ])
  const started = Date.now()
  const arrived = []
  child.stdout.setEncoding("utf8")
  let buffered = ""
  child.stdout.on("data", (chunk) => {
    buffered += chunk
    const parts = buffered.split("\n")
    buffered = parts.pop()
    for (const part of parts) arrived.push([JSON.parse(part).id, Date.now() - started])
    if (arrived.length === 2) child.stdin.end()
  })
  child.stdin.write(
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}\n{"jsonrpc":"2.0","id":2,"method":"ping"}\n',
  )
  await once(child, "close")
  assert.deepEqual(
    arrived.map(([id]) => id),
    [1, 2],
  )
  assert.ok(arrived[0][1] >= 400, `initialize answered after ${arrived[0][1]} ms`)
})
