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
 * written at the places that harness carries them (`PLACES`). What this
 * knows of a harness is those places; the test checks them against the
 * recordings, so a recording that carries the call anywhere else fails it,
 * and replaying the recorded call reproduces the recording exactly. One
 * thing it knows comes from elsewhere: where Claude's harness names a call in
 * its `tools/call` (`CLAUDE_CALL_ID`), which the test holds to the SDK's
 * `CALL_ID`, not to the recordings.
 */
import { readFileSync } from "node:fs"
import { join } from "node:path"

import { SERVER, repoRoot } from "./local-gateway.mjs"
import { TOOLS } from "./server.mjs"

/** The harnesses a scripted agent can stand in for. */
export const AGENTS = ["codex", "claude"]

/**
 * Where Claude's harness names a forwarded call in its `tools/call` params'
 * `_meta`: the call's ACP `toolCallId`. The SDK's `CALL_ID` (`stand_in.rs`) is
 * the copy that matters, as the gateway's stand-in reads it; the test holds
 * this one to it, naming both values when they differ.
 */
export const CLAUDE_CALL_ID = "claudecode/toolUseId"

/** The recorded call every reported call is shaped as. Every call replayed takes its arguments (`recordedArguments`). */
export const RECORDED_TOOL = "show_chart"

/** `agent`'s recorded frames, as the SDK's parser fixtures hold them. */
export function recording(agent) {
  const file = join(
    repoRoot,
    `crates/nessa-sdk/tests/infrastructure/${agent}_acp/tools/fixtures/mcp_live_frames.json`,
  )
  return JSON.parse(readFileSync(file, "utf8"))
}

/** The recorded frames of a call of `tool` (by default the one this replays), from `recorded` (`recording(agent)`). */
export function recordedCall(agent, recorded, tool = RECORDED_TOOL) {
  const name = agent === "claude" ? `mcp__${SERVER}__${tool}` : tool
  const frames = recorded.calls?.[name]
  if (!Array.isArray(frames) || frames.length === 0)
    throw new Error(`the ${agent} recording has no ${name} call`)
  return frames
}

/**
 * The arguments the recorded call was made with, as its frames carry them:
 * Codex's `rawInput.arguments`, Claude's last `rawInput` (its announcement's
 * comes before the model's arguments). A replayed call is made with these,
 * since the frames copy them from the recording.
 */
