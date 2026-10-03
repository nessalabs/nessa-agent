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
  RECORDED_TOOL,
  recording,
  setOption,
  unreplayable,
} from "./scripted-frames.mjs"
import { TOOLS } from "./server.mjs"

const result = TOOLS.review_rows.call({})
const call = (agent) =>
  callFrames(agent, recording(agent), { id: "call-1", tool: "review_rows", result })

describe("callFrames", () => {
  for (const agent of AGENTS) {
    it(`${agent}: replaying the recorded call reproduces the recording, value for value`, () => {
      const recorded = recordedCall(agent, recording(agent))
      assert.ok(recorded.length >= 3, "a recording of one frame replays nothing")
      const replayed = callFrames(agent, recording(agent), {
        id: recorded[0].toolCallId,
        tool: RECORDED_TOOL,
        result: TOOLS[RECORDED_TOOL].call({}),
      })
      assert.deepEqual(replayed, recorded)
    })

    it(`${agent}: another call keeps every recorded value but its id, its tool and its result`, () => {
      const frames = call(agent)
      const text = JSON.stringify(frames)
      for (const frame of frames) assert.equal(frame.toolCallId, "call-1")
      assert.doesNotMatch(text, /show_chart|series|Chart of two rows/)
    })

    it(`${agent}: a recording that does not hold the result where ${agent} puts it is refused`, () => {
      const recorded = recording(agent)
      const frames = recordedCall(agent, recorded).map((frame) =>
        "rawOutput" in frame ? { ...frame, rawOutput: "something else" } : frame,
      )
      const call = recordedCall(agent, recorded)
      const name = Object.keys(recorded.calls).find((key) => recorded.calls[key] === call)
      const altered = { ...recorded, calls: { ...recorded.calls, [name]: frames } }
      if (agent === "claude") {
        // Claude also says it in toolResponse and content: take those away too.
        altered.calls[name] = frames.map((frame) => {
          const { content, ...rest } = frame
          const claudeCode = { ...frame._meta.claudeCode }
          delete claudeCode.toolResponse
          return { ...rest, _meta: { ...frame._meta, claudeCode } }
        })
      }
      assert.throws(
        () => callFrames(agent, altered, { id: "x", tool: "review_rows", result }),
        /does not hold show_chart's result/,
      )
    })
  }

  it("codex: the frames name the server and tool, and carry the server's result", () => {
    const frames = call("codex")
    assert.equal(frames[0]._meta.is_mcp_tool_call, true)
    assert.equal(frames[0].title, "mcp.mcptest.review_rows")
    for (const frame of frames.filter((each) => "rawInput" in each))
      assert.deepEqual(frame.rawInput, {
        server: "mcptest",
        tool: "review_rows",
        arguments: {},
      })
    const last = frames.at(-1)
    assert.equal(last.status, "completed")
    assert.deepEqual(last.rawOutput.result.content, result.content)
    assert.deepEqual(last.rawOutput.result.structuredContent, result.structuredContent)
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

  it("a call the recorded one cannot stand for is refused, not invented", () => {
    const refused = (tool, result) =>
      AGENTS.map((agent) => {
        assert.throws(
          () => callFrames(agent, recording(agent), { id: "x", tool, result }),
          /cannot replay/,
        )
        return unreplayable(tool, result)
      })[0]
    // Claude reports these in frames of their own (its recordings of them).
    assert.match(refused("always_fails", TOOLS.always_fails.call({})), /failed/)
    assert.match(
      refused("link_resources", TOOLS.link_resources.call({})),
      /no structuredContent/,
    )
    assert.match(refused("rows.get", TOOLS["rows.get"].call({ id: 2 })), /rewrite/)
    assert.equal(unreplayable("review_rows", result), null)
  })

  it("claude: a recording still holding the recorded result after the replacing is refused", () => {
    // The completion's text wrapped, as Claude wraps a failure: the whole
    // value no longer matches, but the recorded JSON is still within it.
    const recorded = recording("claude")
    const call = recordedCall("claude", recorded)
    const name = Object.keys(recorded.calls).find((key) => recorded.calls[key] === call)
    const frames = call.map((frame, index) =>
      index === call.length - 1
        ? {
            ...frame,
            content: [
              {
                type: "content",
                content: {
                  type: "text",
                  text: "```\n" + frame.content[0].content.text + "\n```",
                },
              },
            ],
          }
        : frame,
    )
    assert.throws(
      () =>
        callFrames(
          "claude",
          { ...recorded, calls: { ...recorded.calls, [name]: frames } },
          {
            id: "x",
            tool: "review_rows",
            result,
          },
        ),
      /does not hold show_chart's result/,
    )
  })

  it("ids and names are put in literally, whatever they hold", () => {
    const frames = callFrames("codex", recording("codex"), {
      id: "exec-$&-$$",
      tool: "review_rows",
      result,
    })
    for (const frame of frames) assert.equal(frame.toolCallId, "exec-$&-$$")
  })
})

describe("the handshake", () => {
  for (const agent of AGENTS)
    it(`${agent}: the pinned harness is the one the recording came from`, () => {
      const { name, version } = harnessInfo(agent)
      assert.ok(
        recording(agent).recorded.includes(`${name} ${version}`),
        `the ${agent} recording is not of ${name} ${version}: record it again`,
      )
    })

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
    const values = setOption("codex", { model: "m", mode: "default" }, "mode", "agent")
    assert.deepEqual(
      configOptions("codex", values).map(({ id, currentValue }) => [id, currentValue]),
      [
        ["model", "m"],
        ["mode", "agent"],
      ],
    )
    assert.equal(setOption("codex", values, "colour", "red"), null)
  })

  it("each harness has its own effort option, and not the other's", () => {
    const values = { model: "m", mode: "default" }
    const codex = setOption("codex", values, "reasoning_effort", "high")
    assert.equal(configOptions("codex", codex).at(-1).category, "thought_level")
    assert.equal(setOption("codex", values, "effort", "high"), null)
    const claude = setOption("claude", values, "effort", "high")
    assert.equal(configOptions("claude", claude).at(-1).category, "thought_level")
    assert.equal(setOption("claude", values, "reasoning_effort", "high"), null)
  })
})
