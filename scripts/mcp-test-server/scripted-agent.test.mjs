/**
 * `scripted-agent.mjs` over its stdio, as the gateway drives it, with the test
 * server itself as `mcptest` in place of the gateway's stand-in: one test per
 * row of its design table (#418).
 */
import { strict as assert } from "node:assert"
import { spawn, spawnSync } from "node:child_process"
import { dirname, join } from "node:path"
import { existsSync, mkdtempSync, readFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { createInterface } from "node:readline"
import { setTimeout as sleep } from "node:timers/promises"
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
/** Whether process `pid` is still running. */
const alive = (pid) => {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}
after(() => {
  for (const child of started) if (child.exitCode === null) child.kill("SIGKILL")
})

/**
 * The agent as `agent`, calling `review_rows`, with a `request` that resolves
 * with the answer and the notifications before it, or rejects after 20 s: an
 * agent that never answers fails its test rather than hanging the suite.
 */
function start(agent, env = {}) {
  const script = join(here, "scripted-agent.mjs")
  const child = spawn(process.execPath, [script, agent, "review_rows"], {
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
      new Promise((done, fail) => {
        const id = next++
        const timer = setTimeout(() => fail(new Error(`no answer to ${method}`)), 20_000)
        waiting.set(id, (answer) => {
          clearTimeout(timer)
          done(answer)
        })
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

test("a stand-in that never answers initialize fails session/new within the deadline, and is stopped", async (t) => {
  // It says nothing, and its stdio is the agent's: it writes its pid where
  // the test can see whether it is still running.
  const pidFile = join(mkdtempSync(join(tmpdir(), "scripted-agent-")), "pid")
  const silent = `require("node:fs").writeFileSync(${JSON.stringify(pidFile)}, String(process.pid)); setInterval(() => {}, 1000)`
  // Stopped after the test whatever it saw, so a failure here hangs nothing.
  t.after(() => {
    const left = existsSync(pidFile) ? Number(readFileSync(pidFile, "utf8")) : null
    if (left !== null && alive(left)) process.kill(left, "SIGKILL")
  })
  const agent = start("codex", codexEnv)
  const started = Date.now()
  const opened = await agent.request("session/new", {
    cwd: here,
    mcpServers: [{ ...mcptest, args: ["-e", silent] }],
  })
  assert.match(opened.error.message, /did not answer initialize within/)
  assert.ok(Date.now() - started < 15_000)
  const pid = Number(readFileSync(pidFile, "utf8"))
  const end = Date.now() + 5000
  while (alive(pid) && Date.now() < end) await sleep(100)
  assert.equal(alive(pid), false, "the stand-in that never answered is still running")
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("two sessions in one agent: each keeps its own options and servers", async () => {
  const agent = start("codex", codexEnv)
  const one = (await agent.request("session/new", { cwd: here, mcpServers: [mcptest] }))
    .result
  const two = (await agent.request("session/new", { cwd: here, mcpServers: [] })).result
  assert.notEqual(one.sessionId, two.sessionId)
  await agent.request("session/set_config_option", {
    sessionId: one.sessionId,
    configId: "mode",
    value: "agent",
  })
  const set = await agent.request("session/set_config_option", {
    sessionId: two.sessionId,
    configId: "reasoning_effort",
    value: "low",
  })
  assert.equal(
    set.result.configOptions.find((each) => each.id === "mode").currentValue,
    "read-only",
  )
  const first = await agent.request("session/prompt", {
    sessionId: one.sessionId,
    prompt: [],
  })
  assert.deepEqual(first.result, { stopReason: "end_turn" })
  const second = await agent.request("session/prompt", {
    sessionId: two.sessionId,
    prompt: [],
  })
  assert.match(second.error.message, /no mcptest server/)
  const unknown = await agent.request("session/prompt", { sessionId: "nope", prompt: [] })
  assert.match(unknown.error.message, /no session nope/)
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("an agent not named, or with no tool to call, exits 2 saying how to run it", () => {
  for (const args of [["codex"], ["opencode", "review_rows"], []]) {
    const run = spawnSync(process.execPath, [join(here, "scripted-agent.mjs"), ...args], {
      encoding: "utf8",
      timeout: 10_000,
    })
    assert.equal(run.status, 2, args.join(" "))
    assert.match(run.stderr, /usage: scripted-agent\.mjs codex\|claude <tool>/)
  }
})

/**
 * A stand-in that answers every request with `{}`, keeps running after its
 * input closes (as `server.mjs` does not), and writes its pid: what is
 * stopped is then the agent's doing. Returns its `session/new` entry and
 * `pid()`; it is killed after the test whatever happened.
 */
function lingering(t) {
  const pidFile = join(mkdtempSync(join(tmpdir(), "scripted-agent-")), "pid")
  const script = `require("node:fs").writeFileSync(${JSON.stringify(pidFile)}, String(process.pid))
require("node:readline").createInterface({ input: process.stdin }).on("line", (line) => {
  const m = JSON.parse(line)
  if (m.id !== undefined) process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: m.id, result: {} }) + "\\n")
})
setInterval(() => {}, 1000)`
  const pid = () => (existsSync(pidFile) ? Number(readFileSync(pidFile, "utf8")) : null)
  t.after(() => {
    if (pid() !== null && alive(pid())) process.kill(pid(), "SIGKILL")
  })
  return { server: { ...mcptest, name: "lingering", args: ["-e", script] }, pid }
}

/** Waits up to 5 s for process `pid` to have gone; whether it has. */
async function gone(pid) {
  const end = Date.now() + 5000
  while (alive(pid) && Date.now() < end) await sleep(100)
  return !alive(pid)
}

test("a session/new that fails stops the stand-ins it had already started", async (t) => {
  const first = lingering(t)
  const agent = start("codex", codexEnv)
  const opened = await agent.request("session/new", {
    cwd: here,
    mcpServers: [first.server, { ...mcptest, command: join(here, "no-such-command") }],
  })
  assert.match(opened.error.message, /stand-in/)
  assert.equal(await gone(first.pid()), true, "the first stand-in is still running")
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("an agent whose input closes stops its stand-ins", async (t) => {
  const stand = lingering(t)
  const agent = start("codex", codexEnv)
  const opened = await agent.request("session/new", {
    cwd: here,
    mcpServers: [stand.server],
  })
  assert.ok(opened.result.sessionId)
  assert.equal(alive(stand.pid()), true)
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
  assert.equal(await gone(stand.pid()), true, "the stand-in outlived its agent")
})
