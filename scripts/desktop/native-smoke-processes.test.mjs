import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { setTimeout as delay } from "node:timers/promises"
import test from "node:test"

import { alive, stopOwnedGroup, watchInterruptions } from "./native-smoke-processes.mjs"

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

function readFile(path) {
  try {
    return readFileSync(path, "utf8")
  } catch {
    return ""
  }
}
