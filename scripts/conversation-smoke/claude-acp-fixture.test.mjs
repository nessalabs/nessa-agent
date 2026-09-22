import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { once } from "node:events"
import { mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { createInterface } from "node:readline"
import { test } from "node:test"
import { fileURLToPath } from "node:url"
import { collectCleanupFailures, createFixtureSupervisor } from "./runtime.mjs"

const fixture = join(dirname(fileURLToPath(import.meta.url)), "claude-acp-fixture.mjs")

function bounded(promise, description, timeoutMs = 1_000) {
  return Promise.race([
    promise,
    new Promise((_, reject) => {
      const timer = setTimeout(
        () => reject(new Error(`timed out waiting for ${description}`)),
        timeoutMs,
      )
      promise.finally(() => clearTimeout(timer)).catch(() => {})
    }),
  ])
}

async function stopChild(child) {
  if (child.exitCode !== null || child.signalCode !== null) return
  child.stdin.destroy()
  try {
    await bounded(once(child, "exit"), "fixture EOF exit", 200)
    return
  } catch {
    child.kill("SIGTERM")
  }
  try {
    await bounded(once(child, "exit"), "fixture TERM exit", 500)
    return
  } catch {
    child.kill("SIGKILL")
  }
  await bounded(once(child, "exit"), "fixture KILL exit", 500)
}

async function createHarness() {
  const directory = mkdtempSync(join(tmpdir(), "nessa-conversation-fixture-"))
  const supervisor = await createFixtureSupervisor()
  const children = []
  const launch = () => {
    const child = spawn(
      process.execPath,
      [
        fixture,
        "evidence.jsonl",
        "provider-state.json",
        String(supervisor.port),
        supervisor.nonce,
      ],
      {
        cwd: directory,
        env: {
          ...process.env,
          ANTHROPIC_MODEL: "claude-haiku-4-5-20251001",
          ANTHROPIC_CUSTOM_MODEL_OPTION: "claude-haiku-4-5-20251001",
        },
        stdio: ["pipe", "pipe", "pipe"],
      },
    )
    children.push(child)
    const output = createInterface({ input: child.stdout, crlfDelay: Infinity })[
      Symbol.asyncIterator
    ]()
    const send = (value) => child.stdin.write(`${JSON.stringify(value)}\n`)
    const next = async () => {
      const item = await bounded(output.next(), "fixture response")
      assert.equal(item.done, false, "fixture stdout ended before its response")
      return JSON.parse(item.value)
    }
    return { child, send, next }
  }
  const cleanup = async () => {
    const processFailures = await collectCleanupFailures([
      {
        name: "fixture supervisor",
        run: () => supervisor.shutdown({ processTimeoutMs: 500, serverTimeoutMs: 500 }),
      },
      ...children.map((child, index) => ({
        name: `fixture child ${index + 1}`,
        run: () => stopChild(child),
      })),
    ])
    if (processFailures.length > 0) return processFailures
    return collectCleanupFailures([
      {
        name: "fixture directory",
        run: () => rmSync(directory, { recursive: true, force: true }),
      },
    ])
  }
  return { children, directory, launch, cleanup }
}

async function withHarness(run) {
  const harness = await createHarness()
  let result
  let primaryFailure
  try {
    result = await run(harness)
  } catch (error) {
    primaryFailure = error
  }
  const cleanupFailures = await harness.cleanup()
  if (primaryFailure && cleanupFailures.length === 0) throw primaryFailure
  if (primaryFailure || cleanupFailures.length > 0)
    throw new AggregateError(
      [...(primaryFailure ? [primaryFailure] : []), ...cleanupFailures],
      "fixture harness failed",
    )
  return result
}

test("Claude fixture records real prompt, permission, terminal, and resume messages", async () => {
  await withHarness(async ({ directory, launch }) => {
    const first = launch()
    first.send({ id: 1, method: "initialize", params: {} })
    assert.equal((await first.next()).result.agentInfo.version, "0.76.0")
    first.send({ id: 2, method: "session/new", params: { cwd: directory } })
    const opened = await first.next()
    const providerSessionId = opened.result.sessionId
    first.send({
      id: 3,
      method: "session/set_config_option",
      params: { sessionId: providerSessionId, configId: "mode", value: "default" },
    })
    assert.equal((await first.next()).id, 3)
    first.send({
      id: 4,
      method: "session/prompt",
      params: {
        sessionId: providerSessionId,
        prompt: [
          { type: "text", text: "fixtureCorrelation:execution-answer" },
          { type: "image", data: "fixture" },
        ],
      },
    })
    assert.equal((await first.next()).method, "session/update")
    const tool = await first.next()
    assert.equal(tool.params.update.sessionUpdate, "tool_call")
    assert.equal(tool.params.update.toolCallId, "tool-execution-answer")
    assert.equal(tool.params.update.title, "Read fixture-input")
    // ClaudeProfile::permission_input parses Read through the production
    // file-tool schema, whose required field is an absolute `file_path`.
    assert.deepEqual(tool.params.update.rawInput, {
      file_path: join(directory, "fixture-input"),
    })
    const permission = await first.next()
    assert.equal(permission.id, "review-execution-answer")
    assert.equal(permission.params.toolCall.toolCallId, tool.params.update.toolCallId)
    assert.deepEqual(permission.params.toolCall.rawInput, tool.params.update.rawInput)
    const providerOptions = [
      { optionId: "allow-once", kind: "allow_once", name: "Allow once" },
      { optionId: "deny-once", kind: "reject_once", name: "Deny once" },
    ]
    assert.deepEqual(permission.params.options, providerOptions)
    first.send({
      id: permission.id,
      result: { outcome: { outcome: "selected", optionId: "allow-once" } },
    })
    assert.equal((await first.next()).params.update.status, "completed")
    assert.deepEqual((await first.next()).result, { stopReason: "end_turn" })
    first.child.stdin.end()
    assert.equal((await bounded(once(first.child, "exit"), "first fixture exit"))[0], 0)

    const resumed = launch()
    resumed.send({ id: 5, method: "initialize", params: {} })
    await resumed.next()
    resumed.send({
      id: 6,
      method: "session/resume",
      params: { sessionId: providerSessionId },
    })
    assert.equal((await resumed.next()).id, 6)
    resumed.child.stdin.end()
    assert.equal(
      (await bounded(once(resumed.child, "exit"), "resumed fixture exit"))[0],
      0,
    )

    const invalid = launch()
    invalid.send({ id: 7, method: "initialize", params: {} })
    await invalid.next()
    invalid.send({
      id: 8,
      method: "session/resume",
      params: { sessionId: "session-never-created" },
    })
    assert.notEqual(
      (await bounded(once(invalid.child, "exit"), "invalid fixture exit"))[0],
      0,
    )

    const events = readFileSync(join(directory, "evidence.jsonl"), "utf8")
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line))
    assert.equal(
      events.filter(
        (event) =>
          event.type === "prompt" &&
          event.providerSessionId === providerSessionId &&
          event.expectedExecutionId === "execution-answer",
      ).length,
      1,
    )
    assert.equal(
      events.find(
        (event) =>
          event.type === "session-resume" &&
          event.providerSessionId === providerSessionId,
      )?.sessionWorkspace,
      directory,
    )
    assert.deepEqual(
      events.find(
        (event) =>
          event.type === "permission-request" &&
          event.providerSessionId === providerSessionId,
      ),
      {
        type: "permission-request",
        providerSessionId,
        expectedExecutionId: "execution-answer",
        permissionRequestId: permission.id,
        toolCallId: tool.params.update.toolCallId,
        toolName: "Read",
        rawInput: { file_path: join(directory, "fixture-input") },
        options: providerOptions,
      },
    )
    assert.deepEqual(
      events.find(
        (event) =>
          event.type === "permission-answer" &&
          event.providerSessionId === providerSessionId,
      ),
      {
        type: "permission-answer",
        providerSessionId,
        expectedExecutionId: "execution-answer",
        permissionRequestId: permission.id,
        toolCallId: tool.params.update.toolCallId,
        optionId: "allow-once",
      },
    )
    assert.equal(
      events.filter(
        (event) =>
          event.type === "session-resume" &&
          event.providerSessionId === providerSessionId,
      ).length,
      1,
    )
  })
})

test("fixture harness reaps its child after a failed assertion", async () => {
  let child
  await assert.rejects(
    withHarness(async ({ launch }) => {
      const fixtureProcess = launch()
      child = fixtureProcess.child
      fixtureProcess.send({ id: 1, method: "initialize", params: {} })
      assert.equal((await fixtureProcess.next()).result.agentInfo.version, "mutated")
    }),
    assert.AssertionError,
  )
  assert.ok(child.exitCode !== null || child.signalCode !== null)
})
