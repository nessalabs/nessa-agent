#!/usr/bin/env node
/**
 * A stdio ACP agent with no model, standing in for a harness under a real
 * gateway:
 *
 *   node scripted-agent.mjs codex|claude <tool>
 *   node scripted-agent.mjs codex|claude --scenario <file>
 *
 * With `<tool>`, it answers the gateway's handshake as the harness pinned
 * for `<agent>` would, and to each prompt makes one real call of the test
 * server's `<tool>`, with the recorded call's arguments, through the
 * stand-in the gateway gave it for `mcptest`, then reports that call in the
 * frames the harness was recorded sending (`scripted-frames.mjs`), says
 * DONE, and ends the turn. That is the default: the recorded claude and
 * codex frames, so the checks that already run it keep their behavior. The
 * desktop's real-gateway check runs it with `--scripted`
 * (`verification/desktop/scripts/mcp-apps-gateway.mjs`), so what reaches the
 * window is the gateway's own projection of a recorded call, every run alike.
 *
 * With `--scenario`, a prompt runs that file's steps instead
 * (`scripted-scenario.mjs`): text, a permission and the answer's branch, an
 * MCP call, a failure, a wait for cancel, or the end of the turn. The
 * handshake is still the harness's. A prompt the file does not answer fails
 * the turn.
 *
 * It keeps every stand-in it started for the session: the gateway's own
 * connection to the server, which the view's `resourceUri` and the app's
 * calls use, lives as long as the stand-in does. A stand-in that does not
 * answer within `MCP_DEADLINE_MS` fails what was waiting on it. When its input
 * closes or its output fails, it stops every stand-in and waits for each to
 * exit (`stop`) before it exits. Its design table is on #418. It reads no
 * credential; the check starts the gateway signed out (`startLocalGateway`'s
 * `signedOut`).
 *
 * Not replayed, on the recorded path: a harness's permission request for the
 * call. The recordings hold only `session/update` frames, so that path asks
 * for none. A scenario file can ask (`scripted-scenario.mjs`).
 *
 * Not from the recordings: where the call's `tools/call` names it. Claude's
 * harness puts the call's ACP `toolCallId` in its params'
 * `_meta["claudecode/toolUseId"]`, and the gateway's stand-in keeps the
 * result's `structuredContent` under that id for the SDK to attach to the call
 * (the SDK's `CALL_ID` in `stand_in.rs`, and its `forwarded.rs` tests; here
 * `CLAUDE_CALL_ID`). Codex's harness names no call id (no
 * `_meta["claudecode/toolUseId"]`), so its call is sent with no `_meta`. This
 * is the one value the replay takes from the real harness's MCP side, since
 * the recordings hold only the ACP frames.
 */
import { spawn } from "node:child_process"
import { randomUUID } from "node:crypto"
import { readFileSync } from "node:fs"
import { createInterface } from "node:readline"
import { parseArgs } from "node:util"

import {
  AGENTS,
  CLAUDE_CALL_ID,
  callFrames,
  configOptions,
  initialOptions,
  initializeResult,
  recordedArguments,
  recording,
  setOption,
} from "./scripted-frames.mjs"
import { promptText, runSteps, turnFor, parseScenario } from "./scripted-scenario.mjs"
import { SERVER } from "./local-gateway.mjs"

/** How long an MCP request waits for its stand-in's answer. */
const MCP_DEADLINE_MS = 10_000

let parsed
try {
  parsed = parseArgs({
    args: process.argv.slice(2),
    options: { scenario: { type: "string" } },
    allowPositionals: true,
  })
} catch (error) {
  process.stderr.write(`scripted-agent: ${error.message}\n`)
  process.exit(2)
}
const {
  values: { scenario: scenarioPath },
  positionals,
} = parsed
const [agent, tool, ...rest] = positionals
if (
  !AGENTS.includes(agent) ||
  rest.length > 0 ||
  (scenarioPath ? tool !== undefined : !tool)
) {
  process.stderr.write(
    `usage: scripted-agent.mjs ${AGENTS.join("|")} <tool>\n       scripted-agent.mjs ${AGENTS.join("|")} --scenario <file>\n`,
  )
  process.exit(2)
}
let scenario = null
if (scenarioPath) {
  let file
  try {
    file = JSON.parse(readFileSync(scenarioPath, "utf8"))
  } catch (error) {
    process.stderr.write(`scripted-agent: ${error.message}\n`)
    process.exit(2)
  }
  try {
    scenario = parseScenario(file)
  } catch (error) {
    process.stderr.write(`scripted-agent: ${error.message}\n`)
    process.exit(2)
  }
}

