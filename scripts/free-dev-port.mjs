#!/usr/bin/env node
/**
 * Free the Vite (and HMR) ports before `pnpm dev` / `tauri dev` when a prior
 * Nessa vite is still listening. Leaves anything else alone and fails fast
 * with a clear owner — better than cargo compiling then vite dying on bind.
 *
 * Ports match vite.config.ts (1420) and its optional HMR (1421).
 */
import { spawnSync } from "node:child_process"
import { dirname, resolve } from "node:path"
import { setTimeout as sleep } from "node:timers/promises"
import { fileURLToPath } from "node:url"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const ports = [1420, 1421]

function run(command, args) {
  return spawnSync(command, args, { encoding: "utf8" })
}

/** @returns {{ pid: number, command: string }[]} */
function listeners(port) {
  if (process.platform === "win32") {
    const out = run("netstat", ["-ano", "-p", "TCP"]).stdout ?? ""
    const pids = new Set()
    for (const line of out.split(/\r?\n/)) {
      // TCP    127.0.0.1:1420    0.0.0.0:0    LISTENING    1234
      // Prefer `:1420` as its own port field, not a prefix of `:14200`.
      if (!new RegExp(`:${port}(?:\\s|$)`).test(line) || !/LISTENING/i.test(line))
        continue
      const pid = Number(line.trim().split(/\s+/).at(-1))
      if (Number.isFinite(pid) && pid > 0) pids.add(pid)
    }
    return [...pids].map((pid) => ({ pid, command: winCommand(pid) }))
  }

  const out = run("lsof", ["-nP", `-iTCP:${port}`, "-sTCP:LISTEN"]).stdout ?? ""
  const byPid = new Map()
  for (const line of out.split("\n").slice(1)) {
    const cols = line.trim().split(/\s+/)
    const pid = Number(cols[1])
    if (!Number.isFinite(pid) || byPid.has(pid)) continue
    byPid.set(pid, { pid, command: unixCommand(pid) })
  }
  return [...byPid.values()]
}

function unixCommand(pid) {
  const ps = run("ps", ["-p", String(pid), "-ww", "-o", "command="])
  return (ps.stdout ?? "").trim() || `(pid ${pid})`
}

function winCommand(pid) {
  const ps = run("powershell.exe", [
    "-NoProfile",
    "-Command",
    `(Get-CimInstance Win32_Process -Filter "ProcessId=${pid}").CommandLine`,
  ])
  return (ps.stdout ?? "").trim() || `(pid ${pid})`
}

function processCwd(pid) {
  if (process.platform === "win32") return ""
  const out = run("lsof", ["-a", "-p", String(pid), "-d", "cwd", "-Fn"]).stdout ?? ""
  for (const line of out.split("\n")) {
    if (line.startsWith("n")) return line.slice(1)
  }
  return ""
}

/**
 * A leftover from this checkout: node running vite (or the pnpm wrapper) from
 * our tree. Cursor Agents port-forwards are not ours.
 */
function isNessaVite({ pid, command }) {
  if (!belongsToThisCheckout(pid, command)) return false
  return /\bvite\b/.test(command) || /node_modules[/\\]\.bin[/\\]vite/.test(command)
}

/**
 * Whether a process belongs to this checkout.
 *
 * The working directory is the evidence, because a command line is not: a
 * sibling checkout at `/work/nessa-agent-other` contains `/work/nessa-agent` as
 * a substring, and worktrees symlink `target/` into the main checkout so a
 * worktree's process genuinely runs this checkout's binary.
 *
 * Windows has no cwd to read here (`lsof` is not there), so it falls back to a
 * path-boundary check on the command — which rules out the sibling-prefix case
 * and cannot rule out a shared binary. That is weaker, and it is said out loud
 * rather than left to look the same as the Unix answer.
 */
/**
 * Worktrees this repository keeps *inside* itself, which are not this checkout.
 *
 * `scripts/worktree.sh` makes siblings, and the prefix test below rules those
 * out. Agent worktrees land under `.claude/worktrees/`, which is beneath this
 * root, so the same test called them ours and a run from the main checkout
 * would SIGTERM — then SIGKILL — a vite belonging to somebody else's session.
 * Sharing a `target/` symlink makes a worktree's process run this checkout's
 * binary; it does not make its dev server this checkout's to stop.
 */
const NESTED_CHECKOUTS = `${root}/.claude/worktrees/`

function belongsToThisCheckout(pid, command) {
  const cwd = processCwd(pid)
  if (cwd && cwd.startsWith(NESTED_CHECKOUTS)) return false
  if (cwd)
    return cwd === root || cwd.startsWith(`${root}/`) || cwd.startsWith(`${root}\\`)
  if (process.platform !== "win32") return false
  return command.includes(`${root}\\`) || command.includes(`${root}/`)
}

function killPid(pid) {
  if (process.platform === "win32") {
    run("taskkill", ["/PID", String(pid), "/T", "/F"])
    return
  }
  try {
    process.kill(pid, "SIGTERM")
  } catch {
    return
  }
}

async function waitUntilFree(port, pid, attempts = 20) {
  for (let i = 0; i < attempts; i += 1) {
    const still = listeners(port).some((row) => row.pid === pid)
    if (!still) return true
    await sleep(100)
  }
  return false
}

let failed = false

for (const port of ports) {
  const rows = listeners(port)
  if (rows.length === 0) continue

  for (const row of rows) {
    if (isNessaVite(row)) {
      console.error(`→ freeing :${port} (nessa vite pid ${row.pid})`)
      killPid(row.pid)
      if (!(await waitUntilFree(port, row.pid))) {
        console.error(
          `→ pid ${row.pid} still listening on :${port} after SIGTERM; sending SIGKILL`,
        )
        try {
          if (process.platform === "win32") killPid(row.pid)
          else process.kill(row.pid, "SIGKILL")
        } catch {
          // already gone
        }
        if (!(await waitUntilFree(port, row.pid))) {
          console.error(`→ could not free :${port} (pid ${row.pid})`)
          failed = true
        }
      }
      continue
    }

    const label =
      row.command.length > 120 ? `${row.command.slice(0, 117)}...` : row.command
    console.error(`→ port ${port} is already in use by pid ${row.pid}, not a Nessa vite:`)
    console.error(`  ${label}`)
    console.error(
      `  stop that process (or the Cursor Agents preview forwarding this port), then retry`,
    )
    failed = true
  }
}

if (failed) process.exit(1)
