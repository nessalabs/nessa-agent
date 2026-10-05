/**
 * A scripted turn as ordered steps, and the state table those steps run.
 * `scripted-agent.mjs` is the process; this module is the rule, so a test
 * can run a turn without a gateway. The recorded claude and codex frames
 * are not a scenario file: with no `--scenario`, the agent replays them.
 *
 * One prompt is one turn. A scenario's `turns` each name the prompt they
 * answer (`when`, a substring of the prompt's text) or answer any prompt
 * that named no turn. The first named match wins. One unnamed turn is the
 * fallback; a second is rejected, as two turns with the same `when` are.
 *
 * | state | event | next |
 * | --- | --- | --- |
 * | running | text | one `agent_message_chunk` per chunk, one message id, still running |
 * | running | tool | the MCP call, then its completion, still running |
 * | running | permission | the call is announced, the request is sent, waiting |
 * | waiting | selected, and the branch does not end or fail the turn | that answer's steps, then the steps after the request |
 * | waiting | selected, and the branch ends or fails the turn | stop there; the steps after the request do not run |
 * | waiting | selected, and the turn is already cancelled | stop `cancelled`; the answer's steps do not run |
 * | waiting | outcome cancelled, a `cancelled` branch, turn not cancelled | that branch |
 * | waiting | outcome cancelled, no branch, or the turn is cancelled | stop `cancelled` |
 * | waiting | an answer that is not a selection or a cancellation | the turn fails |
 * | running | fail | the turn fails; chunks and calls already sent stay sent |
 * | running | wait-cancel | waiting for `session/cancel` |
 * | waiting for cancel | `session/cancel` | stop `cancelled` |
 * | running | end, or the steps run out | stop `end_turn` |
 * | running | `session/cancel` between steps | stop `cancelled`; nothing further is sent |
 *
 * A permission offers two answers, the two the window can give: allow once
 * and deny once. A withdrawn review is the third answer, `cancelled`, not
 * an option. A branch holds text, a tool call, fail, and end — not another
 * permission, and not a wait for cancel, so a turn waits on one thing. The
 * call announced for the review is the branch's tool call. When the branch
 * does not complete it, that call is failed, and a tool step after the
 * review is a new call.
 */
import { SERVER } from "./local-gateway.mjs"
import { CLAUDE_CALL_ID } from "./scripted-frames.mjs"

/** The option id the window's Allow Once selects. */
export const ALLOW_ONCE = "allow-once"
/** The option id the window's Deny selects. */
export const DENY_ONCE = "deny-once"

const ANSWERS = [ALLOW_ONCE, DENY_ONCE, "cancelled"]
const KINDS = ["text", "permission", "tool", "fail", "wait-cancel", "end"]
const BRANCH_KINDS = ["text", "tool", "fail", "end"]

const FIELDS = {
  text: ["do", "chunks"],
  permission: ["do", "tool", "arguments", "title", "on"],
  tool: ["do", "tool", "arguments"],
  fail: ["do", "message"],
  "wait-cancel": ["do"],
  end: ["do"],
}

const OPTIONS = [
  { optionId: ALLOW_ONCE, kind: "allow_once", name: "Allow once" },
  { optionId: DENY_ONCE, kind: "reject_once", name: "Deny once" },
]

const plain = (value) =>
  value !== null &&
  typeof value === "object" &&
  Object.getPrototypeOf(value) === Object.prototype

/** What a scenario file says, or a throw naming the first thing wrong with it. */
export function parseScenario(value) {
  if (!plain(value) || !Array.isArray(value.turns) || value.turns.length === 0)
    throw new Error("a scenario has a non-empty turns array")
  const extra = Object.keys(value).filter((key) => key !== "turns")
  if (extra.length > 0) throw new Error(`unknown scenario field ${extra[0]}`)
  const seen = new Set()
  let fallback = false
  const turns = value.turns.map((turn, index) => {
    if (!plain(turn) || !Array.isArray(turn.steps))
      throw new Error(`turn ${index + 1} has no steps`)
    const when = Object.hasOwn(turn, "when") ? turn.when : undefined
    if (when !== undefined && (typeof when !== "string" || when === ""))
      throw new Error(`turn ${index + 1} has an empty when`)
    if (when === undefined) {
      if (fallback) throw new Error("two turns match any prompt")
      fallback = true
    } else if (seen.has(when)) {
      throw new Error(`two turns match ${when}`)
    } else {
      for (const earlier of seen)
        if (when.includes(earlier))
          throw new Error(`a turn matching ${when} already matches ${earlier}`)
      seen.add(when)
    }
    const unknown = Object.keys(turn).filter((key) => key !== "when" && key !== "steps")
    if (unknown.length > 0) throw new Error(`unknown turn field ${unknown[0]}`)
    return {
      ...(when === undefined ? {} : { when }),
      steps: parseSteps(turn.steps, false),
    }
  })
  return { turns }
}

