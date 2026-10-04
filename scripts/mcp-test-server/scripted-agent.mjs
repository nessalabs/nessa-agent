#!/usr/bin/env node
/**
 * A stdio ACP agent with no model, standing in for a harness under a real
 * gateway:
 *
 *   node scripted-agent.mjs codex|claude <tool>
 *
 * It answers the gateway's handshake as the harness pinned for `<agent>`
 * would, and to each prompt makes one real call of the test server's
 * `<tool>`, with the recorded call's arguments, through the stand-in the gateway gave it for
 * `mcptest`, then reports that call in the frames the harness was recorded
 * sending (`scripted-frames.mjs`), says DONE, and ends the turn. The
 * desktop's real-gateway check runs it with `--scripted`
 * (`verification/desktop/scripts/mcp-apps-gateway.mjs`), so what reaches the
 * window is the gateway's own projection of a recorded call, every run alike.
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
 * Not replayed: a harness's permission request for the call. The recordings
 * hold only `session/update` frames, so the scripted agent asks for none.
 */
import { spawn } from "node:child_process"
import { randomUUID } from "node:crypto"
import { createInterface } from "node:readline"

import {
  AGENTS,
  callFrames,
  configOptions,
  initialOptions,
  initializeResult,
  recordedArguments,
  recording,
  setOption,
} from "./scripted-frames.mjs"
import { SERVER } from "./local-gateway.mjs"

/** How long an MCP request waits for its stand-in's answer. */
const MCP_DEADLINE_MS = 10_000

const [agent, tool] = process.argv.slice(2)
if (!AGENTS.includes(agent) || !tool) {
  process.stderr.write(`usage: scripted-agent.mjs ${AGENTS.join("|")} <tool>\n`)
  process.exit(2)
}
const recorded = recording(agent)
const args = recordedArguments(agent, recorded)

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

  "session/prompt": async ({ sessionId }) => {
    const session = sessionOf(sessionId)
    const server = session.servers.get(SERVER)
    if (!server) throw new Error(`the session has no ${SERVER} server`)
    if (session.prompt) throw new Error(`session ${sessionId} is already in a prompt`)
    const prompt = { cancelled: false }
    session.prompt = prompt
    let result
    try {
      result = await server.request("tools/call", { name: tool, arguments: args })
    } catch (error) {
      // A cancel decides the turn however the call settles.
      if (prompt.cancelled) return { stopReason: "cancelled" }
      throw error
    } finally {
      session.prompt = null
    }
    // Cancelled while the call was in flight: the turn ends so, and reports
    // nothing (the recordings hold no cancelled call).
    if (prompt.cancelled) return { stopReason: "cancelled" }
    const id =
      agent === "claude" ? `toolu_scripted_${randomUUID()}` : `exec-${randomUUID()}`
    const update = (update) =>
      send({ method: "session/update", params: { sessionId, update } })
    for (const frame of callFrames(agent, recorded, { id, tool, result })) update(frame)
    update({
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "DONE" },
    })
    return { stopReason: "end_turn" }
  },
}

createInterface({ input: process.stdin })
  .on("line", async (line) => {
    let message
    try {
      message = JSON.parse(line)
    } catch {
      return
    }
    // A cancel marks the session's prompt in flight, if there is one; no
    // notification is answered.
    if (message.method === "session/cancel") {
      const prompt = sessions.get(message.params?.sessionId)?.prompt
      if (prompt) prompt.cancelled = true
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