const send = (message) =>
  process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", ...message })}\n`)
const failure = (id, code, message) => send({ id, error: { code, message } })

/** How long a stopped stand-in has to exit before it is killed. */
const STOP_GRACE_MS = 2_000

/**
 * Every stand-in started, connected or still connecting, by its process, with
 * a promise taken at spawn that it has ended: its `exit`, by which it is
 * reaped (not `close`, which a process of its own holding its pipes can put
 * off forever), or an `error`, which a spawn that failed emits in place of
 * `exit` (a kill the system refuses also emits `error`, and ends the wait
 * though the process lives on; a stand-in of our own user never meets one).
 * One that has already ended has nothing left to wait for. Stopped when the
 * agent's input closes or its output fails.
 */
const standIns = new Map()

/** Set once a stop begins: the agent stops once, and starts no stand-in after. */
let stopping = false
/**
 * Stops every stand-in and waits for each to exit, killing one still running
 * after `STOP_GRACE_MS`; then exits 0, its stand-ins reaped (the tests of
 * a stand-in that ignores SIGTERM). A second stop while one is in progress
 * does nothing more, and a stand-in asked for meanwhile is not started
 * (`mcpClient`; the test of a session/new during the grace).
 */
async function stop() {
  if (stopping) return
  stopping = true
  await Promise.all(
    [...standIns].map(async ([child, ended]) => {
      // A failed spawn never has a pid, but until its `error` Node still holds
      // its process handle, so a kill then would signal the agent's process
      // group. A stop never comes first: it begins on an input or output
      // event, later than that `error`'s tick.
      child.kill("SIGTERM")
      const timer = setTimeout(() => child.kill("SIGKILL"), STOP_GRACE_MS)
      await ended
      clearTimeout(timer)
    }),
  )
  process.exit(0)
}

// A gateway that stops reading the agent leaves it nothing to do.
process.stdout.on("error", stop)

/**
 * One MCP server over a stand-in's stdio: `request(method, params)` resolves
 * with its result. Throws, starting nothing, once a stop has begun: the stop
 * has already chosen the stand-ins it waits for.
 */
function mcpClient({ command, args, env }) {
  if (stopping) throw new Error("the agent is stopping")
  const child = spawn(command, args, {
    env: {
      ...process.env,
      ...Object.fromEntries((env ?? []).map((e) => [e.name, e.value])),
    },
    stdio: ["pipe", "pipe", "inherit"],
  })
  standIns.set(
    child,
    new Promise((ended) => {
      child.once("exit", ended)
      child.once("error", ended)
    }),
  )
  const waiting = new Map()
  let next = 1
  let ended = null
  const end = (why) => {
    ended ??= why
    for (const { reject } of waiting.values()) reject(new Error(ended))
    waiting.clear()
  }
  child.on("error", (error) => end(`the stand-in did not start: ${error.message}`))
  child.on("exit", (code) => end(`the stand-in exited (${code})`))
  // A stand-in that stops reading fails what waits on it, rather than the agent.
  child.stdin.on("error", (error) => end(`the stand-in's input closed: ${error.message}`))
  createInterface({ input: child.stdout }).on("line", (line) => {
    let message
    try {
      message = JSON.parse(line)
    } catch {
      return
    }
    const wait = waiting.get(message.id)
    if (!wait) return
    waiting.delete(message.id)
    if (message.error) wait.reject(new Error(message.error.message))
    else wait.resolve(message.result)
  })
  const write = (message) =>
    child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", ...message })}\n`)
  return {
    request: (method, params) =>
      new Promise((resolve, reject) => {
        if (ended) return reject(new Error(ended))
        const id = next++
        const timer = setTimeout(() => {
          waiting.delete(id)
          reject(
            new Error(
              `the stand-in did not answer ${method} within ${MCP_DEADLINE_MS} ms`,
            ),
          )
        }, MCP_DEADLINE_MS)
        const settle = (then) => (value) => {
          clearTimeout(timer)
          then(value)
        }
        waiting.set(id, { resolve: settle(resolve), reject: settle(reject) })
        write({ id, method, params })
      }),
    notify: (method, params) => write({ method, params }),
    close: () => child.kill("SIGTERM"),
  }
}