function parseSteps(steps, branch) {
  if (!Array.isArray(steps) || steps.length === 0) throw new Error("a step list is empty")
  const parsed = steps.map((step) => parseStep(step, branch))
  const terminal = parsed.findIndex((step) =>
    ["fail", "wait-cancel", "end"].includes(step.do),
  )
  if (terminal !== -1 && terminal !== parsed.length - 1)
    throw new Error(`steps follow ${parsed[terminal].do}, which ends the turn`)
  return parsed
}

function parseStep(step, branch) {
  if (!plain(step) || typeof step.do !== "string" || !KINDS.includes(step.do))
    throw new Error("a step's do is text, permission, tool, fail, wait-cancel, or end")
  if (branch && !BRANCH_KINDS.includes(step.do))
    throw new Error(`a permission answer cannot ${step.do}`)
  const allowed = FIELDS[step.do]
  const unknown = Object.keys(step).filter((key) => !allowed.includes(key))
  if (unknown.length > 0) throw new Error(`unknown ${step.do} field ${unknown[0]}`)
  for (const key of allowed)
    if (key !== "do" && !Object.hasOwn(step, key))
      throw new Error(`a ${step.do} step needs ${key}`)
  switch (step.do) {
    case "text":
      if (
        !Array.isArray(step.chunks) ||
        step.chunks.length === 0 ||
        step.chunks.some((chunk) => typeof chunk !== "string" || chunk === "")
      )
        throw new Error("text chunks are a non-empty list of non-empty strings")
      return { do: "text", chunks: [...step.chunks] }
    case "tool":
      return {
        do: "tool",
        tool: toolName(step.tool),
        arguments: argumentsOf(step.arguments),
      }
    case "fail":
      if (typeof step.message !== "string" || step.message === "")
        throw new Error("a failure needs a message")
      return { do: "fail", message: step.message }
    case "permission":
      return {
        do: "permission",
        tool: toolName(step.tool),
        arguments: argumentsOf(step.arguments),
        title: titleOf(step.title),
        on: answers(step.on),
      }
    default:
      return { do: step.do }
  }
}

function toolName(name) {
  if (typeof name !== "string" || !/^[A-Za-z0-9_-]+$/.test(name))
    throw new Error(
      `a tool name is letters, digits, "_" and "-": ${JSON.stringify(name)}`,
    )
  return name
}

function argumentsOf(value) {
  if (!plain(value)) throw new Error("a tool's arguments are an object")
  return value
}

function titleOf(title) {
  if (typeof title !== "string" || title.trim() === "")
    throw new Error("a permission needs a title")
  return title
}

function answers(on) {
  if (!plain(on)) throw new Error("a permission's on is an object")
  const unknown = Object.keys(on).filter((key) => !ANSWERS.includes(key))
  if (unknown.length > 0) throw new Error(`unknown permission answer ${unknown[0]}`)
  const parsed = {}
  for (const name of ANSWERS) {
    if (!Object.hasOwn(on, name)) continue
    parsed[name] = parseSteps(on[name], true)
  }
  if (Object.keys(parsed).length === 0) throw new Error("a permission names no answer")
  return parsed
}

/** The prompt's text blocks, in order, joined. Anything else in the prompt is not text. */
export function promptText(prompt) {
  if (!Array.isArray(prompt)) return ""
  return prompt
    .filter(
      (block) => plain(block) && block.type === "text" && typeof block.text === "string",
    )
    .map((block) => block.text)
    .join("")
}

