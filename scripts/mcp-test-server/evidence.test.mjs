import { strict as assert } from "node:assert"
import { test } from "node:test"
import { lines, record } from "./acp-recorder.mjs"
import {
  allowOnce,
  frameShape,
  parseRecording,
  toolFrames,
  uiMentions,
  viewTools,
} from "./evidence.mjs"

const update = (sessionUpdate, extra = {}) => ({
  jsonrpc: "2.0",
  method: "session/update",
  params: { sessionId: "s", update: { sessionUpdate, toolCallId: "t", ...extra } },
})

test("the recorder logs each frame with its direction, and text that is not JSON as text", () => {
  assert.deepEqual(JSON.parse(record("to-agent", '{"id":1}')), {
    direction: "to-agent",
    frame: { id: 1 },
  })
  assert.deepEqual(JSON.parse(record("from-agent", "not json")), {
    direction: "from-agent",
    text: "not json",
  })
  const seen = []
  let rest = lines("", '{"a":1}\n{"b"', (line) => seen.push(line))
  rest = lines(rest, ":2}\n\n", (line) => seen.push(line))
  assert.deepEqual(seen, ['{"a":1}', '{"b":2}'])
  assert.equal(rest, "")
})

test("only the agent's tool updates are read as tool frames", () => {
  const records = parseRecording(
    [
      record("from-agent", JSON.stringify(update("tool_call", { title: "x" }))),
      record("from-agent", JSON.stringify(update("agent_message_chunk"))),
      record("to-agent", JSON.stringify(update("tool_call"))),
      "broken line\n",
      record(
        "from-agent",
        JSON.stringify(update("tool_call_update", { status: "failed" })),
      ),
    ].join(""),
  )
  assert.deepEqual(
    toolFrames(records).map((frame) => frame.sessionUpdate),
    ["tool_call", "tool_call_update"],
  )
  assert.deepEqual(
    frameShape({
      sessionUpdate: "tool_call_update",
      toolCallId: "t",
      content: [{ type: "content", content: { type: "text" } }, { type: "diff" }],
    }).content,
    ["content:text", "diff"],
  )
})

test("any ui:// resource or resourceUri the agent sends is found, and none is not invented", () => {
  const records = [
    {
      direction: "from-agent",
      frame: update("tool_call", { _meta: { ui: { resourceUri: "x" } } }),
    },
    { direction: "from-agent", frame: { result: { text: "see ui://app/a.html" } } },
    { direction: "to-agent", frame: { params: { uri: "ui://asked/for" } } },
  ]
  assert.deepEqual(uiMentions(records), [
    "[0].params.update._meta.ui.resourceUri",
    "[1].result.text",
  ])
  assert.deepEqual(uiMentions(records.slice(2)), [])
})

test("the view's MCP tools are those naming the server", () => {
  const view = {
    tools: [
      { title: "a", mcp: { server: "mcptest", tool: "a" } },
      { title: "b" },
      { title: "c", mcp: { server: "other", tool: "c" } },
    ],
  }
  assert.deepEqual(
    viewTools(view, "mcptest").map((tool) => tool.title),
    ["a"],
  )
  assert.deepEqual(viewTools(null, "mcptest"), [])
})

test("only a call to the test server's tools is allowed, and only once", () => {
  const options = [
    { id: "allow_always", label: "Always allow" },
    { id: "allow_once", label: "Allow" },
    { id: "reject_once", label: "Don't allow" },
  ]
  const view = {
    tools: [
      { executionId: "e", toolId: "mcp", mcp: { server: "mcptest", tool: "x" } },
      { executionId: "e", toolId: "shell" },
      { executionId: "e", toolId: "other", mcp: { server: "other", tool: "x" } },
    ],
  }
  const ask = (toolId, offered = options) => ({
    executionId: "e",
    toolId,
    toolName: "execute",
    options: offered,
  })
  assert.equal(allowOnce(view, ask("mcp"), "mcptest")?.id, "allow_once")
  assert.equal(
    allowOnce(view, ask("mcp", [{ id: "allow-once", label: "x" }]), "mcptest")?.id,
    "allow-once",
  )
  for (const toolId of ["shell", "other", "unknown"])
    assert.equal(allowOnce(view, ask(toolId), "mcptest"), null, toolId)
  // Never a standing approval, nor an option merely labelled as allowing.
  assert.equal(allowOnce(view, ask("mcp", options.slice(0, 1)), "mcptest"), null)
  assert.equal(
    allowOnce(view, ask("mcp", [{ id: "x", label: "Allow once" }]), "mcptest"),
    null,
  )
})

test("the recorder logs a last line that has no newline, and exits with the agent's status", async () => {
  const { spawnSync } = await import("node:child_process")
  const { mkdtempSync, readFileSync } = await import("node:fs")
  const { tmpdir } = await import("node:os")
  const { join } = await import("node:path")
  const { fileURLToPath } = await import("node:url")
  const log = join(mkdtempSync(join(tmpdir(), "acp-recorder-")), "log.jsonl")
  const recorder = fileURLToPath(new URL("./acp-recorder.mjs", import.meta.url))
  const echo = [process.execPath, "-e", "process.stdin.pipe(process.stdout)"]
  const run = spawnSync(process.execPath, [recorder, log, ...echo], {
    input: '{"a":1}\n{"b":"é"}',
    encoding: "utf8",
  })
  assert.equal(run.status, 0)
  assert.equal(run.stdout, '{"a":1}\n{"b":"é"}')
  assert.deepEqual(
    parseRecording(readFileSync(log, "utf8")).map((each) => [each.direction, each.frame]),
    [
      ["to-agent", { a: 1 }],
      ["to-agent", { b: "é" }],
      ["from-agent", { a: 1 }],
      ["from-agent", { b: "é" }],
    ],
  )
  const failing = spawnSync(
    process.execPath,
    [recorder, log, process.execPath, "-e", "process.exit(3)"],
    { input: "" },
  )
  assert.equal(failing.status, 3)
})
