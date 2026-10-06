import { strict as assert } from "node:assert"
import { execFile } from "node:child_process"
import { once } from "node:events"
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import { fileURLToPath } from "node:url"
import { captureProbe } from "./capture.mjs"
import { boundedFrames } from "./acp-session.mjs"
import { providerVersion, startProbeProcess, stopProbeProcess } from "./processes.mjs"

const fixture = fileURLToPath(new URL("./fixtures/probe-agent.mjs", import.meta.url))
const posix = { skip: process.platform === "win32", timeout: 8000 }
const cleanupOptions = { graceMs: 100, confirmMs: 2000 }
function directory(t) {
  const path = mkdtempSync(join(tmpdir(), "nessa-probe-test-"))
  t.after(() => rmSync(path, { recursive: true, force: true }))
  return path
}
async function probe(t, scenario, extra = {}) {
  const parent = directory(t)
  const result = await captureProbe({
    args: [fixture, scenario],
    source: { kind: "scripted-test-fixture" },
    expectedAgent: { name: "scripted-fixture", version: "1" },
    workspaceParent: parent,
    versionCommand: process.execPath,
    versionArgs: [fixture, "version-ok"],
    cleanupOptions,
    requestBudgetMs: 1000,
    promptBudgetMs: 1000,
    ...extra,
  })
  assert.deepEqual(result.source.probeCleanup, {
    kind: "stopped",
    leaderReaped: true,
    liveGroupMembers: 0,
  })
  assert.deepEqual(readdirSync(parent), [])
  assert.doesNotMatch(JSON.stringify(result), /SECRET_ACCOUNT|SECRET_TOKEN/)
  return result
}

for (const scenario of [
  "invalid-null",
  "invalid-array",
  "invalid-json",
  "invalid-envelope",
]) {
  test(
    `${scenario} fails outstanding ACP work and confirms subprocess cleanup`,
    posix,
    async (t) => {
      const result = await probe(t, scenario)
      assert.deepEqual(result.outcome, { kind: "failed", code: "invalid_frame" })
      assert.deepEqual(result.frames, [])
    },
  )
}
for (const scenario of ["oversized-line", "oversized-unterminated"]) {
  test(
    `${scenario} is bounded before parse and still cleans the subprocess`,
    posix,
    async (t) => {
      const result = await probe(t, scenario, { maxBytes: 4096 })
      assert.deepEqual(result.outcome, { kind: "failed", code: "recording_limit" })
      assert.deepEqual(result.frames, [])
    },
  )
}

test("chunk framing refuses oversize before parsing and supervises handler exceptions", () => {
  let calls = 0
  const failures = []
  const reader = boundedFrames(
    () => {
      calls++
      throw new Error("SECRET_HANDLER")
    },
    (error) => failures.push(error.code),
    { maxBytes: 20 },
  )
  reader.take(Buffer.from('{"jsonrpc":"2.0","id":1,"result":{}}\n'))
  reader.end()
  assert.equal(calls, 0)
  assert.deepEqual(failures, ["recording_limit"])
  const handler = boundedFrames(
    () => {
      throw new Error("SECRET_HANDLER")
    },
    (error) => failures.push(error.code),
  )
  handler.take(Buffer.from('{"jsonrpc":"2.0","id":1,"result":{}}\n'))
  handler.end()
  assert.deepEqual(failures, ["recording_limit", "frame_handler_failed"])
})

test("setup and spawn failure use the same workspace cleanup path", posix, async (t) => {
  const parent = directory(t)
  const setup = await captureProbe({
    workspaceParent: join(parent, "absent"),
    source: { kind: "scripted-test-fixture" },
    expectedAgent: { name: "scripted-fixture", version: "1" },
  })
  assert.equal(setup.outcome.kind, "failed")
  assert.deepEqual(readdirSync(parent), [])
  const spawn = await probe(t, "unused", {
    command: join(parent, "missing-binary"),
    args: [],
  })
  assert.equal(spawn.outcome.code, "spawn_failed")
})