/**
 * The strings a window check shows and sends, read off the scenario: the
 * permission turn, the failure, and the cancel. The file is the owner; a
 * check does not retype them.
 */
export function scenarioScript(scenario) {
  const turn = (kind) =>
    scenario.turns.find((each) => each.steps.some((step) => step.do === kind))
  const permission = turn("permission")
  const failure = turn("fail")
  const cancel = turn("wait-cancel")
  if (!permission?.when || !failure?.when || !cancel?.when)
    throw new Error("the scenario has no permission, failure and cancel turns")
  const ask = permission.steps.find((step) => step.do === "permission")
  const asked = permission.steps.find((step) => step.do === "text")
  const allowed = ask.on[ALLOW_ONCE]?.find((step) => step.do === "text")
  const beforeFailure = failure.steps.find((step) => step.do === "text")
  const beforeCancel = cancel.steps.find((step) => step.do === "text")
  if (!ask || !asked || !allowed || !beforeFailure || !beforeCancel)
    throw new Error("the scenario's turns do not say what the window should show")
  return {
    permission: permission.when,
    asking: asked.chunks.join(""),
    title: ask.title,
    allowed: allowed.chunks.join(""),
    fail: failure.when,
    beforeFailure: beforeFailure.chunks.join(""),
    cancel: cancel.when,
    beforeCancel: beforeCancel.chunks.join(""),
  }
}

/**
 * The turn that answers `text`: the first whose `when` is a substring, else
 * the turn that names no prompt. `null` when nothing answers it.
 */
export function turnFor(scenario, text) {
  return (
    scenario.turns.find((turn) => turn.when !== undefined && text.includes(turn.when)) ??
    scenario.turns.find((turn) => turn.when === undefined) ??
    null
  )
}

/**
 * What a permission answer does. `cancelled` is the turn's own cancel, which
 * wins over a selection that arrives after it. `result` is the gateway's
 * reply to `session/request_permission`.
 */
export function branchFor(result, cancelled, on) {
  if (cancelled) return { kind: "stop", stopReason: "cancelled" }
  const outcome = plain(result) ? result.outcome : undefined
  if (!plain(outcome))
    return { kind: "fail", message: "the permission answer was not an outcome" }
  if (outcome.outcome === "cancelled") {
    if (Object.hasOwn(on, "cancelled")) return { kind: "branch", steps: on.cancelled }
    return { kind: "stop", stopReason: "cancelled" }
  }
  if (outcome.outcome === "selected" && typeof outcome.optionId === "string") {
    if (!Object.hasOwn(on, outcome.optionId))
      return { kind: "fail", message: `no branch for ${outcome.optionId}` }
    return { kind: "branch", steps: on[outcome.optionId] }
  }
  return {
    kind: "fail",
    message: "the permission answer was not a selection or a cancellation",
  }
}

/**
 * The call `agent` announces, and the permission request that names it.
 * Claude's harness names the tool `mcp__<server>__<tool>`; Codex marks an
 * MCP call and puts the server and the tool in `rawInput`.
 */
export function permissionCall(agent, { id, tool, title, arguments: args }) {
  const rawInput = agent === "codex" ? { server: SERVER, tool, arguments: args } : args
  const meta =
    agent === "claude"
      ? { claudeCode: { toolName: `mcp__${SERVER}__${tool}` } }
      : { is_mcp_tool_call: true }
  const toolCall = {
    toolCallId: id,
    title,
    kind: "other",
    status: "pending",
    rawInput,
    _meta: meta,
  }
  return {
    update: { sessionUpdate: "tool_call", ...toolCall },
    request: (sessionId) => ({ sessionId, toolCall, options: OPTIONS }),
  }
}

/** A `tools/call` for the stand-in. Claude names the ACP call id; Codex names none. */
export function mcpArguments(agent, { id, tool, arguments: args }) {
  return {
    name: tool,
    arguments: args,
    ...(agent === "claude" ? { _meta: { [CLAUDE_CALL_ID]: id } } : {}),
  }
}

/** Text the window can show for an MCP result: its first text block, or a short stand-in. */
export function resultText(result) {
  const block = Array.isArray(result?.content)
    ? result.content.find(
        (each) => each?.type === "text" && typeof each.text === "string",
      )
    : undefined
  const text = block?.text ?? "done"
  return text.length > 500 ? text.slice(0, 500) : text
}

