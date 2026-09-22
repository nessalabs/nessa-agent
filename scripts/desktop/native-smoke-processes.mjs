import { spawnSync } from "node:child_process"
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

/** Read a PID written into this run's private temporary directory. */
export function recordedPid(path) {
  if (!existsSync(path)) return undefined
  const pid = Number(readFileSync(path, "utf8").trim())
  return Number.isInteger(pid) && pid > 0 ? pid : undefined
}

function systemSnapshot() {
  const result = spawnSync("ps", ["-axo", "pid=,pgid=,stat="], {
    encoding: "utf8",
    timeout: 5_000,
  })
  if (result.error) throw result.error
  if (result.status !== 0)
    throw new Error(`ps exited with ${result.status}: ${result.stderr}`)
  return result.stdout
    .split("\n")
    .map((line) => line.trim().split(/\s+/))
    .filter((fields) => fields.length === 3)
    .map(([pid, groupId, state]) => ({
      pid: Number(pid),
      groupId: Number(groupId),
      state,
    }))
    .filter(
      ({ pid, groupId, state }) =>
        Number.isInteger(pid) &&
        pid > 0 &&
        Number.isInteger(groupId) &&
        groupId > 0 &&
        state.length > 0,
    )
}

/**
 * Process cleanup over one injected operating-system boundary.
 *
 * A snapshot names processes. The controller treats non-zombies as executable;
 * zombies are already unable to run and are left for their actual parent to
 * reap. Observation errors remain unknown, never absence. Signals target only
 * a PID or the exact negative PGID supplied by the caller that created it.
 */
export function createProcessControl({
  snapshot = systemSnapshot,
  sendSignal = (target, name) => process.kill(target, name),
  pause = delay,
  now = Date.now,
} = {}) {
  function observe(matches) {
    try {
      return {
        active: snapshot().some(
          (entry) => !entry.state.startsWith("Z") && matches(entry),
        ),
        reliable: true,
      }
    } catch {
      return { active: true, reliable: false }
    }
  }

  const observePid = (pid) => observe((entry) => entry.pid === pid)
  const observeGroup = (groupId) => observe((entry) => entry.groupId === groupId)

  async function waitUntilGone(observeOwned, timeout) {
    const deadline = now() + timeout
    let reliable = true
    while (now() < deadline) {
      const observed = observeOwned()
      reliable &&= observed.reliable
      if (observed.reliable && !observed.active) return { gone: true, reliable }
      await pause(50)
    }
    const observed = observeOwned()
    return {
      gone: observed.reliable && !observed.active,
      reliable: reliable && observed.reliable,
    }
  }

  function signalOwned(target, name, observeOwned) {
    try {
      sendSignal(target, name)
      return true
    } catch (error) {
      if (error?.code !== "ESRCH") return false
      const observed = observeOwned()
      return observed.reliable && !observed.active
    }
  }

  async function stop(target, observeOwned, timeout) {
    const initial = observeOwned()
    if (!initial.reliable) return false
    if (!initial.active) return true
    let succeeded = true
    succeeded = signalOwned(target, "SIGTERM", observeOwned) && succeeded
    const afterTerm = await waitUntilGone(observeOwned, timeout)
    succeeded = afterTerm.reliable && succeeded
    if (!afterTerm.reliable) return false
    if (afterTerm.gone) return succeeded
    succeeded = signalOwned(target, "SIGKILL", observeOwned) && succeeded
    const afterKill = await waitUntilGone(observeOwned, timeout)
    return succeeded && afterKill.reliable && afterKill.gone
  }

  return {
    /** Whether the process table has one executable process at this PID. */
    alive(pid) {
      if (!Number.isInteger(pid) || pid <= 0) return false
      return observePid(pid).active
    },

    /**
     * Stop the detached process group this run created, even if its leader
     * exited. Success means no executable members and no hidden OS failure.
     */
    stopOwnedGroup(groupId, timeout = 5_000) {
      if (!Number.isInteger(groupId) || groupId <= 0) return Promise.resolve(true)
      return stop(-groupId, () => observeGroup(groupId), timeout)
    },

    /** Stop an exact PID recorded by this run and report the observed result. */
    stopRecordedProcess(pid, timeout = 5_000) {
      if (!Number.isInteger(pid) || pid <= 0) return Promise.resolve(true)
      return stop(pid, () => observePid(pid), timeout)
    },
  }
}

const systemProcesses = createProcessControl()

export const alive = (pid) => systemProcesses.alive(pid)
export const stopOwnedGroup = (groupId) => systemProcesses.stopOwnedGroup(groupId)
export const stopRecordedProcess = (pid) => systemProcesses.stopRecordedProcess(pid)
