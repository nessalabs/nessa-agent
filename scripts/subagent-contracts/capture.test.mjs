import { strict as assert } from "node:assert"
import { execFile, spawn } from "node:child_process"
import { once } from "node:events"
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import { setTimeout as sleep } from "node:timers/promises"
import { fileURLToPath } from "node:url"
import { captureProbe, parseManualArgs } from "./capture.mjs"
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
  ["close-eof", "incomplete_frame_at_seal"],
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
    assert.equal(result.outcome.rpcCode, -32000)
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

test("EOF cannot turn an unterminated JSON fragment into an admitted frame", () => {
  const frames = []
  const failures = []
  const reader = boundedFrames(
    (frame) => frames.push(frame),
    (error) => failures.push(error.code),
  )
  reader.take(Buffer.from('{"jsonrpc":"2.0","id":4,'))
  reader.take(Buffer.from('"result":{}}'))
  reader.end()
  reader.complete()
  assert.deepEqual(frames, [])
  assert.deepEqual(failures, ["incomplete_frame_at_seal"])
  const valid = boundedFrames(
    (frame) => frames.push(frame),
    (error) => failures.push(error.code),
  )
  valid.take(Buffer.from('{"jsonrpc":"2.0","id":4,"result":{}}\n  '))
  valid.end()
  assert.equal(frames.length, 1)
  assert.deepEqual(failures, ["incomplete_frame_at_seal"])
})

const captureEntry = fileURLToPath(new URL("./capture.mjs", import.meta.url))
const manual = { skip: process.platform === "win32", timeout: 15000 }

test("manual arguments accept the live command or one controlled provider", () => {
  assert.deepEqual(parseManualArgs(["out.json"]), { kind: "live", out: "out.json" })
  const controlled = parseManualArgs([
    "out.json",
    "--controlled",
    "probe.mjs",
    "hold-prompt",
    "marker",
    "workspaces",
    "--grace",
    "0",
    "--confirm",
    "0",
    "--version-scenario",
    "version-stall",
    "--version-budget",
    "3000",
  ])
  assert.equal(controlled.kind, "controlled")
  assert.equal(controlled.out, "out.json")
  assert.equal(controlled.provider, "probe.mjs")
  assert.equal(controlled.scenario, "hold-prompt")
  assert.deepEqual(controlled.cleanupOptions, { graceMs: 0, confirmMs: 0 })
  assert.equal(controlled.versionScenario, "version-stall")
  assert.equal(controlled.versionBudgetMs, 3000)
  for (const argv of [
    [],
    ["--controlled"],
    ["out.json", "--controlled", "probe.mjs", "hold-prompt", "marker"],
    [
      "out.json",
      "--controlled",
      "probe.mjs",
      "hold-prompt",
      "marker",
      "workspaces",
      "--grace",
      "1",
    ],
    [
      "out.json",
      "--controlled",
      "probe.mjs",
      "hold-prompt",
      "marker",
      "workspaces",
      "--grace",
      "-1",
      "--confirm",
      "1",
    ],
    ["out.json", "--nope"],
    ["out.json", "--controlled", "probe.mjs", "BAD", "marker", "workspaces"],
  ])
    assert.equal(parseManualArgs(argv).kind, "invalid", JSON.stringify(argv))
})

