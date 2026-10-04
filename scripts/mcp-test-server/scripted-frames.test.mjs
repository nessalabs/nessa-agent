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
  PLACES,
  RECORDED_TOOL,
  at,
  recordedArguments,
  recordedCall,
  recordedResult,
  recording,
  setOption,
  unreplayable,
  withAt,
} from "./scripted-frames.mjs"
import { TOOLS } from "./server.mjs"

const result = TOOLS.review_rows.call({})
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b)

/**
 * What of a result a recording may not hold outside its places: its text, its
 * structured JSON, and every string, number and key within its
 * `structuredContent`.
 */
function resultMarks(result) {
  const strings = [
    JSON.stringify(result.structuredContent),
    ...result.content.map((b) => b.text),
  ]
  const numbers = []
  const keys = []
  const walk = (value) => {
    if (typeof value === "string") strings.push(value)
    else if (typeof value === "number") numbers.push(value)
    else if (value !== null && typeof value === "object")
      for (const [key, each] of Object.entries(value)) {
        if (!Array.isArray(value)) keys.push(key)
        walk(each)
      }
  }
  walk(result.structuredContent)
  return { strings, numbers, keys }
}
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
      const marks = resultMarks(recordedResult())
      for (const mark of [RECORDED_TOOL, ...marks.strings, ...marks.keys])
        assert.ok(!text.includes(mark), mark)
    })

    it(`${agent}: the recording carries its call at the declared places and nowhere else`, () => {
      const frames = recordedCall(agent, recording(agent))
      const recordedId = frames[0].toolCallId
      const result = recordedResult()
      for (const place of PLACES[agent])
        assert.ok(
          frames.some((frame) => at(frame, place.path) !== undefined),
          `no ${agent} frame has ${place.path.join(".")}`,
        )
      // Everything left once the places are taken away is copied as recorded:
      // none of it may be the recorded call's.
      const rest = frames.map((frame) =>
        PLACES[agent].reduce(
          (each, place) =>
            at(each, place.path) === undefined ? each : withAt(each, place.path, "PLACE"),
          frame,
        ),
      )
      const found = []
      const marks = resultMarks(result)
      const look = (value, where) => {
        if (same(value, result.content) || same(value, result.structuredContent))
          return found.push(`${where}: the recorded result`)
        if (typeof value === "string") {
          for (const mark of [recordedId, RECORDED_TOOL, ...marks.strings])
            if (value.includes(mark)) found.push(`${where}: ${mark}`)
          return
        }
        if (typeof value === "number" && marks.numbers.includes(value))
          return found.push(`${where}: ${value}`)
        if (value !== null && typeof value === "object")
          for (const [key, each] of Object.entries(value)) {
            if (marks.keys.includes(key))
              found.push(`${where}.${key}: a key of the recorded result`)
            look(each, `${where}.${key}`)
          }
      }
      rest.forEach((frame, index) => look(frame, `frame ${index}`))
      assert.deepEqual(found, [])
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
    assert.match(refused("always_fails", TOOLS.always_fails.call({})), /not shaped/)
    assert.match(refused("link_resources", TOOLS.link_resources.call({})), /not shaped/)
    assert.match(refused("rows.get", TOOLS["rows.get"].call({ id: 2 })), /rewrite/)
    assert.match(refused("no_such_tool", result), /not one of the test server's tools/)
    // Anything not shaped as the recorded result, whatever it adds or lacks.
    for (const shaped of [
      { ...result, isError: false },
      { ...result, _meta: {} },
      { ...result, extra: 1 },
      { content: result.content },
      { structuredContent: result.structuredContent },
      null,
    ])
      assert.match(refused("review_rows", shaped), /not shaped/)
    for (const content of [
      [{ type: "image", data: "", mimeType: "image/png" }],
      [{ type: "text", text: 1 }],
      [{ type: "resource", text: "a" }],
      [{ type: "text", text: "a", extra: 1 }],
      [],
      [...result.content, ...result.content],
      "text",
    ])
      assert.match(refused("review_rows", { ...result, content }), /kind of block/)
    for (const structuredContent of ["text", [1], null, {}])
      assert.match(
        refused("review_rows", { ...result, structuredContent }),
        /not a non-empty object/,
      )
    assert.equal(unreplayable("review_rows", result), null)
  })

  for (const agent of AGENTS)
    it(`${agent}: every recorded tool it admits replays as its own recording, value for value`, () => {
      const recorded = recording(agent)
      const admitted = Object.keys(TOOLS).filter((tool) => {
        try {
          recordedCall(agent, recorded, tool)
        } catch {
          return false
        }
        return unreplayable(tool, TOOLS[tool].call({})) === null
      })
      // More than the recorded call itself, or this shows nothing new.
      assert.ok(admitted.length >= 2, `admitted: ${admitted.join(", ")}`)
      for (const tool of admitted) {
        const frames = recordedCall(agent, recorded, tool)
        const replayed = callFrames(agent, recorded, {
          id: frames[0].toolCallId,
          tool,
          result: TOOLS[tool].call({}),
        })
        assert.deepEqual(replayed, frames, tool)
      }
    })

  it("a replayed call is made with the recorded arguments, the same for each harness", () => {
    const [codex, claude] = AGENTS.map((agent) =>
      recordedArguments(agent, recording(agent)),
    )
    assert.deepEqual(codex, {})
    assert.deepEqual(claude, codex)
  })

  for (const agent of AGENTS)
    it(`${agent}: a recording with no place for the result is refused`, () => {
      const recorded = recording(agent)
      const call = recordedCall(agent, recorded)
      const name = Object.keys(recorded.calls).find((key) => recorded.calls[key] === call)
      const frames = call.map(({ rawOutput, content, ...frame }) => {
        const claudeCode = { ...frame._meta?.claudeCode }
        delete claudeCode.toolResponse
        return { ...frame, _meta: { ...frame._meta, claudeCode } }
      })
      assert.throws(
        () =>
          callFrames(
            agent,
            { ...recorded, calls: { ...recorded.calls, [name]: frames } },
            {
              id: "x",
              tool: "review_rows",
              result,
            },
          ),
        /has no place for show_chart's result/,
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

describe("at and withAt", () => {
  it("read only what a frame holds, and copy rather than change", () => {
    const frame = { rawOutput: { result: { content: [1] } }, list: ["a"] }
    assert.deepEqual(at(frame, ["rawOutput", "result", "content"]), [1])
    assert.equal(at(frame, ["list", 0]), "a")
    // An inherited name is not a place the frame has.
    assert.equal(at(frame, ["toString"]), undefined)
    assert.equal(at(frame, ["rawOutput", "constructor"]), undefined)
    const written = withAt(frame, ["list", 0], "b")
    assert.deepEqual(written.list, ["b"])
    assert.deepEqual(frame.list, ["a"])
  })
})

describe("the handshake", () => {
  for (const agent of AGENTS)
    it(`${agent}: the pinned harness is the one the recording came from`, () => {
      const { name, version } = harnessInfo(agent)
      const escaped = `${name} ${version}`.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
      assert.match(
        recording(agent).recorded,
        new RegExp(`${escaped}(?![\\w.])`),
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
    assert.equal(setOption("codex", values, "constructor", "x"), null)
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
