#!/usr/bin/env node
/**
 * Free the dev gateway port before `just start`, and refuse to fight anything
 * that is not ours.
 *
 * The installed app's gateway is a launchd background service. It is meant to
 * outlive the app, so `kill -TERM` on its listener is answered by launchd
 * restarting it a moment later — the PID changes and the port never opens. That
 * is not a race to win, it is a service someone else owns. The dev stage has
 * its own port for exactly this reason (`protocol/defaults/gateway-ports.json`),
 * so a collision here is now unusual and worth explaining rather than resolving
 * by force.
 *
 * Exit 0 means the port is free. Exit 1 names the owner and what to do.
 *
 *   node scripts/free-gateway-port.mjs [stage]   # default dev
 */
import { spawnSync } from "node:child_process"
import { dirname, resolve } from "node:path"
import { setTimeout as sleep } from "node:timers/promises"
import { fileURLToPath } from "node:url"

import { gatewayPort, selectedStage } from "./gateway-port.mjs"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..")
// The same resolution the server and the health probe use. Freeing the stage's
// table port while the server was told `NESSA_PORT` kills whatever is on the
// table port — another checkout's gateway, say — and leaves the port actually
// wanted still held. This script's own advice is to "set NESSA_PORT for this
// run", which only works if this script reads it.
const stage = process.argv[2] ?? selectedStage()
const override = (process.env.NESSA_PORT ?? "").trim()
const port = override === "" ? gatewayPort(stage) : Number(override)
if (!Number.isInteger(port) || port <= 0 || port > 65535) {
  console.error(`NESSA_PORT is not a port: ${override}`)
  process.exit(1)
}

function run(command, args) {
  return spawnSync(command, args, { encoding: "utf8" })
}

/** @returns {{ pid: number, command: string }[]} */
function listeners() {
  if (process.platform === "win32") {
    const out = run("netstat", ["-ano", "-p", "TCP"]).stdout ?? ""
    const pids = new Set()
    for (const line of out.split(/\r?\n/)) {
      // Prefer `:7421` as its own port field, not a prefix of `:74210`.
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
 * The launchd label supervising `pid`, if any.
 *
 * `launchctl list` prints `PID\tStatus\tLabel` for the calling user's domain
 * and needs no privileges. A hit means launchd owns the process lifetime: it
 * will respawn on SIGTERM, and only `bootout` actually stops it.
 *
 * @returns {string | null}
 */
function launchdLabel(pid) {
  if (process.platform !== "darwin") return null
  const out = run("/bin/launchctl", ["list"]).stdout ?? ""
  for (const line of out.split("\n").slice(1)) {
    const [servicePid, , label] = line.split("\t")
    if (Number(servicePid) === pid && label) return label.trim()
  }
  return null
}

/**
 * A nessa-server this checkout started: a gateway whose working directory is
 * this tree. A gateway staged under Application Support and launched by launchd
 * is not ours even when it is the same build.
 *
 * The working directory, and not the command line. A command was treated as
 * evidence of ownership if it contained this root anywhere in it, and that is
 * not what it means:
 *
 *   - `/work/nessa-agent-other/target/debug/nessa server` contains
 *     `/work/nessa-agent` as a substring, so a sibling checkout's gateway read
 *     as ours and was sent SIGTERM.
 *   - Worktrees symlink `target/` into the main checkout, so a worktree's
 *     gateway genuinely *is* this checkout's binary — the same path, a
 *     different owner. No amount of path-boundary care fixes that one.
 *
 * `just start` runs the server with this tree as its working directory, so a
 * leftover of ours has it; anything else is somebody else's and is reported
 * rather than killed.
 */
function isOurDevServer({ pid, command }) {
  if (!/\bnessa\b.*\bserver\b/.test(command) && !/nessa-server/.test(command))
    return false
  return belongsToThisCheckout(pid, command)
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
function belongsToThisCheckout(pid, command) {
  const cwd = processCwd(pid)
  if (cwd)
    return cwd === root || cwd.startsWith(`${root}/`) || cwd.startsWith(`${root}\\`)
  if (process.platform !== "win32") return false
  return command.includes(`${root}\\`) || command.includes(`${root}/`)
}

function killPid(pid, signal) {
  if (process.platform === "win32") {
    run("taskkill", ["/PID", String(pid), "/T", "/F"])
    return
  }
  try {
    process.kill(pid, signal)
  } catch {
    // already gone
  }
}

async function waitUntilGone(pid, attempts = 20) {
  for (let i = 0; i < attempts; i += 1) {
    if (!listeners().some((row) => row.pid === pid)) return true
    await sleep(100)
  }
  return false
}

function label(command) {
  return command.length > 120 ? `${command.slice(0, 117)}...` : command
}

let failed = false

for (const row of listeners()) {
  const service = launchdLabel(row.pid)
  if (service !== null) {
    console.error(
      `→ :${port} is held by the launchd service ${service} (pid ${row.pid}).`,
    )
    console.error(`  ${label(row.command)}`)
    console.error(
      "  launchd restarts it the moment it is killed, so nothing here will stop it.",
    )
    console.error(
      `  It is a background service, not a leftover dev server — stopping it stops the installed`,
    )
    console.error(
      `  app from working until it re-registers. If you truly want it gone for now:`,
    )
    console.error(`    launchctl bootout gui/$(id -u)/${service}`)
    console.error(
      `  Otherwise give this run a different port: NESSA_PORT=... just server`,
    )
    failed = true
    continue
  }

  if (isOurDevServer(row)) {
    console.error(`→ restarting the dev gateway on :${port} (pid ${row.pid})`)
    killPid(row.pid, "SIGTERM")
    if (!(await waitUntilGone(row.pid))) {
      killPid(row.pid, "SIGKILL")
      if (!(await waitUntilGone(row.pid))) {
        console.error(`→ could not free :${port} (pid ${row.pid})`)
        failed = true
      }
    }
    continue
  }

  console.error(`→ :${port} is already in use by pid ${row.pid}, which is not ours:`)
  console.error(`  ${label(row.command)}`)
  console.error("  stop that process yourself, or set NESSA_PORT for this run")
  failed = true
}

if (failed) process.exit(1)
