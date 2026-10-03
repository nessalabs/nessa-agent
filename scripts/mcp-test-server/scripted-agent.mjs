#!/usr/bin/env node
/**
 * A stdio ACP agent with no model, standing in for a harness under a real
 * gateway:
 *
 *   node scripted-agent.mjs codex|claude
 *
 * It answers the gateway's handshake as the harness pinned for `<agent>`
 * would, and to each prompt makes one real call of the test server's app
 * tool (`review_rows`) through the stand-in the gateway gave it for
 * `mcptest`, then reports that call in the frames the harness was recorded
 * sending (`scripted-frames.mjs`), says DONE, and ends the turn. The
 * desktop's real-gateway check runs it with `--scripted`
 * (`verification/desktop/scripts/mcp-apps-gateway.mjs`), so what reaches the
 * window is the gateway's own projection of a recorded call, every run alike.
 *
 * It keeps every stand-in it started for the session: the gateway's own
 * connection to the server, which the view's `resourceUri` and the app's
 * calls use, lives as long as the stand-in does. Its design table is on
 * #418. It reads no credential; the check starts the gateway without any for
 * it to be handed.
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
  recording,
  setOption,
} from "./scripted-frames.mjs"
import { SERVER } from "./local-gateway.mjs"

/** The app tool every prompt calls. */
const TOOL = "review_rows"

const agent = process.argv[2]
if (!AGENTS.includes(agent)) {
  process.stderr.write(`usage: scripted-agent.mjs ${AGENTS.join("|")}\n`)
  process.exit(2)
}
const recorded = recording(agent)

const send = (message) =>
  process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", ...message })}\n`)
const failure = (id, code, message) => send({ id, error: { code, message } })

/** One MCP server over a stand-in's stdio: `request(method, params)` resolves with its result. */
function mcpClient({ command, args, env }) {
  const child = spawn(command, args, {
    env: {
      ...process.env,
      ...Object.fromEntries((env ?? []).map((e) => [e.name, e.value])),
    },
    stdio: ["pipe", "pipe", "inherit"],
  })
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
        waiting.set(id, { resolve, reject })
        write({ id, method, params })
      }),
    notify: (method, params) => write({ method, params }),
    close: () => child.kill("SIGTERM"),
  }
}

/** Starts and initializes the MCP server `server` (a `session/new` entry). */
async function connect(server) {
  const client = mcpClient(server)
  await client.request("initialize", {
    protocolVersion: "2025-06-18",
    capabilities: {},
    clientInfo: { name: "scripted-agent", version: "0.1.0" },
  })
  client.notify("notifications/initialized", {})
  return client
}

/** The one session: its id, its options' values, and its MCP servers by name. */
let session = null

const handlers = {
  initialize: () => initializeResult(agent),

  "session/new": async (params) => {
    const values = initialOptions(agent, process.env, params)
    const servers = new Map()
    try {
      for (const server of params.mcpServers ?? [])
        servers.set(server.name, await connect(server))
    } catch (error) {
      for (const client of servers.values()) client.close()
      throw error
    }
    session = { id: randomUUID(), values, servers }
    return { sessionId: session.id, configOptions: configOptions(values) }
  },

  "session/set_config_option": ({ sessionId, configId, value }) => {
    if (sessionId !== session?.id) throw new Error(`no session ${sessionId}`)
    const values = setOption(session.values, configId, value)
    if (!values) throw new Error(`no config option ${configId}`)
    session.values = values
    return { configOptions: configOptions(values) }
  },

  "session/prompt": async ({ sessionId }) => {
    if (sessionId !== session?.id) throw new Error(`no session ${sessionId}`)
    const server = session.servers.get(SERVER)
    if (!server) throw new Error(`the session has no ${SERVER} server`)
    const args = {}
    const result = await server.request("tools/call", { name: TOOL, arguments: args })
    const id =
      agent === "claude" ? `toolu_scripted_${randomUUID()}` : `exec-${randomUUID()}`
    const update = (update) =>
      send({ method: "session/update", params: { sessionId, update } })
    for (const frame of callFrames(agent, recorded, {
      id,
      server: SERVER,
      tool: TOOL,
      args,
      result,
    }))
      update(frame)
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
    // A notification (`session/cancel` among them) has nothing to answer;
    // nothing is in flight between prompts to stop.
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
  .on("close", () => {
    for (const client of session?.servers.values() ?? []) client.close()
    process.exit(0)
  })
