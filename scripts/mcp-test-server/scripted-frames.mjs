/**
 * What `scripted-agent.mjs` says, as pure functions: the handshake answers a
 * harness gives the gateway, and one MCP tool call reported in the frames a
 * harness was recorded sending. Pure, so `scripted-frames.test.mjs` checks
 * them without a gateway.
 *
 * The frames' shape is the recording's, not this module's: the SDK's parser
 * fixtures hold each harness's live run, and a call is reported as the
 * recorded `show_chart` call was — the same frames, in the same order, each
 * carrying the same fields — with only the call's identity and its result put
 * in. A new recording changes what this replays without an edit here.
 */
import { readFileSync } from "node:fs"
import { join } from "node:path"

import { repoRoot } from "./local-gateway.mjs"

/** The harnesses a scripted agent can stand in for. */
export const AGENTS = ["codex", "claude"]

/** The recorded call every reported call is shaped as. */
const RECORDED_TOOL = "show_chart"

/** `agent`'s recorded frames, as the SDK's parser fixtures hold them. */
export function recording(agent) {
  const file = join(
    repoRoot,
    `crates/nessa-sdk/tests/infrastructure/${agent}_acp/tools/fixtures/mcp_live_frames.json`,
  )
  return JSON.parse(readFileSync(file, "utf8"))
}

/** The recorded frames of the call this replays, from `recorded` (`recording(agent)`). */
export function recordedCall(agent, recorded) {
  const name = agent === "claude" ? `mcp__mcptest__${RECORDED_TOOL}` : RECORDED_TOOL
  const frames = recorded.calls?.[name]
  if (!Array.isArray(frames) || frames.length === 0)
    throw new Error(`the ${agent} recording has no ${name} call`)
  return frames
}

/**
 * The harness `agent`'s gateway runs, as its package pins it: the name and
 * version the SDK requires of `initialize`'s `agentInfo`, and the version the
 * recording came from.
 */
export function harnessInfo(agent) {
  const file = join(repoRoot, `crates/nessa-sdk/harnesses/${agent}-acp/package.json`)
  const dependencies = Object.entries(JSON.parse(readFileSync(file, "utf8")).dependencies)
  if (dependencies.length !== 1)
    throw new Error(`${file} pins ${dependencies.length} packages, not one harness`)
  const [[name, version]] = dependencies
  return { name, version }
}

/** `initialize`'s result: protocol 1, the pinned harness, nothing optional offered. */
export const initializeResult = (agent) => ({
  protocolVersion: 1,
  agentInfo: harnessInfo(agent),
  agentCapabilities: { loadSession: false, promptCapabilities: {} },
  authMethods: [],
})

/**
 * The config options a session has, by id, with the ACP category each is
 * listed under (`null`: none). The SDK sets `model` and `mode`, and an effort
 * level under Codex's or Claude's id when one is chosen.
 */
const OPTIONS = {
  model: null,
  mode: null,
  effort: "thought_level",
  reasoning_effort: "thought_level",
}

/**
 * The session's config options as ACP lists them, from `values` (option id →
 * current value). Each option offers only its current value: nothing here
 * chooses among models or modes, it reports what it was told.
 */
export const configOptions = (values) =>
  Object.entries(values).map(([id, value]) => ({
    id,
    name: id,
    type: "select",
    ...(OPTIONS[id] ? { category: OPTIONS[id] } : {}),
    currentValue: value,
    options: [{ value, name: value }],
  }))

/**
 * The options a new session starts with: `model` as the gateway configured
 * it (Codex: `CODEX_CONFIG`'s `model`; Claude: `session/new`'s
 * `_meta.claudeCode.options.model`), and `mode` as Codex's
 * `INITIAL_AGENT_MODE` or Claude's `permissionMode`, else `default`.
 */
export function initialOptions(agent, env, params) {
  if (agent === "codex") {
    const config = JSON.parse(env.CODEX_CONFIG ?? "{}")
    if (!config.model) throw new Error("CODEX_CONFIG names no model")
    return { model: config.model, mode: env.INITIAL_AGENT_MODE ?? "default" }
  }
  const options = params?._meta?.claudeCode?.options ?? {}
  if (!options.model) throw new Error("session/new names no model")
  return { model: options.model, mode: options.permissionMode ?? "default" }
}

/** Options `values` with `configId` set to `value`; `null` for an option the session does not have. */
export function setOption(values, configId, value) {
  if (!Object.hasOwn(OPTIONS, configId)) return null
  return { ...values, [configId]: value }
}

/**
 * The frames reporting one call of `tool` of MCP server `server`, with id
 * `id` and arguments `args`, which returned `result` (an MCP `CallToolResult`):
 * the recorded call's frames, each with this call put in where the
 * recording had its own. Which fields a frame carries is the recording's.
 */
export function callFrames(agent, recorded, { id, server, tool, args, result }) {
  return recordedCall(agent, recorded).map((frame) =>
    agent === "claude"
      ? claudeFrame(frame, { id, name: `mcp__${server}__${tool}`, args, result })
      : codexFrame(frame, { id, server, tool, args, result }),
  )
}

/** Codex: `rawInput` names the server and tool; `rawOutput.result` is the MCP result. */
function codexFrame(frame, { id, server, tool, args, result }) {
  const out = { ...frame, toolCallId: id }
  if ("title" in frame) out.title = `mcp.${server}.${tool}`
  if ("rawInput" in frame) out.rawInput = { server, tool, arguments: args }
  if ("rawOutput" in frame)
    out.rawOutput = {
      result: {
        content: result.content,
        structuredContent: result.structuredContent ?? null,
        _meta: null,
      },
      error: null,
    }
  return out
}

/**
 * Claude: `_meta.claudeCode.toolName` and the title are `mcp__<server>__<tool>`;
 * the result is reported as the recording reports it — the JSON of its
 * `structuredContent` (its text, when it has none) as `toolResponse`,
 * `rawOutput` and the one text block of `content`.
 */
function claudeFrame(frame, { id, name, args, result }) {
  const said =
    result.structuredContent !== undefined
      ? JSON.stringify(result.structuredContent)
      : result.content
          .filter((block) => block.type === "text")
          .map((block) => block.text)
          .join("\n")
  const out = { ...frame, toolCallId: id }
  const claudeCode = { ...frame._meta.claudeCode, toolName: name }
  if ("toolResponse" in claudeCode) claudeCode.toolResponse = said
  out._meta = { ...frame._meta, claudeCode }
  if ("title" in frame) out.title = name
  // The announcement's input is the recording's (`{}`, before the model's
  // arguments stream in); later frames carry the arguments.
  if ("rawInput" in frame && frame.sessionUpdate !== "tool_call") out.rawInput = args
  if ("rawOutput" in frame) out.rawOutput = said
  if ("content" in frame && frame.content.length > 0)
    out.content = [{ type: "content", content: { type: "text", text: said } }]
  return out
}
