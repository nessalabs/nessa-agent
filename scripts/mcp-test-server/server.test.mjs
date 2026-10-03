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
    ...Object.values(APP_CALLS).map((name) => JSON.stringify(name)),
  ])
    assert.ok(text.includes(said), said)
  // The names are written in by the server, never left as placeholders.
  assert.ok(!text.includes("${"))
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
