import { existsSync, readFileSync } from "node:fs"
import { setTimeout as delay } from "node:timers/promises"

/** Turn termination signals into one abort that lets the harness reach cleanup. */
export function watchInterruptions() {
  const controller = new AbortController()
  const interrupt = (name) => {
    if (!controller.signal.aborted)
      controller.abort(new Error(`native-window smoke interrupted by ${name}`))
  }
  const handlers = new Map(
    ["SIGINT", "SIGTERM"].map((name) => [name, () => interrupt(name)]),
  )
  for (const [name, handler] of handlers) process.on(name, handler)
  return {
    signal: controller.signal,
    interrupt,
    dispose() {
      for (const [name, handler] of handlers) process.off(name, handler)
    },
  }
}

/** Whether a process currently answers the signal-zero ownership check. */
export function alive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

/** Read a PID written into this run's private temporary directory. */
export function recordedPid(path) {
  if (!existsSync(path)) return undefined
  const pid = Number(readFileSync(path, "utf8").trim())
  return Number.isInteger(pid) && pid > 0 ? pid : undefined
}

async function gone(pid, timeout = 5_000) {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    if (!alive(pid)) return true
    await delay(50)
  }
  return !alive(pid)
}

function groupAlive(groupId) {
  try {
    process.kill(-groupId, 0)
    return true
  } catch {
    return false
  }
}

async function groupGone(groupId, timeout = 5_000) {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    if (!groupAlive(groupId)) return true
    await delay(50)
  }
  return !groupAlive(groupId)
}

/**
 * Stop the detached process group this run created, even if its leader exited.
 *
 * Children retain the leader's process-group id after that exit, which is the
 * early-failure case this helper exists to clean up.
 */
export async function stopOwnedGroup(groupId) {
  if (!Number.isInteger(groupId) || groupId <= 0) return true
  try {
    process.kill(-groupId, "SIGTERM")
  } catch {}
  if (await groupGone(groupId)) return true
  try {
    process.kill(-groupId, "SIGKILL")
  } catch {}
  return groupGone(groupId)
}

/** Stop an exact PID recorded by this run, after its owning group is stopped. */
export async function stopRecordedProcess(pid) {
  if (!alive(pid)) return
  try {
    process.kill(pid, "SIGTERM")
  } catch {}
  if (await gone(pid)) return
  try {
    process.kill(pid, "SIGKILL")
  } catch {}
  await gone(pid)
}