test(
  "controlled success waits for cleanup; cancelled/error child remains unsuccessful with selected evidence",
  posix,
  async (t) => {
    const valid = await probe(t, "complete")
    assert.equal(valid.outcome.kind, "completed")
    assert.equal(valid.outcome.stopReason, "end_turn")
    for (const [scenario, code] of [
      ["cancelled", "unexpected_terminal"],
      ["child-error", "child_not_completed"],
    ]) {
      const result = await probe(t, scenario)
      assert.deepEqual(result.outcome, { kind: "failed", code })
      assert.equal(result.frames.length, 7)
    }
  },
)

test(
  "a stalled version binary is bounded, killed and reaped; ACP cleanup still completes",
  posix,
  async (t) => {
    const parent = directory(t)
    const pidFile = join(parent, "version.pid")
    t.after(() => {
      // Emergency release also runs when an adversarial mutation breaks cleanup.
      if (existsSync(pidFile)) {
        try {
          process.kill(-Number(readFileSync(pidFile, "utf8")), "SIGKILL")
        } catch {}
      }
    })
    const started = Date.now()
    const result = await probe(t, "complete", {
      versionArgs: [fixture, "version-stall", pidFile],
      versionOptions: { budgetMs: 200, ...cleanupOptions },
    })
    assert.equal(result.outcome.kind, "completed")
    assert.equal(result.source.providerVersion, "unavailable")
    const pid = Number(readFileSync(pidFile, "utf8"))
    assert.throws(
      () => process.kill(pid, 0),
      (error) => error.code === "ESRCH",
    )
    assert.ok(Date.now() - started < 3000)
    // The isolated helper also settles unavailable on a missing version command.
    assert.equal(
      (
        await providerVersion(join(parent, "missing-version"), [], {
          budgetMs: 200,
          ...cleanupOptions,
        })
      ).value,
      "unavailable",
    )
  },
)

function state(pid) {
  return new Promise((resolve, reject) =>
    execFile(
      "/bin/ps",
      ["-o", "stat=", "-p", String(pid)],
      { timeout: 1000, killSignal: "SIGKILL" },
      (error, stdout) => {
        if (error && error.code !== 1) return reject(error)
        resolve(stdout.trim())
      },
    ),
  )
}
test(
  "leader-first exit cannot cancel escalation for a SIGTERM-ignoring descendant with independent stdio",
  posix,
  async (t) => {
    const owned = startProbeProcess(process.execPath, [fixture, "leader-first"])
    t.after(async () => {
      // Keep the test from leaking its deliberately TERM-ignoring process if
      // an adversarial mutation falsely releases the owning group.
      try {
        process.kill(-owned.child.pid, "SIGKILL")
      } catch {}
      await stopProbeProcess(owned, cleanupOptions)
    })
    const [bytes] = await once(owned.child.stdout, "data")
    const { descendant } = JSON.parse(bytes.toString("utf8"))
    const reaped = once(owned.child, "close")
    const cleanup = stopProbeProcess(owned, { graceMs: 200, confirmMs: 2000 })
    await reaped
    const result = await cleanup
    assert.deepEqual(result, { kind: "stopped", leaderReaped: true, liveGroupMembers: 0 })
    const after = await state(descendant)
    assert.ok(
      after === "" || after.startsWith("Z"),
      "descendant has exited; orphan zombies are not running",
    )
  },
)

for (const [scenario, code] of [
  ["foreign-session", "admitted_session_mismatch"],
  ["forged-terminal", "unsolicited_response"],
  ["forged-terminal-only", "unsolicited_response"],
  ["empty-terminal", "prompt_invalid"],
  ["startup-native", "premature_activity"],
  ["open-before-prompt-native", "premature_activity"],
  ["terminal-trailing-native", "activity_after_terminal"],
  ["close-null", "invalid_frame"],
  ["close-partial", "incomplete_frame_at_seal"],
  ["close-rejected", "close_rejected"],
  ["close-rejected-null", "close_rejected"],
]) {
  test(
    `${scenario} cannot replace capture admission or erase a recording failure`,
    posix,
    async (t) => {
      const result = await probe(t, scenario)
      assert.equal(result.outcome.kind, "failed")
      assert.equal(result.outcome.code, code)
      assert.deepEqual(result.frames, [])
      assert.doesNotMatch(JSON.stringify(result), /SECRET_/)
    },
  )
}

