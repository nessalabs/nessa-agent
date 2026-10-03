/**
 * `scripted-frames.mjs` against the recordings it replays (the design table
 * on #418): a reported call keeps the recorded call's shape, carries its own
 * id and the server's result where the harness puts it, and the handshake
 * answers what the SDK checks.
 */
import { strict as assert } from "node:assert"
import { describe, it } from "node:test"

import {
  AGENTS,
  callFrames,
  configOptions,
  harnessInfo,
  initialOptions,
  initializeResult,
  recordedCall,
  recording,
  setOption,
} from "./scripted-frames.mjs"
import { TOOLS } from "./server.mjs"

const result = TOOLS.review_rows.call({})
const call = (agent) =>
  callFrames(agent, recording(agent), {
    id: "call-1",
    server: "mcptest",
    tool: "review_rows",
    args: {},
    result,
  })
const shape = (frame) => ({
  sessionUpdate: frame.sessionUpdate,
  status: frame.status,
  fields: Object.keys(frame).sort(),
  meta: Object.keys(frame._meta?.claudeCode ?? {}).sort(),
})

describe("callFrames", () => {
  for (const agent of AGENTS) {
    it(`${agent}: the recorded call's frames, field for field, each with the new id`, () => {
      const recorded = recordedCall(agent, recording(agent))
      const frames = call(agent)
      assert.ok(recorded.length >= 3, "a recording of one frame replays nothing")
      assert.deepEqual(frames.map(shape), recorded.map(shape))
      for (const frame of frames) assert.equal(frame.toolCallId, "call-1")
    })
  }

  it("codex: every frame the recording marked as MCP names the server and tool", () => {
    const frames = call("codex")
    assert.equal(frames[0]._meta.is_mcp_tool_call, true)
    for (const frame of frames.filter((each) => "rawInput" in each))
      assert.deepEqual(frame.rawInput, {
        server: "mcptest",
        tool: "review_rows",
        arguments: {},
      })
    assert.equal(frames[0].title, "mcp.mcptest.review_rows")
  })

  it("codex: the completion carries the server's result as rawOutput.result", () => {
    const last = call("codex").at(-1)
    assert.equal(last.status, "completed")
    assert.deepEqual(last.rawOutput, {
      result: {
        content: result.content,
        structuredContent: result.structuredContent,
        _meta: null,
      },
      error: null,
    })
  })

  it("claude: every frame names mcp__mcptest__review_rows, and the result is its structured JSON", () => {
    const frames = call("claude")
    for (const frame of frames)
      assert.equal(frame._meta.claudeCode.toolName, "mcp__mcptest__review_rows")
    const said = JSON.stringify(result.structuredContent)
    assert.equal(
      frames.find((each) => "toolResponse" in each._meta.claudeCode)._meta.claudeCode
        .toolResponse,
      said,
    )
    const last = frames.at(-1)
    assert.equal(last.status, "completed")
    assert.equal(last.rawOutput, said)
    assert.deepEqual(last.content, [
      { type: "content", content: { type: "text", text: said } },
    ])
  })

  it("claude: a result with no structured content is reported as its text", () => {
    const frames = callFrames("claude", recording("claude"), {
      id: "call-2",
      server: "mcptest",
      tool: "always_fails",
      args: {},
      result: {
        content: [
          { type: "text", text: "a" },
          { type: "text", text: "b" },
        ],
      },
    })
    assert.equal(frames.at(-1).rawOutput, "a\nb")
  })
})

describe("the handshake", () => {
  it("initialize names the pinned harness, at protocol 1", () => {
    assert.deepEqual(initializeResult("codex").agentInfo, {
      name: "@agentclientprotocol/codex-acp",
      version: harnessInfo("codex").version,
    })
    assert.equal(initializeResult("claude").protocolVersion, 1)
    assert.equal(
      initializeResult("claude").agentInfo.name,
      "@agentclientprotocol/claude-agent-acp",
    )
  })

  it("codex starts at CODEX_CONFIG's model and INITIAL_AGENT_MODE", () => {
    const env = {
      CODEX_CONFIG: JSON.stringify({ model: "m" }),
      INITIAL_AGENT_MODE: "agent",
    }
    assert.deepEqual(initialOptions("codex", env, {}), { model: "m", mode: "agent" })
    assert.throws(() => initialOptions("codex", {}, {}), /no model/)
  })

  it("claude starts at session/new's model and permission mode", () => {
    const params = {
      _meta: { claudeCode: { options: { model: "m", permissionMode: "auto" } } },
    }
    assert.deepEqual(initialOptions("claude", {}, params), { model: "m", mode: "auto" })
    assert.throws(() => initialOptions("claude", {}, {}), /no model/)
  })

  it("a set option is answered at its new value; an unknown one is refused", () => {
    const values = setOption({ model: "m", mode: "default" }, "mode", "agent")
    assert.deepEqual(
      configOptions(values).map(({ id, currentValue }) => [id, currentValue]),
      [
        ["model", "m"],
        ["mode", "agent"],
      ],
    )
    assert.equal(
      configOptions(setOption(values, "effort", "high")).at(-1).category,
      "thought_level",
    )
    assert.equal(setOption(values, "colour", "red"), null)
  })
})