/** Starts and initializes the MCP server `server` (a `session/new` entry); stops it again if it does not initialize. */
async function connect(server) {
  const client = mcpClient(server)
  try {
    await client.request("initialize", {
      protocolVersion: "2025-06-18",
      capabilities: {},
      clientInfo: { name: "scripted-agent", version: "0.1.0" },
    })
  } catch (error) {
    client.close()
    throw error
  }
  client.notify("notifications/initialized", {})
  return client
}

/** The sessions by id: each one's options' values, and its MCP servers by name. */
const sessions = new Map()

/** The session `sessionId` names; throws for one this agent did not open. */
function sessionOf(sessionId) {
  const session = sessions.get(sessionId)
  if (!session) throw new Error(`no session ${sessionId}`)
  return session
}

const handlers = {
  initialize: () => initializeResult(agent),

  "session/new": async (params) => {
    const values = initialOptions(agent, process.env, params)
    const names = (params.mcpServers ?? []).map((server) => server.name)
    const twice = names.findIndex((name, index) => names.indexOf(name) !== index)
    if (twice !== -1)
      throw new Error(
        !names[twice]
          ? "two MCP servers with no name"
          : `two MCP servers named ${names[twice]}`,
      )
    const servers = new Map()
    try {
      for (const server of params.mcpServers ?? [])
        servers.set(server.name, await connect(server))
    } catch (error) {
      for (const client of servers.values()) client.close()
      throw error
    }
    const sessionId = randomUUID()
    sessions.set(sessionId, { values, servers, prompt: null })
    return { sessionId, configOptions: configOptions(agent, values) }
  },

  "session/set_config_option": ({ sessionId, configId, value }) => {
    const session = sessionOf(sessionId)
    const values = setOption(agent, session.values, configId, value)
    if (!values) throw new Error(`no config option ${configId}`)
    session.values = values
    return { configOptions: configOptions(agent, values) }
  },

  "session/prompt": async (params) =>
    scenario ? scenarioTurn(params) : recordedTurn(params),
}

/**
 * The default turn: one recorded call of `tool`, then DONE. Loaded here, not
 * at startup, so a scenario run does not need the recording on disk.
 */
async function recordedTurn({ sessionId }) {
  const recorded = recording(agent)
  const args = recordedArguments(agent, recorded)
  const session = sessionOf(sessionId)
  const server = session.servers.get(SERVER)
  if (!server) throw new Error(`the session has no ${SERVER} server`)
  if (session.prompt) throw new Error(`session ${sessionId} is already in a prompt`)
  const prompt = { cancelled: false, waiters: [] }
  session.prompt = prompt
  // The call's id, chosen before the call: Claude's harness names the call
  // by it in the `tools/call`, and the frames below carry the same one.
  const id =
    agent === "claude" ? `toolu_scripted_${randomUUID()}` : `exec-${randomUUID()}`
  const call = {
    name: tool,
    arguments: args,
    ...(agent === "claude" ? { _meta: { [CLAUDE_CALL_ID]: id } } : {}),
  }
  let result
  try {
    result = await server.request("tools/call", call)
  } catch (error) {
    // A cancel decides the turn however the call settles.
    if (prompt.cancelled) return { stopReason: "cancelled" }
    throw error
  } finally {
    session.prompt = null
  }
  // Cancelled while the call was in flight: the turn ends so, and reports
  // nothing (the recordings hold no cancelled call). The stand-in may still
  // keep the call's result under its id, unreported, until the SDK's bound
  // of 32 kept results drops it or its grant goes
  // (docs/design/mcp-connections.md, Forwarded results, S9).
  if (prompt.cancelled) return { stopReason: "cancelled" }
  const update = (frame) =>
    send({ method: "session/update", params: { sessionId, update: frame } })
  for (const frame of callFrames(agent, recorded, { id, tool, result })) update(frame)
  update({
    sessionUpdate: "agent_message_chunk",
    content: { type: "text", text: "DONE" },
  })
  return { stopReason: "end_turn" }
}

