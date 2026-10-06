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