export function recordedArguments(agent, recorded) {
  const inputs = recordedCall(agent, recorded)
    .filter((frame) => frame.rawInput !== undefined)
    .map((frame) => (agent === "codex" ? frame.rawInput.arguments : frame.rawInput))
  if (inputs.length === 0) throw new Error(`the ${agent} recording has no arguments`)
  return inputs.at(-1)
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

/** The recorded call's result: the test server's own answer for the recorded tool. */
export const recordedResult = () => TOOLS[RECORDED_TOOL].call({})

const plainObject = (value) =>
  value !== null &&
  typeof value === "object" &&
  Object.getPrototypeOf(value) === Object.prototype

const same = (a, b) => JSON.stringify(a) === JSON.stringify(b)
const sameKeys = (a, b) => same(Object.keys(a).sort(), Object.keys(b).sort())

/**
 * Why a call of `tool` that returned `result` cannot be replayed in the
 * recorded call's frames, or `null` when it can: it can when the tool is one
 * of the test server's own, under a name no harness rewrites, and its result
 * is shaped as the recorded call's is — the same keys, as many content blocks,
 * each of the recorded block's keys and type with text, and
 * `structuredContent` a non-empty object. Anything else (a failure, a text-only result, `_meta`, an image, a
 * dotted name) a harness reports in frames of its own, which no replay of
 * the recorded call would match.
 */
export function unreplayable(tool, result) {
  if (!Object.hasOwn(TOOLS, tool)) return `${tool} is not one of the test server's tools`
  if (!/^[A-Za-z0-9_-]+$/.test(tool)) return `${tool}: a harness may rewrite this name`
  const recorded = recordedResult()
  if (!plainObject(result) || !sameKeys(result, recorded))
    return `${tool}'s result is not shaped as the recorded call's (${Object.keys(recorded).join(", ")})`
  const [block] = recorded.content
  if (
    !Array.isArray(result.content) ||
    result.content.length !== recorded.content.length ||
    !result.content.every(
      (each) =>
        plainObject(each) &&
        sameKeys(each, block) &&
        each.type === block.type &&
        typeof each.text === "string",
    )
  )
    return `${tool}'s content is not the recorded call's kind of block`
  if (
    !plainObject(result.structuredContent) ||
    Object.keys(result.structuredContent).length === 0
  )
    return `${tool}'s structuredContent is not a non-empty object, as the recorded call's is`
  return null
}

/** The recorded name, with the recorded tool's put in its place. */
const renamed = (recorded, { tool }) => recorded.replaceAll(RECORDED_TOOL, () => tool)
/** Claude's rendering of a result: the JSON of its `structuredContent`. */
const claudeSays = (_, { result }) => JSON.stringify(result.structuredContent)

/**
 * Where each harness's frames carry the call, as paths into a frame: its id,
 * its tool's name, and (`result: true`) its result. A replay writes the new
 * call at these places, in the frames that have them, and copies everything
 * else from the recording; `scripted-frames.test.mjs` checks the recordings
 * carry the recorded call nowhere else.
 */
export const PLACES = {
  codex: [
    { path: ["toolCallId"], put: (_, { id }) => id },
    { path: ["title"], put: renamed },
    { path: ["rawInput", "tool"], put: renamed },
    {
      path: ["rawOutput", "result", "content"],
      put: (_, { result }) => result.content,
      result: true,
    },
    {
      path: ["rawOutput", "result", "structuredContent"],
      put: (_, { result }) => result.structuredContent,
      result: true,
    },
  ],
  claude: [
    { path: ["toolCallId"], put: (_, { id }) => id },
    { path: ["title"], put: renamed },
    { path: ["_meta", "claudeCode", "toolName"], put: renamed },
    { path: ["_meta", "claudeCode", "toolResponse"], put: claudeSays, result: true },
    { path: ["rawOutput"], put: claudeSays, result: true },
    { path: ["content", 0, "content", "text"], put: claudeSays, result: true },
  ],
}

const holds = (value, key) =>
  value !== null && typeof value === "object" && Object.hasOwn(value, key)

/** The value at `path` in `value`, or `undefined` where it has none. */
export const at = (value, path) =>
  path.reduce((each, key) => (holds(each, key) ? each[key] : undefined), value)

/** `value` with `to` at `path` (which it holds), copied rather than changed. */
export function withAt(value, [key, ...rest], to) {
  const copy = Array.isArray(value) ? [...value] : { ...value }
  copy[key] = rest.length === 0 ? to : withAt(value[key], rest, to)
  return copy
}

/**
 * The frames reporting one call of `tool`, with id `id`, which returned
 * `result` (an MCP `CallToolResult`): the recorded call's frames with this
 * call written at the places its harness carries a call (`PLACES`). Throws
 * for a call the recorded one cannot stand for (`unreplayable`), and for a
 * recording with no place for the result.
 */
export function callFrames(agent, recorded, { id, tool, result }) {
  const refused = unreplayable(tool, result)
  if (refused) throw new Error(`cannot replay: ${refused}`)
  const call = { id, tool, result }
  let results = 0
  const frames = recordedCall(agent, recorded).map((frame) =>
    PLACES[agent].reduce((each, place) => {
      const recordedValue = at(each, place.path)
      if (recordedValue === undefined) return each
      if (place.result) results += 1
      return withAt(each, place.path, place.put(recordedValue, call))
    }, frame),
  )
  if (results === 0)
    throw new Error(`the ${agent} recording has no place for ${RECORDED_TOOL}'s result`)
  return frames
}
