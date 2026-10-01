import { strict as assert } from "node:assert"
import { test } from "node:test"
import { lines, record } from "./acp-recorder.mjs"
import {
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
