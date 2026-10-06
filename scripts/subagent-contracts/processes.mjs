/** POSIX process-group ownership for this probe and its bounded version check. */
import { execFile, spawn } from "node:child_process"
import { setTimeout as sleep } from "node:timers/promises"

/** Windows live capture is refused; credential-free evidence inspection is portable. */
export function startProbeProcess(command, args, options = {}) {
  if (process.platform === "win32") throw { code: "unsupported_process_ownership" }
  const child = spawn(command, args, {
    ...options,
    detached: true,
    stdio: ["pipe", "pipe", "pipe"],
  })
  const owned = { child, closed: false, stopping: undefined }
  child.on("error", () => {})
  child.on("close", () => {
    owned.closed = true
  })
  child.stdin.on("error", () => {})
  child.stderr.resume()
  return owned
}

/** Count live members, excluding zombies; no command arguments/account data is read. */
function groupMembers(group, budget) {
  return new Promise((resolve, reject) => {
    execFile(
      "/bin/ps",
      ["-eo", "pgid=,stat="],
      { timeout: budget, killSignal: "SIGKILL", maxBuffer: 1024 * 1024 },
      (error, stdout) => {
        if (error) return reject({ code: "process_state_unavailable" })
        let live = 0
        for (const line of stdout.trim().split("\n")) {
          if (!line.trim()) continue
          const match = /^\s*(\d+)\s+(\S+)\s*$/.exec(line)
          if (!match) return reject({ code: "process_state_unavailable" })
          if (Number(match[1]) === group && !match[2].startsWith("Z")) live++
        }
        resolve(live)
      },
    )
  })
}

function signalGroup(owned, signal) {
  if (!owned.child.pid) return
  try {
    process.kill(-owned.child.pid, signal)
  } catch (error) {
    if (error.code !== "ESRCH") throw { code: "process_signal_failed" }
  }
}

/**
 * Reap the leader and confirm no live members. Leader exit cannot release the
 * group: the leader-first regression retains escalation while a child ignores TERM.
 * Escaped groups are outside this developer probe's ownership.
 */
export function stopProbeProcess(owned, { graceMs = 2000, confirmMs = 2000 } = {}) {
  if (owned.stopping) return owned.stopping
  owned.stopping = (async () => {
    const waitForRelease = async (budget) => {
      const deadline = Date.now() + budget
      while (Date.now() < deadline) {
        const remaining = deadline - Date.now()
        if (remaining < 25) return false
        let live
        try {
          live = owned.child.pid
            ? await groupMembers(owned.child.pid, Math.min(500, remaining))
            : 0
        } catch (error) {
          if (Date.now() >= deadline) return false
          throw error
        }
        if (live === 0 && owned.closed) return true
        await sleep(Math.min(10, Math.max(1, deadline - Date.now())))
      }
      return false
    }
    try {
      signalGroup(owned, "SIGTERM")
      if (await waitForRelease(graceMs))
        return { kind: "stopped", leaderReaped: true, liveGroupMembers: 0 }
      signalGroup(owned, "SIGKILL")
      if (await waitForRelease(confirmMs))
        return { kind: "stopped", leaderReaped: true, liveGroupMembers: 0 }
    } catch {
      // A failed state query still attempts the necessary physical teardown.
      try {
        signalGroup(owned, "SIGKILL")
      } catch {}
    }
    return { kind: "unconfirmed", code: "cleanup_unconfirmed" }
  })()
  return owned.stopping
}

/** A stalled, failed or oversized version command yields unavailable after cleanup. */
export async function providerVersion(
  command,
  args = ["--version"],
  { budgetMs = 2000, graceMs = 100, confirmMs = 2000 } = {},
) {
  let owned
  try {
    owned = startProbeProcess(command, args)
  } catch {
    return {
      value: "unavailable",
      cleanup: { kind: "stopped", leaderReaped: true, liveGroupMembers: 0 },
    }
  }
  let text = ""
  let bytes = 0
  let overflow = false
  let timer
  const result = await new Promise((resolve) => {
    timer = setTimeout(() => resolve(false), budgetMs)
    owned.child.stdout.on("data", (chunk) => {
      bytes += chunk.length
      if (bytes > 4096) {
        overflow = true
        resolve(false)
      } else text += chunk.toString("utf8")
    })
    owned.child.on("error", () => resolve(false))
    owned.child.on("close", (code) => resolve(code === 0 && !overflow))
  })
  clearTimeout(timer)
  const cleanup = await stopProbeProcess(owned, { graceMs, confirmMs })
  return {
    value:
      result && cleanup.kind === "stopped" && /^codex-cli [0-9][\w.+-]*\s*$/.test(text)
        ? text.trim()
        : "unavailable",
    cleanup,
  }
}
