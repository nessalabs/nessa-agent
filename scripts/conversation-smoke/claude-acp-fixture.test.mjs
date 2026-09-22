import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { once } from "node:events"
import { mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { createInterface } from "node:readline"
import { test } from "node:test"
import { fileURLToPath } from "node:url"

const fixture = join(dirname(fileURLToPath(import.meta.url)), "claude-acp-fixture.mjs")

function launch(directory) {
  const child = spawn(
    process.execPath,
    [fixture, "evidence.jsonl", "provider-state.json"],
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
  const output = createInterface({ input: child.stdout, crlfDelay: Infinity })[
    Symbol.asyncIterator
  ]()
  const send = (value) => child.stdin.write(`${JSON.stringify(value)}\n`)
  const next = async () => JSON.parse((await output.next()).value)
  return { child, send, next }
}

test("Claude fixture records real prompt, permission, terminal, and resume messages", async () => {
  const directory = mkdtempSync(join(tmpdir(), "nessa-conversation-fixture-"))
  try {
    const first = launch(directory)
    first.send({ id: 1, method: "initialize", params: {} })
    assert.equal((await first.next()).result.agentInfo.version, "0.76.0")
    first.send({ id: 2, method: "session/new", params: {} })
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
    assert.equal((await first.next()).params.update.sessionUpdate, "tool_call")
    const permission = await first.next()
    assert.equal(permission.id, "review-execution-answer")
    first.send({
      id: permission.id,
      result: { outcome: { outcome: "selected", optionId: "allow-once" } },
    })
    assert.equal((await first.next()).params.update.status, "completed")
    assert.deepEqual((await first.next()).result, { stopReason: "end_turn" })
    first.child.stdin.end()
    assert.equal((await once(first.child, "exit"))[0], 0)

    const resumed = launch(directory)
    resumed.send({ id: 5, method: "initialize", params: {} })
    await resumed.next()
    resumed.send({
      id: 6,
      method: "session/resume",
      params: { sessionId: providerSessionId },
    })
    assert.equal((await resumed.next()).id, 6)
    resumed.child.stdin.end()
    assert.equal((await once(resumed.child, "exit"))[0], 0)

    const invalid = launch(directory)
    invalid.send({ id: 7, method: "initialize", params: {} })
    await invalid.next()
    invalid.send({
      id: 8,
      method: "session/resume",
      params: { sessionId: "session-never-created" },
    })
    assert.notEqual((await once(invalid.child, "exit"))[0], 0)

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
      events.filter(
        (event) =>
          event.type === "session-resume" &&
          event.providerSessionId === providerSessionId,
      ).length,
      1,
    )
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})