/** The completion of a call, failed when the server said `isError`. */
export function completedUpdate(id, result) {
  return {
    sessionUpdate: "tool_call_update",
    toolCallId: id,
    status: result?.isError === true ? "failed" : "completed",
    content: [{ type: "content", content: { type: "text", text: resultText(result) } }],
  }
}

/**
 * Runs `steps`. Returns `{ stopReason }` or `{ fail }`. A branch that finishes
 * does not end the turn: the steps after its permission still run.
 *
 * `ctx` carries the turn: `cancelled()`, `untilCancelled()`, `update(frame)`,
 * `requestPermission(params)`, `callTool(args)`, `messageId()`, `toolId()`,
 * `openCall(tool)`, `rememberCall(tool, id)`, `clearCall(tool)`, `agent`,
 * `sessionId`.
 */
export async function runSteps(steps, ctx) {
  for (const step of steps) {
    if (ctx.cancelled()) return { stopReason: "cancelled" }
    const outcome = await runStep(step, ctx)
    if (outcome) return outcome
  }
  return { stopReason: "end_turn" }
}

async function runStep(step, ctx) {
  switch (step.do) {
    case "text": {
      const messageId = ctx.messageId()
      for (const text of step.chunks) {
        if (ctx.cancelled()) return { stopReason: "cancelled" }
        ctx.update({
          sessionUpdate: "agent_message_chunk",
          content: { type: "text", text },
          messageId,
        })
      }
      return null
    }
    case "tool":
      return callTool(step, ctx, ctx.openCall(step.tool))
    case "permission":
      return ask(step, ctx)
    case "fail":
      return { fail: step.message }
    case "wait-cancel":
      await ctx.untilCancelled()
      return { stopReason: "cancelled" }
    case "end":
      return { stopReason: "end_turn" }
    default:
      return { fail: `unknown step ${step.do}` }
  }
}

async function callTool(step, ctx, existing) {
  const id = existing ?? ctx.toolId()
  if (!existing) {
    const announced = permissionCall(ctx.agent, {
      id,
      tool: step.tool,
      title: step.tool,
      arguments: step.arguments,
    })
    ctx.update(announced.update)
  }
  let result
  try {
    result = await ctx.callTool(
      mcpArguments(ctx.agent, { id, tool: step.tool, arguments: step.arguments }),
    )
  } catch (error) {
    if (ctx.cancelled()) return { stopReason: "cancelled" }
    return { fail: error.message }
  }
  if (ctx.cancelled()) return { stopReason: "cancelled" }
  ctx.update(completedUpdate(id, result))
  ctx.clearCall(step.tool)
  return null
}

async function ask(step, ctx) {
  const id = ctx.toolId()
  const call = permissionCall(ctx.agent, {
    id,
    tool: step.tool,
    title: step.title,
    arguments: step.arguments,
  })
  ctx.update(call.update)
  ctx.rememberCall(step.tool, id)
  // The branch did not complete the announced call. Fail it before a later
  // step can reuse its id, and leave a cancelled turn's call alone.
  const abandon = () => {
    if (ctx.cancelled() || ctx.openCall(step.tool) !== id) return
    ctx.update({
      sessionUpdate: "tool_call_update",
      toolCallId: id,
      status: "failed",
    })
    ctx.clearCall(step.tool)
  }
  let result
  try {
    result = await ctx.requestPermission(call.request(ctx.sessionId))
  } catch (error) {
    if (ctx.cancelled()) return { stopReason: "cancelled" }
    abandon()
    return { fail: error.message }
  }
  const decision = branchFor(result, ctx.cancelled(), step.on)
  if (decision.kind === "stop") return { stopReason: decision.stopReason }
  if (decision.kind === "fail") {
    abandon()
    return { fail: decision.message }
  }
  let outcome = null
  for (const branch of decision.steps) {
    if (ctx.cancelled()) {
      outcome = { stopReason: "cancelled" }
      break
    }
    outcome = await runStep(branch, ctx)
    if (outcome) break
  }
  abandon()
  return outcome
}
