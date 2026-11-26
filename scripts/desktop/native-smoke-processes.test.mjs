import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { EventEmitter, once } from "node:events"
import { mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { PassThrough } from "node:stream"
import { setTimeout as delay } from "node:timers/promises"
import test from "node:test"

import {
  alive,
  createProcessControl,
  OwnedCommandCleanupError,
  runOwnedCommand,
  stopOwnedGroup,
  watchInterruptions,
} from "./native-smoke-processes.mjs"

const entry = (pid, groupId, state = "S") => ({ pid, groupId, state })

function ownedCommand(clean = true, captureLimit = 16) {
  const owner = new AbortController()
  const deadline = new AbortController()
  const child = new EventEmitter()
  child.pid = 41
  child.stdout = new PassThrough()
  const calls = []
  child.unref = () => calls.push("unref")
  const run = runOwnedCommand(
    "fixture-command",
    [],
    { signal: owner.signal, timeout: 500, captureLimit },
    {
      start: (command, args, options) => {
        calls.push([command, args, options.detached])
        return child
      },
      stop: async (pid) => {
        calls.push(pid)
        return clean
      },
      timeoutSignal: (timeout) => {
        assert.equal(timeout, 500)
        return deadline.signal
      },
    },
  )
  return { run, owner, deadline, child, calls }
}

test("owned command waits for stdout close and settles the detached group", async () => {
  const { run, child, calls } = ownedCommand()
  child.emit("exit", 0, null)
  child.stdout.write("artifact")
  assert.deepEqual(calls, [["fixture-command", [], true]])
  child.emit("close", 0, null)
  assert.equal(await run, "artifact")
  assert.deepEqual(calls, [["fixture-command", [], true], 41])
})

test("owned command interruption or deadline stops its group without waiting for exit", async () => {
  for (const interrupt of ["owner", "deadline"]) {
    const fixture = ownedCommand()
    fixture[interrupt].abort(new Error("interrupted"))
    await assert.rejects(
      fixture.run,
      interrupt === "owner" ? /interrupted/ : /500ms deadline/,
    )
    assert.deepEqual(fixture.calls, [["fixture-command", [], true], 41])
  }
})

test("owned command limits captured bytes and preserves unconfirmed cleanup", async () => {
  const { run, child, calls } = ownedCommand(false, 3)
  child.stdout.write("four")
  await assert.rejects(run, (error) => {
    assert.ok(error instanceof OwnedCommandCleanupError)
    assert.match(error.cause.errors[0].message, /exceeded 3 bytes/)
    return true
  })
  assert.equal(child.stdout.destroyed, true)
  assert.deepEqual(calls, [["fixture-command", [], true], 41, "unref"])
})

test("unconfirmed cleanup lets the supervising Node process exit while its child lives", async (context) => {
  if (process.platform === "win32") return context.skip("POSIX process groups only")
  const directory = mkdtempSync(join(tmpdir(), "nessa-smoke-unconfirmed-"))
  const pidFile = join(directory, "child.pid")
  const processes = new URL("./native-smoke-processes.mjs", import.meta.url)
  const parent = spawn(
    process.execPath,
    [
      "--input-type=module",
      "--eval",
      `
        import { spawn } from "node:child_process"
        import { writeFileSync } from "node:fs"
        import { runOwnedCommand, OwnedCommandCleanupError } from ${JSON.stringify(processes.href)}

        const owner = new AbortController()
        try {
          await runOwnedCommand(process.execPath, ["-e", "setTimeout(() => {}, 12_000)"], {
            signal: owner.signal,
            timeout: 2_000,
            captureLimit: 16,
          }, {
            start(command, args, options) {
              const child = spawn(command, args, options)
              writeFileSync(process.argv[1], String(child.pid))
              child.once("spawn", () => owner.abort(new Error("fixture interrupted")))
              return child
            },
            stop: async () => false,
          })
        } catch (error) {
          if (!(error instanceof OwnedCommandCleanupError)) throw error
          process.exitCode = 37
        }
      `,
      pidFile,
    ],
    { detached: true, stdio: "ignore" },
  )
  try {
    const exit = await once(parent, "exit", { signal: AbortSignal.timeout(5_000) })
    assert.deepEqual(exit, [37, null])
    assert.equal(alive(Number(readFileSync(pidFile, "utf8"))), true)
  } finally {
    const parentGone = await stopOwnedGroup(parent.pid, 500)
    const childGone = await stopOwnedGroup(Number(readFile(pidFile)), 500)
    rmSync(directory, { recursive: true, force: true })
    assert.ok(parentGone && childGone, "test process cleanup unconfirmed")
  }
})

function controlled(snapshot, sendSignal) {
  let time = 0
  return createProcessControl({
    snapshot,
    sendSignal,
    pause: async (milliseconds) => {
      time += milliseconds
    },
    now: () => time,
  })
}

test("an interruption aborts work so the harness can enter cleanup", () => {
  const interruption = watchInterruptions()
  try {
    interruption.interrupt("SIGTERM")
    assert.equal(interruption.signal.aborted, true)
    assert.match(interruption.signal.reason.message, /SIGTERM/)
  } finally {
    interruption.dispose()
  }
})

test("an exited smoke group leader does not leave its child behind", async (context) => {
  if (process.platform === "win32") return context.skip("POSIX process groups only")
  const directory = mkdtempSync(join(tmpdir(), "nessa-smoke-cleanup-"))
  const pidFile = join(directory, "child.pid")
  const leader = spawn("sh", ["-c", `sleep 30 & echo $! > "$1"`, "sh", pidFile], {
    detached: true,
    stdio: "ignore",
  })
  try {
    await new Promise((resolve) => leader.once("exit", resolve))
    for (let attempt = 0; attempt < 100 && !readFile(pidFile); attempt += 1)
      await delay(10)
    const child = Number(readFileSync(pidFile, "utf8").trim())
    assert.equal(alive(child), true)

    assert.equal(await stopOwnedGroup(leader.pid), true)

    assert.equal(alive(child), false)
  } finally {
    await stopOwnedGroup(leader.pid)
    rmSync(directory, { recursive: true, force: true })
  }
})

test("denied group and exact-PID signals are cleanup failures", async () => {
  const calls = []
  const denied = controlled(
    () => [entry(42, 40)],
    (target, name) => {
      calls.push([target, name])
      const error = new Error("denied")
      error.code = "EPERM"
      throw error
    },
  )

  assert.equal(await denied.stopOwnedGroup(40, 100), false)
  assert.equal(await denied.stopRecordedProcess(42, 100), false)
  assert.deepEqual(calls, [
    [-40, "SIGTERM"],
    [-40, "SIGKILL"],
    [42, "SIGTERM"],
    [42, "SIGKILL"],
  ])
})

test("an unknown process snapshot never means cleanup succeeded", async () => {
  const calls = []
  const unknown = controlled(
    () => {
      throw new Error("process table unavailable")
    },
    (target, name) => calls.push([target, name]),
  )

  assert.equal(await unknown.stopOwnedGroup(50, 100), false)
  assert.deepEqual(calls, [])
})

test("losing observation after TERM prevents an unverified KILL", async () => {
  let observations = 0
  const calls = []
  const unknown = controlled(
    () => {
      if (observations++ === 0) return [entry(52, 51)]
      throw new Error("process table unavailable")
    },
    (target, name) => calls.push([target, name]),
  )

  assert.equal(await unknown.stopOwnedGroup(51, 100), false)
  assert.deepEqual(calls, [[-51, "SIGTERM"]])
})

test("ESRCH is accepted only after a fresh snapshot proves absence", async () => {
  const missing = new Error("gone")
  missing.code = "ESRCH"
  let observed = 0
  const vanished = controlled(
    () => (++observed === 1 ? [entry(56, 55)] : []),
    () => {
      throw missing
    },
  )
  const stillPresent = controlled(
    () => [entry(58, 57)],
    () => {
      throw missing
    },
  )

  assert.equal(await vanished.stopOwnedGroup(55, 100), true)
  assert.equal(await stillPresent.stopOwnedGroup(57, 100), false)
})

test("a surviving group escalates from TERM to KILL", async () => {
  let active = true
  const calls = []
  const processes = controlled(
    () => (active ? [entry(61, 60)] : []),
    (target, name) => {
      calls.push([target, name])
      if (name === "SIGKILL") active = false
    },
  )

  assert.equal(await processes.stopOwnedGroup(60, 100), true)
  assert.deepEqual(calls, [
    [-60, "SIGTERM"],
    [-60, "SIGKILL"],
  ])
})

test("a group still executable after KILL is not reported gone", async () => {
  const processes = controlled(
    () => [entry(71, 70)],
    () => {},
  )

  assert.equal(await processes.stopOwnedGroup(70, 100), false)
})

test("PID state distinguishes executable, zombie, absent, and unknown", () => {
  const processes = controlled(
    () => [entry(81, 80), entry(82, 80, "Z")],
    () => {},
  )
  const unknown = controlled(
    () => {
      throw new Error("process table unavailable")
    },
    () => {},
  )

  assert.equal(processes.alive(81), true)
  assert.equal(processes.alive(82), false)
  assert.equal(processes.alive(83), false)
  assert.equal(unknown.alive(84), true)
})

function readFile(path) {
  try {
    return readFileSync(path, "utf8")
  } catch {
    return ""
  }
}
