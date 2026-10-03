/**
 * `scripted-agent.mjs` over its stdio, as the gateway drives it, with the test
 * server itself as `mcptest` in place of the gateway's stand-in: one test per
 * row of its design table (#418).
 */
import { strict as assert } from "node:assert"
import { spawn } from "node:child_process"
import { dirname, join } from "node:path"
import { createInterface } from "node:readline"
import { after, test } from "node:test"
import { fileURLToPath } from "node:url"

import { exited } from "./local-gateway.mjs"

const here = dirname(fileURLToPath(import.meta.url))
const mcptest = {
  name: "mcptest",
  command: process.execPath,
  args: [join(here, "server.mjs")],
  env: [],
}

const started = []
after(() => {
  for (const child of started) if (child.exitCode === null) child.kill("SIGKILL")
})

/** The agent as `agent`, with a `request` that resolves with the answer and the notifications before it. */
function start(agent, env = {}) {
  const child = spawn(process.execPath, [join(here, "scripted-agent.mjs"), agent], {
    env: { ...process.env, ...env },
    stdio: ["pipe", "pipe", "inherit"],
  })
  // A test that fails before closing its agent leaves nothing running.
  started.push(child)
  const notes = []
  const waiting = new Map()
  createInterface({ input: child.stdout }).on("line", (line) => {
    const message = JSON.parse(line)
    if (message.id === undefined) return notes.push(message)
    waiting.get(message.id)?.({ ...message, notes: notes.splice(0) })
  })
  let next = 1
  return {
    child,
    request: (method, params) =>
      new Promise((done) => {
        const id = next++
        waiting.set(id, done)
        child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`)
      }),
    notify: (method, params) =>
      child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method, params })}\n`),
  }
}

const codexEnv = {
  CODEX_CONFIG: JSON.stringify({ model: "gpt-test" }),
  INITIAL_AGENT_MODE: "read-only",
}

test("codex: handshake, options, one replayed call per prompt, and exit on close", async () => {
  const agent = start("codex", codexEnv)
  const init = await agent.request("initialize", { protocolVersion: 1 })
  assert.equal(init.result.protocolVersion, 1)
  assert.equal(init.result.agentInfo.name, "@agentclientprotocol/codex-acp")

  const opened = await agent.request("session/new", { cwd: here, mcpServers: [mcptest] })
  const { sessionId, configOptions } = opened.result
  assert.deepEqual(
    configOptions.map(({ id, currentValue }) => [id, currentValue]),
    [
      ["model", "gpt-test"],
      ["mode", "read-only"],
    ],
  )
  const set = await agent.request("session/set_config_option", {
    sessionId,
    configId: "mode",
    value: "agent",
  })
  assert.equal(
    set.result.configOptions.find((each) => each.id === "mode").currentValue,
    "agent",
  )
  const unknown = await agent.request("session/set_config_option", {
    sessionId,
    configId: "colour",
    value: "red",
  })
  assert.match(unknown.error.message, /no config option colour/)
  // The session keeps what was set: a later answer still reports it.
  const effort = await agent.request("session/set_config_option", {
    sessionId,
    configId: "reasoning_effort",
    value: "high",
  })
  assert.deepEqual(
    effort.result.configOptions.map(({ id, currentValue }) => [id, currentValue]),
    [
      ["model", "gpt-test"],
      ["mode", "agent"],
      ["reasoning_effort", "high"],
    ],
  )

  const first = await agent.request("session/prompt", { sessionId, prompt: [] })
  assert.deepEqual(first.result, { stopReason: "end_turn" })
  const tools = first.notes.map((note) => note.params.update).filter((u) => u.toolCallId)
  assert.equal(tools.length, 3)
  assert.equal(new Set(tools.map((u) => u.toolCallId)).size, 1)
  assert.deepEqual(tools.at(-1).rawOutput.result.structuredContent, { rows: [1, 2] })
  assert.equal(first.notes.at(-1).params.update.content.text, "DONE")

  // A second prompt is a second call, under an id of its own.
  const second = await agent.request("session/prompt", { sessionId, prompt: [] })
  const again = second.notes.map((note) => note.params.update).find((u) => u.toolCallId)
  assert.notEqual(again.toolCallId, tools[0].toolCallId)

  // A cancel has nothing to stop and no answer; the agent goes on.
  agent.notify("session/cancel", { sessionId })
  const other = await agent.request("session/load", {})
  assert.equal(other.error.code, -32601)

  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
  assert.equal(agent.child.exitCode, 0)
})

test("claude: the model from session/new, and the recorded four frames", async () => {
  const agent = start("claude")
  const opened = await agent.request("session/new", {
    cwd: here,
    mcpServers: [mcptest],
    _meta: {
      claudeCode: { options: { model: "claude-test", permissionMode: "default" } },
    },
  })
  assert.equal(opened.result.configOptions[0].currentValue, "claude-test")
  const turn = await agent.request("session/prompt", {
    sessionId: opened.result.sessionId,
    prompt: [],
  })
  const tools = turn.notes.map((note) => note.params.update).filter((u) => u.toolCallId)
  assert.equal(tools.length, 4)
  assert.equal(tools.at(-1).rawOutput, '{"rows":[1,2]}')
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("a stand-in that does not start fails session/new", async () => {
  const agent = start("codex", codexEnv)
  const opened = await agent.request("session/new", {
    cwd: here,
    mcpServers: [{ ...mcptest, command: join(here, "no-such-command") }],
  })
  assert.match(opened.error.message, /stand-in/)
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("a prompt with no mcptest server fails the turn", async () => {
  const agent = start("codex", codexEnv)
  const opened = await agent.request("session/new", { cwd: here, mcpServers: [] })
  const turn = await agent.request("session/prompt", {
    sessionId: opened.result.sessionId,
    prompt: [],
  })
  assert.match(turn.error.message, /no mcptest server/)
  assert.equal(turn.notes.length, 0)
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("a call the server refuses fails the turn, and nothing is reported", async () => {
  // A server that answers initialize and refuses everything else.
  const refusing = `require("node:readline").createInterface({ input: process.stdin }).on("line", (line) => {
    const m = JSON.parse(line)
    if (m.id === undefined) return
    const answer = m.method === "initialize" ? { result: {} } : { error: { code: -32000, message: "refused" } }
    process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: m.id, ...answer }) + "\\n")
  })`
  const agent = start("codex", codexEnv)
  const opened = await agent.request("session/new", {
    cwd: here,
    mcpServers: [{ ...mcptest, args: ["-e", refusing] }],
  })
  const turn = await agent.request("session/prompt", {
    sessionId: opened.result.sessionId,
    prompt: [],
  })
  assert.match(turn.error.message, /refused/)
  assert.equal(turn.notes.length, 0)
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})