function manualPaths(t) {
  const root = directory(t)
  const workspaceParent = join(root, "workspaces")
  mkdirSync(workspaceParent)
  return {
    workspaceParent,
    marker: join(root, "marker"),
    out: join(root, "out.json"),
  }
}
function controlledArgs(paths, scenario, extra = []) {
  return [
    paths.out,
    "--controlled",
    fixture,
    scenario,
    paths.marker,
    paths.workspaceParent,
    ...extra,
  ]
}
function spawnManual(args) {
  const child = spawn(process.execPath, [captureEntry, ...args], {
    stdio: ["ignore", "pipe", "pipe"],
  })
  let stdout = ""
  let stderr = ""
  child.stdout.setEncoding("utf8")
  child.stderr.setEncoding("utf8")
  child.stdout.on("data", (chunk) => {
    stdout += chunk
  })
  child.stderr.on("data", (chunk) => {
    stderr += chunk
  })
  const exited = once(child, "close").then(([code, signal]) => ({
    code,
    signal,
    stdout,
    stderr,
  }))
  return { child, exited }
}
function commandLines(needle) {
  return new Promise((resolve, reject) => {
    execFile(
      "/bin/ps",
      ["-eo", "pid=,stat=,command="],
      { timeout: 1000, killSignal: "SIGKILL" },
      (error, stdout) => {
        if (error) return reject(error)
        resolve(
          stdout
            .split("\n")
            .map((line) => line.trim())
            .filter((line) => {
              if (!line.includes(needle)) return false
              const stat = line.split(/\s+/)[1] ?? ""
              return !stat.startsWith("Z")
            }),
        )
      },
    )
  })
}
async function readMarker(path, phase) {
  const deadline = Date.now() + 4000
  while (Date.now() < deadline) {
    if (existsSync(path)) {
      try {
        const parsed = JSON.parse(readFileSync(path, "utf8"))
        if (parsed.phase === phase && Number.isSafeInteger(parsed.pid)) return parsed
      } catch {
        // A concurrent writer can be observed mid-replace.
      }
    }
    await sleep(10)
  }
  throw new Error(`marker ${phase} was not observed`)
}
async function readPidFile(path) {
  const deadline = Date.now() + 4000
  while (Date.now() < deadline) {
    if (existsSync(path)) {
      const pid = Number(readFileSync(path, "utf8"))
      if (Number.isSafeInteger(pid) && pid > 0) return pid
    }
    await sleep(10)
  }
  throw new Error("version pid was not observed")
}
async function runManual(t, scenario, extra = [], act) {
  const paths = manualPaths(t)
  const { child, exited } = spawnManual(controlledArgs(paths, scenario, extra))
  t.after(async () => {
    try {
      child.kill("SIGKILL")
    } catch {
      // The manual command already exited.
    }
    try {
      for (const line of await commandLines(paths.marker)) {
        const pid = Number(line.split(/\s+/)[0])
        if (!Number.isSafeInteger(pid)) continue
        try {
          process.kill(-pid, "SIGKILL")
        } catch {
          // The group leader may already be gone.
        }
        try {
          process.kill(pid, "SIGKILL")
        } catch {
          // Already reaped.
        }
      }
    } catch {
      // Process-state failure must not hide the test result.
    }
  })
  if (act) await act({ child, paths })
  const finished = await exited
  assert.equal(finished.signal, null)
  assert.equal(finished.stderr, "")
  const saved = JSON.parse(readFileSync(paths.out, "utf8"))
  assert.doesNotMatch(JSON.stringify(saved), /SECRET_/)
  const reported = JSON.parse(finished.stdout)
  assert.deepEqual(
    reported,
    saved.outcome.kind === "completed"
      ? {
          ...saved.outcome,
          nativeSessionUpdates: saved.source.nativeSessionUpdates,
          permissionRequests: saved.source.permissionRequests,
        }
      : saved.outcome,
  )
  return { paths, finished, saved }
}

test(
  "manual entry keeps scripted success and recording failure without a signal",
  manual,
  async (t) => {
    const success = await runManual(t, "complete")
    assert.equal(success.finished.code, 0)
    assert.equal(success.saved.outcome.kind, "completed")
    assert.equal(success.saved.outcome.stopReason, "end_turn")
    assert.deepEqual(success.saved.source.probeCleanup, {
      kind: "stopped",
      leaderReaped: true,
      liveGroupMembers: 0,
    })
    assert.deepEqual(readdirSync(success.paths.workspaceParent), [])
    assert.deepEqual(await commandLines(success.paths.marker), [])
    const failure = await runManual(t, "invalid-json")
    assert.equal(failure.finished.code, 1)
    assert.deepEqual(failure.saved.outcome, { kind: "failed", code: "invalid_frame" })
    assert.equal(Object.hasOwn(failure.saved.outcome, "signal"), false)
    assert.deepEqual(readdirSync(failure.paths.workspaceParent), [])
    assert.deepEqual(await commandLines(failure.paths.marker), [])
  },
)

test(
  "manual SIGINT before opening records interruption and releases the provider group",
  manual,
  async (t) => {
    const { paths, finished, saved } = await runManual(
      t,
      "hold-initialize",
      [],
      async ({ child, paths: manualPaths }) => {
        await readMarker(manualPaths.marker, "started")
        assert.equal(child.kill("SIGINT"), true)
      },
    )
    assert.equal(finished.code, 1)
    assert.deepEqual(saved.outcome, {
      kind: "failed",
      code: "interrupted",
      signal: "SIGINT",
    })
    assert.deepEqual(saved.frames, [])
    assert.equal(saved.admission, undefined)
    assert.deepEqual(saved.source.probeCleanup, {
      kind: "stopped",
      leaderReaped: true,
      liveGroupMembers: 0,
    })
    assert.deepEqual(readdirSync(paths.workspaceParent), [])
    assert.deepEqual(await commandLines(paths.marker), [])
    const marker = JSON.parse(readFileSync(paths.marker, "utf8"))
    assert.ok(
      (await state(marker.pid)) === "" || (await state(marker.pid)).startsWith("Z"),
    )
  },
)

