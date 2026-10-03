/**
 * What `scripted-agent.mjs` says, as pure functions: the handshake answers a
 * harness gives the gateway, and one MCP tool call reported in the frames a
 * harness was recorded sending. Pure, so `scripted-frames.test.mjs` checks
 * them without a gateway.
 *
 * The frames are the recording's, not this module's: the SDK's parser
 * fixtures hold each harness's live run, and a call is reported as the
 * recorded `show_chart` call was — the same frames, in the same order, each
 * value as recorded — with only the call's id, its tool's name and its result
 * put in where the recording has its own. What this knows of a harness is how
 * it renders a result, to find the recorded one; a recording that renders it
 * otherwise is refused (`callFrames`), and replaying the recorded call
 * reproduces the recording exactly (`scripted-frames.test.mjs`).
 */
import { readFileSync } from "node:fs"
import { join } from "node:path"

import { repoRoot } from "./local-gateway.mjs"
import { TOOLS } from "./server.mjs"

/** The harnesses a scripted agent can stand in for. */
export const AGENTS = ["codex", "claude"]

/** The recorded call every reported call is shaped as. It took no arguments, and so does every call replayed. */
export const RECORDED_TOOL = "show_chart"

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
 * recording came from (`scripted-frames.test.mjs` fails when they part).
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
 * The config options a session of each harness has, by id, with the ACP
 * category each is listed under (`null`: none): `model` and `mode`, and the
 * harness's own effort option.
 */
const OPTIONS = {
  codex: { model: null, mode: null, reasoning_effort: "thought_level" },
  claude: { model: null, mode: null, effort: "thought_level" },
}

/**
 * `agent`'s session's config options as ACP lists them, from `values` (option
 * id → current value). Each option offers only its current value: nothing
 * here chooses among models or modes, it reports what it was told.
 */
export const configOptions = (agent, values) =>
  Object.entries(values).map(([id, value]) => ({
    id,
    name: id,
    type: "select",
    ...(OPTIONS[agent][id] ? { category: OPTIONS[agent][id] } : {}),
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

/** `agent`'s options `values` with `configId` set to `value`; `null` for an option its session does not have. */
export function setOption(agent, values, configId, value) {
  if (!Object.hasOwn(OPTIONS[agent], configId)) return null
  return { ...values, [configId]: value }
}

/**
 * Why a call of `tool` that returned `result` cannot be replayed in the
 * recorded call's frames, or `null` when it can. The recorded call is a
 * successful one, whose result has `structuredContent`, of a tool whose name
 * no harness rewrites. The harnesses report anything else in frames of its
 * own (Claude a failure in three frames, a text result as blocks, a dotted
 * name with `_`), which no replay of this call would match.
 */
export function unreplayable(tool, result) {
  if (!/^[A-Za-z0-9_-]+$/.test(tool)) return `${tool}: a harness may rewrite this name`
  if (result.isError) return `${tool} failed: the recorded call succeeded`
  if (result.structuredContent === undefined)
    return `${tool} has no structuredContent: the recorded call's result has`
  return null
}

/**
 * Where each harness's recording holds a result, as `[recorded, replayed]`
 * pairs: a value in a frame equal to `recorded` is replaced, whole, by
 * `replayed`. Codex sends the MCP result's `content` and `structuredContent`
 * in `rawOutput.result`; Claude the JSON of its `structuredContent` as
 * `toolResponse`, `rawOutput` and the text of `content`.
 */
const resultValues = {
  codex: (recorded, result) => [
    [recorded.content, result.content],
    [recorded.structuredContent, result.structuredContent],
  ],
  claude: (recorded, result) => [
    [
      JSON.stringify(recorded.structuredContent),
      JSON.stringify(result.structuredContent),
    ],
  ],
}

/** The recorded result's text and structured JSON: what a replay must not still hold, whole or within a string. */
const marks = (recorded) => [
  JSON.stringify(recorded.structuredContent),
  ...recorded.content.filter((block) => block.type === "text").map((block) => block.text),
]

const same = (a, b) => JSON.stringify(a) === JSON.stringify(b)

/** Every string within `value`. */
const strings = (value) =>
  typeof value === "string"
    ? [value]
    : value && typeof value === "object"
      ? Object.values(value).flatMap(strings)
      : []

/**
 * The frames reporting one call of `tool`, with id `id`, which returned
 * `result` (an MCP `CallToolResult`): the recorded call's frames, with the
 * recorded call's id and tool name replaced wherever a string holds them, and
 * its result wherever the harness put it (`resultValues`). The recorded
 * result is the test server's own answer for the recorded tool.
 *
 * Throws for a call this cannot replay (`unreplayable`), and for a recording
 * that does not hold the recorded result where the harness is known to put
 * it, or still holds any of it after the replacing (`marks`): the replay
 * would carry the recorded result.
 */
export function callFrames(agent, recorded, { id, tool, result }) {
  const refused = unreplayable(tool, result)
  if (refused) throw new Error(`cannot replay: ${refused}`)
  const frames = recordedCall(agent, recorded)
  const recordedId = frames[0].toolCallId
  const recordedResult = TOOLS[RECORDED_TOOL].call({})
  const pairs = resultValues[agent](recordedResult, result)
  const used = new Set()
  const put = (value) => {
    const pair = pairs.findIndex(([from]) => same(value, from))
    if (pair !== -1) {
      used.add(pair)
      return pairs[pair][1]
    }
    if (typeof value === "string")
      return value.replaceAll(recordedId, () => id).replaceAll(RECORDED_TOOL, () => tool)
    if (Array.isArray(value)) return value.map(put)
    if (value && typeof value === "object")
      return Object.fromEntries(
        Object.entries(value).map(([key, each]) => [key, put(each)]),
      )
    return value
  }
  const replayed = frames.map(put)
  // The recorded result's marks the replayed one does not share.
  const own = marks(result)
  const recordedOnly = marks(recordedResult).filter((mark) => !own.includes(mark))
  const left = strings(replayed).some((text) =>
    recordedOnly.some((mark) => text.includes(mark)),
  )
  if (used.size !== pairs.length || left)
    throw new Error(
      `the ${agent} recording does not hold ${RECORDED_TOOL}'s result where ${agent} puts it`,
    )
  return replayed
}