test(
  "sealing publishes separately correlated opening/prompt/close and discards later output during slow metadata",
  posix,
  async (t) => {
    const parent = directory(t)
    const result = await probe(t, "late-null", {
      versionArgs: [fixture, "version-stall", join(parent, "late-version.pid")],
      versionOptions: { budgetMs: 200, ...cleanupOptions },
    })
    assert.equal(result.outcome.kind, "completed")
    assert.deepEqual(result.admission, {
      opening: { requestId: 2, responseId: 2, sessionId: "identity-1" },
      prompt: {
        requestId: 3,
        responseId: 3,
        sessionId: "identity-1",
        stopReason: "end_turn",
      },
      close: { requestId: 4, responseId: 4, sessionId: "identity-1", acknowledged: true },
    })
    assert.equal(result.frames.length, 7)
    assert.equal(result.source.providerVersion, "unavailable")
    assert.doesNotMatch(JSON.stringify(result), /SECRET_/)
  },
)

test(
  "failed close remains the primary failure when physical cleanup cannot be confirmed",
  posix,
  async (t) => {
    const parent = directory(t)
    const result = await captureProbe({
      args: [fixture, "close-rejected"],
      source: { kind: "scripted-test-fixture" },
      expectedAgent: { name: "scripted-fixture", version: "1" },
      workspaceParent: parent,
      requestBudgetMs: 1000,
      promptBudgetMs: 1000,
      cleanupOptions: { graceMs: 0, confirmMs: 0 },
    })
    assert.equal(result.outcome.code, "close_rejected")
    assert.equal(result.outcome.cleanupCode, "cleanup_unconfirmed")
    assert.equal(result.source.probeCleanup.kind, "unconfirmed")
    assert.equal(
      readdirSync(parent).length,
      1,
      "unconfirmed cleanup retains workspace ownership",
    )
  },
)

test("sealing refuses unfinished bytes already delivered after the close boundary", () => {
  const failures = []
  const reader = boundedFrames(
    () => {},
    (error) => failures.push(error.code),
  )
  reader.take(Buffer.from('{"jsonrpc":"2.0","id":4,"result":{}}\nnull'))
  reader.complete()
  assert.deepEqual(failures, ["incomplete_frame_at_seal"])
})

test(
  "provider metadata is projected without account, label or nested credential content",
  posix,
  async (t) => {
    const result = await probe(t, "metadata-secrets")
    assert.equal(result.outcome.kind, "completed")
    assert.deepEqual(result.source.agentInfo, { name: "scripted-fixture", version: "1" })
    assert.deepEqual(result.source.sessionCapabilities, { close: {} })
    assert.deepEqual(result.source.modeConfig, {
      id: "mode",
      currentValue: "read-only",
      options: [{ value: "read-only", _meta: { kind: "standard" } }],
    })
    assert.doesNotMatch(JSON.stringify(result), /SECRET_/)
  },
)

test(
  "unadmitted provider identity or mode fails before prompt without exposing the value",
  posix,
  async (t) => {
    for (const [scenario, code] of [
      ["metadata-invalid-agent", "metadata_agent_mismatch"],
      ["metadata-invalid-mode", "metadata_mode_invalid"],
    ]) {
      const parent = directory(t)
      const marker = join(parent, "prompt.marker")
      const result = await probe(t, scenario, { args: [fixture, scenario, marker] })
      assert.equal(result.outcome.kind, "failed")
      assert.equal(result.outcome.code, code)
      assert.equal(existsSync(marker), false)
      assert.doesNotMatch(JSON.stringify(result), /SECRET_/)
    }
  },
)