test(
  "manual SIGINT during a pending prompt keeps the first signal through a later SIGTERM",
  manual,
  async (t) => {
    const run = await runManual(
      t,
      "hold-prompt",
      ["--grace", "2000", "--confirm", "2000"],
      async ({ child, paths }) => {
        await readMarker(paths.marker, "prompt")
        assert.equal(child.kill("SIGINT"), true)
        await readMarker(paths.marker, "term")
        assert.equal(child.kill("SIGTERM"), true)
      },
    )
    assert.equal(run.finished.code, 1)
    assert.deepEqual(run.saved.outcome, {
      kind: "failed",
      code: "interrupted",
      signal: "SIGINT",
    })
    assert.deepEqual(run.saved.frames, [])
    assert.deepEqual(run.saved.source.probeCleanup, {
      kind: "stopped",
      leaderReaped: true,
      liveGroupMembers: 0,
    })
    assert.deepEqual(readdirSync(run.paths.workspaceParent), [])
    assert.deepEqual(await commandLines(run.paths.marker), [])
  },
)

test(
  "manual SIGINT during cleanup preserves the earlier close failure and still releases the group",
  manual,
  async (t) => {
    const run = await runManual(
      t,
      "cleanup-hold",
      ["--grace", "2000", "--confirm", "2000"],
      async ({ child, paths }) => {
        await readMarker(paths.marker, "term")
        assert.equal(child.kill("SIGINT"), true)
      },
    )
    assert.equal(run.finished.code, 1)
    assert.deepEqual(run.saved.outcome, {
      kind: "failed",
      code: "close_rejected",
      rpcCode: -32000,
    })
    assert.equal(Object.hasOwn(run.saved.outcome, "signal"), false)
    assert.deepEqual(run.saved.source.probeCleanup, {
      kind: "stopped",
      leaderReaped: true,
      liveGroupMembers: 0,
    })
    assert.deepEqual(readdirSync(run.paths.workspaceParent), [])
    assert.deepEqual(await commandLines(run.paths.marker), [])
  },
)

test(
  "manual interruption with unconfirmed cleanup retains the workspace and the interrupted cause",
  manual,
  async (t) => {
    const run = await runManual(
      t,
      "hold-prompt",
      ["--grace", "0", "--confirm", "0"],
      async ({ child, paths }) => {
        await readMarker(paths.marker, "prompt")
        assert.equal(child.kill("SIGINT"), true)
      },
    )
    assert.equal(run.finished.code, 1)
    assert.deepEqual(run.saved.outcome, {
      kind: "failed",
      code: "interrupted",
      signal: "SIGINT",
      cleanupCode: "cleanup_unconfirmed",
    })
    assert.equal(run.saved.source.probeCleanup.kind, "unconfirmed")
    assert.equal(readdirSync(run.paths.workspaceParent).length, 1)
  },
)

test(
  "manual SIGINT during version lookup interrupts after that group is cleaned",
  manual,
  async (t) => {
    const run = await runManual(
      t,
      "complete",
      ["--version-scenario", "version-stall", "--version-budget", "3000"],
      async ({ child, paths }) => {
        const versionPid = await readPidFile(`${paths.marker}.version`)
        assert.equal(child.kill("SIGINT"), true)
        // Let the first signal latch before the second is sent. Two different
        // pending signals have no delivery order.
        await sleep(50)
        assert.equal(child.kill("SIGTERM"), true)
        paths.versionPid = versionPid
      },
    )
    assert.equal(run.finished.code, 1)
    assert.deepEqual(run.saved.outcome, {
      kind: "failed",
      code: "interrupted",
      signal: "SIGINT",
    })
    assert.equal(run.saved.source.versionCleanup.kind, "stopped")
    assert.deepEqual(run.saved.source.probeCleanup, {
      kind: "stopped",
      leaderReaped: true,
      liveGroupMembers: 0,
    })
    assert.deepEqual(readdirSync(run.paths.workspaceParent), [])
    assert.deepEqual(await commandLines(run.paths.marker), [])
    assert.ok(
      (await state(run.paths.versionPid)) === "" ||
        (await state(run.paths.versionPid)).startsWith("Z"),
    )
  },
)

test("a malformed manual command exits before starting a provider", async () => {
  const { exited } = spawnManual([])
  const finished = await exited
  assert.equal(finished.code, 2)
  assert.equal(finished.signal, null)
  assert.equal(finished.stdout, "")
  assert.match(finished.stderr, /^capture\.mjs /)
})