/** How long a permission request waits for the gateway's answer. */
const PERMISSION_DEADLINE_MS = 120_000

/** Replies the gateway owes this agent, by the request id it sent. */
const replies = new Map()

/** One prompt of the scenario: the turn `prompt` matches, or a failure when none does. */
async function scenarioTurn({ sessionId, prompt: blocks }) {
  const session = sessionOf(sessionId)
  const server = session.servers.get(SERVER)
  if (!server) throw new Error(`the session has no ${SERVER} server`)
  if (session.prompt) throw new Error(`session ${sessionId} is already in a prompt`)
  const matched = turnFor(scenario, promptText(blocks))
  if (!matched) throw new Error("the scenario does not answer this prompt")
  const state = { cancelled: false, waiters: [] }
  session.prompt = state
  const calls = new Map()
  const update = (frame) =>
    send({ method: "session/update", params: { sessionId, update: frame } })
  try {
    const outcome = await runSteps(matched.steps, {
      agent,
      sessionId,
      cancelled: () => state.cancelled,
      untilCancelled: () =>
        state.cancelled
          ? Promise.resolve()
          : new Promise((resolve) => state.waiters.push(resolve)),
      update,
      requestPermission: (params) =>
        new Promise((resolve, reject) => {
          const id = `perm-${randomUUID()}`
          const timer = setTimeout(() => {
            replies.delete(id)
            reject(
              new Error(
                `the gateway did not answer the permission request within ${PERMISSION_DEADLINE_MS} ms`,
              ),
            )
          }, PERMISSION_DEADLINE_MS)
          replies.set(id, (message) => {
            clearTimeout(timer)
            if (message.error)
              reject(new Error(message.error.message ?? "the permission request failed"))
            else resolve(message.result)
          })
          send({ id, method: "session/request_permission", params })
        }),
      callTool: (args) => server.request("tools/call", args),
      messageId: () => `msg-${randomUUID()}`,
      toolId: () =>
        agent === "claude" ? `toolu_scripted_${randomUUID()}` : `exec-${randomUUID()}`,
      openCall: (name) => calls.get(name),
      rememberCall: (name, id) => calls.set(name, id),
      clearCall: (name) => calls.delete(name),
    })
    if (outcome.fail) throw new Error(outcome.fail)
    return { stopReason: outcome.stopReason }
  } finally {
    session.prompt = null
  }
}

createInterface({ input: process.stdin })
  .on("line", async (line) => {
    let message
    try {
      message = JSON.parse(line)
    } catch {
      return
    }
    // A cancel marks the session's prompt in flight, if there is one, and
    // wakes a step that is waiting for it. No notification is answered.
    if (message.method === "session/cancel") {
      const prompt = sessions.get(message.params?.sessionId)?.prompt
      if (prompt) {
        prompt.cancelled = true
        const waiting = prompt.waiters ?? []
        prompt.waiters = []
        for (const wake of waiting) wake()
      }
      return
    }
    // A reply to a permission request this agent sent. It has an id and no
    // method; a request from the gateway has both.
    if (message.id !== undefined && message.method === undefined) {
      const reply = replies.get(message.id)
      if (!reply) return
      replies.delete(message.id)
      reply(message)
      return
    }
    if (message.id === undefined || !message.method) return
    const handler = Object.hasOwn(handlers, message.method)
      ? handlers[message.method]
      : null
    if (!handler) return failure(message.id, -32601, `no method ${message.method}`)
    try {
      send({ id: message.id, result: await handler(message.params ?? {}) })
    } catch (error) {
      failure(message.id, -32603, error.message)
    }
  })
  .on("close", stop)
