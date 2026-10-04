/**
 * `scripted-agent.mjs` over its stdio, as the gateway drives it, with the test
 * server itself as `mcptest` in place of the gateway's stand-in: one test per
 * row of its design table (#418).
 */
import { strict as assert } from "node:assert"
import { spawn, spawnSync } from "node:child_process"
import { dirname, join } from "node:path"
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { createInterface } from "node:readline"
import { setTimeout as sleep } from "node:timers/promises"
import { after, test } from "node:test"
import { fileURLToPath } from "node:url"

import { exited } from "./local-gateway.mjs"
import { CLAUDE_CALL_ID } from "./scripted-frames.mjs"

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
function start(agent, env = {}, tool = "review_rows") {
  const script = join(here, "scripted-agent.mjs")
  const child = spawn(process.execPath, [script, agent, tool], {
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

/** The design's 2 s a stopped stand-in has before it is killed (#418; the agent's `STOP_GRACE_MS`). */
const STOP_GRACE_MS = 2000

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

/**
 * The test server as `mcptest`, answering as `server.mjs` does, that also
 * writes the params of each `tools/call` it receives, one JSON line each, to a
 * file: `calls()` reads them back. Like `server.mjs`, it exits when its input
 * closes, so an agent killed after the test leaves it nothing to run on; the
 * file's directory is removed after the test.
 */
function recordingCalls(t) {
  const dir = mkdtempSync(join(tmpdir(), "scripted-agent-"))
  t.after(() => rmSync(dir, { recursive: true, force: true }))
  const callsFile = join(dir, "calls")
  const server = JSON.stringify(join(here, "server.mjs"))
  const script = `const { answer } = await import(${server})
const { appendFileSync } = await import("node:fs")
const { createInterface } = await import("node:readline")
createInterface({ input: process.stdin }).on("line", (line) => {
  const m = JSON.parse(line)
  if (m.method === "tools/call") appendFileSync(${JSON.stringify(callsFile)}, JSON.stringify(m.params) + "\\n")
  const reply = answer(m)
  if (reply) process.stdout.write(JSON.stringify(reply) + "\\n")
})`
  const calls = () =>
    existsSync(callsFile)
      ? readFileSync(callsFile, "utf8").trim().split("\n").map(JSON.parse)
      : []
  return {
    server: { ...mcptest, args: ["--input-type=module", "-e", script] },
    calls,
  }
}

test("claude: the tools/call names the call in _meta, by the frames' toolCallId", async (t) => {
  const recorder = recordingCalls(t)
  const agent = start("claude")
  const opened = await agent.request("session/new", {
    cwd: here,
    mcpServers: [recorder.server],
    _meta: { claudeCode: { options: { model: "claude-test" } } },
  })
  const turn = await agent.request("session/prompt", {
    sessionId: opened.result.sessionId,
    prompt: [],
  })
  assert.deepEqual(turn.result, { stopReason: "end_turn" })
  const ids = new Set(
    turn.notes.map((note) => note.params.update.toolCallId).filter(Boolean),
  )
  assert.equal(ids.size, 1)
  const calls = recorder.calls()
  assert.equal(calls.length, 1)
  assert.equal(calls[0].name, "review_rows")
  assert.equal(calls[0]._meta?.[CLAUDE_CALL_ID], [...ids][0])
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test(`codex: the tools/call names no call id (no _meta["${CLAUDE_CALL_ID}"])`, async (t) => {
  const recorder = recordingCalls(t)
  const agent = start("codex", codexEnv)
  const opened = await agent.request("session/new", {
    cwd: here,
    mcpServers: [recorder.server],
  })
  const turn = await agent.request("session/prompt", {
    sessionId: opened.result.sessionId,
    prompt: [],
  })
  assert.deepEqual(turn.result, { stopReason: "end_turn" })
  const calls = recorder.calls()
  assert.equal(calls.length, 1)
  assert.equal(calls[0].name, "review_rows")
  assert.equal(calls[0]._meta?.[CLAUDE_CALL_ID], undefined)
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
  // At once: a spawn that failed has ended, so the stop has no grace to wait out.
  assert.equal(await exited(agent.child, STOP_GRACE_MS - 500), true)
  assert.equal(agent.child.exitCode, 0)
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
  const pidDir = mkdtempSync(join(tmpdir(), "scripted-agent-"))
  const pidFile = join(pidDir, "pid")
  const silent = `require("node:fs").writeFileSync(${JSON.stringify(pidFile)}, String(process.pid)); setInterval(() => {}, 1000)`
  // Stopped after the test whatever it saw, so a failure here hangs nothing.
  t.after(() => {
    const left = existsSync(pidFile) ? Number(readFileSync(pidFile, "utf8")) : null
    if (left !== null && alive(left)) process.kill(left, "SIGKILL")
    rmSync(pidDir, { recursive: true, force: true })
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
 * stopped is then the agent's doing. Returns its `session/new` entry,
 * `pid()`, `termed()` (whether it was sent SIGTERM, with `ignoresTerm`), and
 * `heirPid()` (with `heir`); both are killed after the test whatever happened,
 * and then the directory of their files is removed.
 */
function lingering(
  t,
  {
    answers = () => true,
    refuses = () => false,
    exits = () => false,
    delayMs = 0,
    deafAfterInitialize = false,
    ignoresTerm = false,
    heir = false,
    name = "lingering",
  } = {},
) {
  const pidDir = mkdtempSync(join(tmpdir(), "scripted-agent-"))
  const pidFile = join(pidDir, "pid")
  // `answers(method)`: whether it answers; `refuses(method)`: with an error;
  // `exits(method)`: by exiting instead; `delayMs`: after how long;
  // `deafAfterInitialize`: it stops reading its input once it has answered
  // initialize, but keeps running; `ignoresTerm`: SIGTERM does not stop it,
  // and writes `termed` beside the pid file; `heir`: it starts a process of
  // its own that holds its stdout open, ignores SIGTERM, writes its pid to
  // `heir` beside the pid file, and ends by itself after 30 s, so one whose
  // pid was never read cannot outlive the suite.
  const termedFile = join(dirname(pidFile), "termed")
  const heirFile = join(dirname(pidFile), "heir")
  const heirScript = `require("node:fs").writeFileSync(${JSON.stringify(heirFile)}, String(process.pid)); process.on("SIGTERM", () => {}); setTimeout(() => {}, 30000)`
  const script = `if (${heir}) require("node:child_process").spawn(process.execPath, ["-e", ${JSON.stringify(heirScript)}], { stdio: ["ignore", "inherit", "inherit"] })
if (${ignoresTerm}) process.on("SIGTERM", () => require("node:fs").writeFileSync(${JSON.stringify(termedFile)}, ""))
require("node:fs").writeFileSync(${JSON.stringify(pidFile)}, String(process.pid))
require("node:readline").createInterface({ input: process.stdin }).on("line", (line) => {
  const m = JSON.parse(line)
  if (m.id === undefined || !(${answers.toString()})(m.method)) return
  const answer = (${refuses.toString()})(m.method) ? { error: { code: -32000, message: "refused " + m.method } } : { result: {} }
  setTimeout(() => {
    if ((${exits.toString()})(m.method)) process.exit(1)
    process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: m.id, ...answer }) + "\\n")
    if (${deafAfterInitialize} && m.method === "initialize") {
      process.stdin.destroy()
      try { require("node:fs").closeSync(0) } catch {}
    }
  }, m.method === "initialize" ? 0 : ${delayMs})
})
setInterval(() => {}, 1000)`
  const pid = () => (existsSync(pidFile) ? Number(readFileSync(pidFile, "utf8")) : null)
  const termed = () => existsSync(termedFile)
  const heirPid = () =>
    existsSync(heirFile) ? Number(readFileSync(heirFile, "utf8")) : null
  t.after(() => {
    for (const p of [pid(), heirPid()])
      if (p !== null && alive(p)) process.kill(p, "SIGKILL")
    rmSync(pidDir, { recursive: true, force: true })
  })
  return { server: { ...mcptest, name, args: ["-e", script] }, pid, termed, heirPid }
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

test("closing the agent's input stops a stand-in still connecting", async (t) => {
  const silent = lingering(t, { answers: () => false })
  const agent = start("codex", codexEnv)
  agent.request("session/new", { cwd: here, mcpServers: [silent.server] }).catch(() => {})
  const end = Date.now() + 5000
  while (silent.pid() === null && Date.now() < end) await sleep(50)
  assert.notEqual(silent.pid(), null, "the stand-in never started")
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
  assert.equal(
    await gone(silent.pid()),
    true,
    "the connecting stand-in outlived its agent",
  )
})

test("a call its stand-in does not answer within the deadline fails the turn, and nothing is reported", async (t) => {
  const stand = lingering(t, {
    answers: (method) => method === "initialize",
    name: "mcptest",
  })
  const agent = start("codex", codexEnv)
  const opened = await agent.request("session/new", {
    cwd: here,
    mcpServers: [stand.server],
  })
  const turn = await agent.request("session/prompt", {
    sessionId: opened.result.sessionId,
    prompt: [],
  })
  assert.match(turn.error.message, /did not answer tools\/call within/)
  assert.equal(turn.notes.length, 0)
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("a call the recorded one cannot stand for fails the turn, and nothing is reported", async () => {
  const agent = start("codex", codexEnv, "always_fails")
  const opened = await agent.request("session/new", { cwd: here, mcpServers: [mcptest] })
  const turn = await agent.request("session/prompt", {
    sessionId: opened.result.sessionId,
    prompt: [],
  })
  assert.match(turn.error.message, /cannot replay: always_fails/)
  assert.equal(turn.notes.length, 0)
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("a cancel while the call is in flight ends the turn cancelled, and nothing is reported", async (t) => {
  const slow = lingering(t, { name: "mcptest", delayMs: 1500 })
  const agent = start("codex", codexEnv)
  const opened = await agent.request("session/new", {
    cwd: here,
    mcpServers: [slow.server],
  })
  const { sessionId } = opened.result
  const asked = Date.now()
  const turn = agent.request("session/prompt", { sessionId, prompt: [] })
  agent.notify("session/cancel", { sessionId })
  const answered = await turn
  // Answered once the stand-in's answer came, not at the cancel.
  assert.ok(Date.now() - asked >= 1400, "answered before the call settled")
  assert.deepEqual(answered.result, { stopReason: "cancelled" })
  assert.equal(answered.notes.length, 0)
  // The session goes on: a cancel with nothing in flight changes nothing.
  agent.notify("session/cancel", { sessionId })
  const unknown = await agent.request("session/set_config_option", {
    sessionId: "nope",
    configId: "mode",
    value: "agent",
  })
  assert.match(unknown.error.message, /no session nope/)
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("a cancel while the call is in flight ends the turn cancelled even when the call then fails", async (t) => {
  const refusing = lingering(t, {
    name: "mcptest",
    delayMs: 1500,
    refuses: (method) => method === "tools/call",
  })
  const agent = start("codex", codexEnv)
  const { sessionId } = (
    await agent.request("session/new", { cwd: here, mcpServers: [refusing.server] })
  ).result
  const asked = Date.now()
  const turn = agent.request("session/prompt", { sessionId, prompt: [] })
  agent.notify("session/cancel", { sessionId })
  const answered = await turn
  // Answered once the stand-in's refusal came, not at the cancel.
  assert.ok(Date.now() - asked >= 1400, "answered before the call settled")
  assert.equal(answered.error, undefined, answered.error?.message)
  assert.deepEqual(answered.result, { stopReason: "cancelled" })
  assert.equal(answered.notes.length, 0)
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("a cancel while the call is in flight ends the turn cancelled even when the call's deadline passes", async (t) => {
  const silent = lingering(t, {
    answers: (method) => method === "initialize",
    name: "mcptest",
  })
  const agent = start("codex", codexEnv)
  const { sessionId } = (
    await agent.request("session/new", { cwd: here, mcpServers: [silent.server] })
  ).result
  // The request's own 20 s outlasts the call's 10 s deadline.
  const asked = Date.now()
  const turn = agent.request("session/prompt", { sessionId, prompt: [] })
  agent.notify("session/cancel", { sessionId })
  const answered = await turn
  // Answered once the call's 10 s deadline passed, not at the cancel.
  assert.ok(Date.now() - asked >= 9500, "answered before the call settled")
  assert.equal(answered.error, undefined, answered.error?.message)
  assert.deepEqual(answered.result, { stopReason: "cancelled" })
  assert.equal(answered.notes.length, 0)
  // The stand-in is kept, still silent: the next prompt's call meets its own
  // deadline, with no cancel to decide it.
  const next = await agent.request("session/prompt", { sessionId, prompt: [] })
  assert.match(next.error.message, /did not answer tools\/call within/)
  assert.equal(next.notes.length, 0)
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("a cancel while the call is in flight ends the turn cancelled even when the stand-in exits", async (t) => {
  const exiting = lingering(t, {
    name: "mcptest",
    delayMs: 1500,
    exits: (method) => method === "tools/call",
  })
  const agent = start("codex", codexEnv)
  const { sessionId } = (
    await agent.request("session/new", { cwd: here, mcpServers: [exiting.server] })
  ).result
  const asked = Date.now()
  const turn = agent.request("session/prompt", { sessionId, prompt: [] })
  agent.notify("session/cancel", { sessionId })
  const answered = await turn
  // Answered once the stand-in exited, not at the cancel.
  assert.ok(Date.now() - asked >= 1400, "answered before the call settled")
  assert.equal(answered.error, undefined, answered.error?.message)
  assert.deepEqual(answered.result, { stopReason: "cancelled" })
  assert.equal(answered.notes.length, 0)
  assert.equal(await gone(exiting.pid()), true, "the stand-in did not exit")
  // The session's stand-in is gone: the next prompt fails at once.
  const next = await agent.request("session/prompt", { sessionId, prompt: [] })
  assert.match(next.error.message, /the stand-in exited \(1\)/)
  assert.equal(next.notes.length, 0)
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("a cancel for another session leaves the prompt in flight alone", async (t) => {
  const refusing = lingering(t, {
    name: "mcptest",
    delayMs: 1500,
    refuses: (method) => method === "tools/call",
  })
  const agent = start("codex", codexEnv)
  const busy = (
    await agent.request("session/new", { cwd: here, mcpServers: [refusing.server] })
  ).result.sessionId
  const idle = (await agent.request("session/new", { cwd: here, mcpServers: [mcptest] }))
    .result.sessionId
  const turn = agent.request("session/prompt", { sessionId: busy, prompt: [] })
  agent.notify("session/cancel", { sessionId: idle })
  // Not cancelled: the call's own failure is the turn's answer.
  assert.match((await turn).error.message, /refused tools\/call/)
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("a cancel with nothing in flight leaves the session's next prompt to end its turn", async () => {
  const agent = start("codex", codexEnv)
  const { sessionId } = (
    await agent.request("session/new", { cwd: here, mcpServers: [mcptest] })
  ).result
  agent.notify("session/cancel", { sessionId })
  const turn = await agent.request("session/prompt", { sessionId, prompt: [] })
  assert.deepEqual(turn.result, { stopReason: "end_turn" })
  assert.equal(turn.notes.at(-1).params.update.content.text, "DONE")
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("an agent whose output fails stops its stand-ins and exits 0", async (t) => {
  const stand = lingering(t)
  const agent = start("codex", codexEnv)
  const opened = await agent.request("session/new", {
    cwd: here,
    mcpServers: [stand.server],
  })
  assert.ok(opened.result.sessionId)
  // The gateway stops reading; the agent's next answer has nowhere to go.
  agent.child.stdout.destroy()
  agent.request("initialize", { protocolVersion: 1 }).catch(() => {})
  assert.equal(await exited(agent.child, 5000), true)
  assert.equal(agent.child.exitCode, 0)
  assert.equal(await gone(stand.pid()), true, "the stand-in outlived its agent")
})

for (const [trigger, close] of [
  ["input closes", (agent) => agent.child.stdin.end()],
  [
    "output fails",
    (agent) => {
      agent.child.stdout.destroy()
      agent.request("initialize", { protocolVersion: 1 }).catch(() => {})
    },
  ],
])
  test(`an agent whose ${trigger} kills a stand-in that ignores SIGTERM, and exits 0 once it is gone`, async (t) => {
    const stubborn = lingering(t, { ignoresTerm: true })
    const agent = start("codex", codexEnv)
    const opened = await agent.request("session/new", {
      cwd: here,
      mcpServers: [stubborn.server],
    })
    assert.ok(opened.result.sessionId)
    const stopped = Date.now()
    close(agent)
    assert.equal(await exited(agent.child, STOP_GRACE_MS + 3000), true)
    // At once, not polled: the agent waited for the stand-in before exiting.
    assert.equal(alive(stubborn.pid()), false, "the stand-in outlived its agent")
    assert.equal(agent.child.exitCode, 0)
    // Asked first: the SIGKILL came only after a SIGTERM and its grace.
    assert.equal(stubborn.termed(), true, "killed without a SIGTERM")
    assert.ok(Date.now() - stopped >= STOP_GRACE_MS - 100, "killed before its grace")
  })

test("a stand-in whose own process holds its output open does not keep the agent from exiting", async (t) => {
  const stubborn = lingering(t, { ignoresTerm: true, heir: true })
  const agent = start("codex", codexEnv)
  const opened = await agent.request("session/new", {
    cwd: here,
    mcpServers: [stubborn.server],
  })
  assert.ok(opened.result.sessionId)
  const end = Date.now() + 5000
  while (stubborn.heirPid() === null && Date.now() < end) await sleep(50)
  assert.notEqual(stubborn.heirPid(), null, "the heir never started")
  agent.child.stdin.end()
  // The stand-in is reaped once killed, though its pipe stays open in the heir.
  assert.equal(await exited(agent.child, STOP_GRACE_MS + 3000), true)
  assert.equal(agent.child.exitCode, 0)
  assert.equal(alive(stubborn.pid()), false, "the stand-in outlived its agent")
  assert.equal(alive(stubborn.heirPid()), true, "the heir did not hold the pipe")
})

/**
 * Only that nothing starts is asserted: the row's failed `session/new` is
 * answered on the output whose failure began the stop, so it cannot be read.
 */
test("a session/new during the stop's grace starts no stand-in", async (t) => {
  const stubborn = lingering(t, { ignoresTerm: true })
  const late = lingering(t, { name: "late" })
  const agent = start("codex", codexEnv)
  const opened = await agent.request("session/new", {
    cwd: here,
    mcpServers: [stubborn.server],
  })
  assert.ok(opened.result.sessionId)
  // The output fails and the input stays open: the stop holds its grace for
  // the stand-in that ignores SIGTERM, and the gateway can still ask.
  agent.child.stdout.destroy()
  agent.request("initialize", { protocolVersion: 1 }).catch(() => {})
  // Its SIGTERM is the stop's outward sign: only then is the late one asked for.
  const end = Date.now() + 5000
  while (!stubborn.termed() && Date.now() < end) await sleep(50)
  assert.equal(stubborn.termed(), true, "the stop never began")
  agent.request("session/new", { cwd: here, mcpServers: [late.server] }).catch(() => {})
  assert.equal(await exited(agent.child, STOP_GRACE_MS + 3000), true)
  assert.equal(agent.child.exitCode, 0)
  assert.equal(alive(stubborn.pid()), false, "the stand-in outlived its agent")
  assert.equal(late.pid(), null, "a stand-in started after the stop began")
})

test("a second prompt while one is in flight is refused", async (t) => {
  const slow = lingering(t, { name: "mcptest", delayMs: 1500 })
  const agent = start("codex", codexEnv)
  const { sessionId } = (
    await agent.request("session/new", { cwd: here, mcpServers: [slow.server] })
  ).result
  const first = agent.request("session/prompt", { sessionId, prompt: [] })
  const second = await agent.request("session/prompt", { sessionId, prompt: [] })
  assert.match(second.error.message, /already in a prompt/)
  // The stand-in answers {}, which the recorded call cannot stand for.
  assert.match((await first).error.message, /cannot replay/)
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("a stand-in that stops reading fails the call, not the agent, and is stopped on close", async (t) => {
  const deaf = lingering(t, { name: "mcptest", deafAfterInitialize: true })
  const agent = start("codex", codexEnv)
  const opened = await agent.request("session/new", {
    cwd: here,
    mcpServers: [deaf.server],
  })
  await sleep(300)
  const turn = await agent.request("session/prompt", {
    sessionId: opened.result.sessionId,
    prompt: [],
  })
  assert.match(turn.error.message, /the stand-in's input closed/)
  assert.equal(turn.notes.length, 0)
  assert.equal(agent.child.exitCode, null, "the agent died")
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
  assert.equal(agent.child.exitCode, 0)
  assert.equal(await gone(deaf.pid()), true, "the stand-in outlived its agent")
})

test("a stand-in that refuses initialize fails session/new, and is stopped", async (t) => {
  const refusing = lingering(t, { refuses: (method) => method === "initialize" })
  const agent = start("codex", codexEnv)
  const opened = await agent.request("session/new", {
    cwd: here,
    mcpServers: [refusing.server],
  })
  assert.match(opened.error.message, /refused initialize/)
  assert.equal(await gone(refusing.pid()), true)
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("two MCP servers of one name are refused before either starts", async () => {
  const agent = start("codex", codexEnv)
  const opened = await agent.request("session/new", {
    cwd: here,
    mcpServers: [mcptest, { ...mcptest, command: join(here, "no-such-command") }],
  })
  assert.equal(opened.error.message, "two MCP servers named mcptest")
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})

test("two MCP servers with no name are refused as alike", async () => {
  const agent = start("codex", codexEnv)
  const { name: _, ...nameless } = mcptest
  const opened = await agent.request("session/new", {
    cwd: here,
    mcpServers: [nameless, { ...nameless, command: join(here, "no-such-command") }],
  })
  assert.equal(opened.error.message, "two MCP servers with no name")
  const blank = { ...mcptest, name: "" }
  const blanks = await agent.request("session/new", {
    cwd: here,
    mcpServers: [blank, { ...blank, command: join(here, "no-such-command") }],
  })
  assert.equal(blanks.error.message, "two MCP servers with no name")
  agent.child.stdin.end()
  assert.equal(await exited(agent.child, 5000), true)
})
