#!/usr/bin/env node
/**
 * Free the dev gateway port before `just start` / `just dev`, and refuse to
 * fight anything that is not ours.
 *
 * The installed app's gateway is a launchd background service. It is meant to
 * outlive the app, so `kill -TERM` on its listener is answered by launchd
 * restarting it a moment later — the PID changes and the port never opens. That
 * is not a race to win, it is a service someone else owns. The dev stage has
 * its own port for exactly this reason (`protocol/defaults/gateway-ports.json`),
 * but a dev gateway can be a launchd service too — a test install registered
 * under the dev stage keeps :7421 for as long as it is registered.
 *
 * Two modes:
 *
 *   node scripts/free-gateway-port.mjs [stage]          # free the port or refuse
 *   node scripts/free-gateway-port.mjs --move [stage]   # free it, or take the next
 *
 * Without `--move`, exit 0 means the port is free; exit 1 names the owner and
 * what to do.
 *
 * With `--move` (what the dev recipes use), a stage port held by something that
 * is not ours is reported and stepped around: the next port that is free, or
 * held only by a dev gateway of this checkout, is printed on stdout for the
 * caller to export as `NESSA_PORT`. Everything said to the person goes to
 * stderr, so stdout is only ever the port. An explicit `NESSA_PORT` is the
 * person's choice and is never moved: it is freed or refused as before.
 */
import { spawnSync } from "node:child_process"
import net from "node:net"
import { readFileSync } from "node:fs"
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
const args = process.argv.slice(2)
const move = args.includes("--move")
const named = args.find((arg) => arg !== "--move")
// Read when run, not at import, so a test importing `choosePort` does not
// resolve a stage or exit over an environment it never meant to use.
const stageName = () => named ?? selectedStage()
const override = () => (process.env.NESSA_PORT ?? "").trim()
function wanted() {
  const port = override() === "" ? gatewayPort(stageName()) : Number(override())
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    console.error(`NESSA_PORT is not a port: ${override()}`)
    process.exit(1)
  }
  return port
}

/**
 * How far `--move` looks past the stage port. Ports another stage is named for
 * are skipped, so a moved dev gateway never sits where a product one belongs.
 */
export const MOVE_RANGE = 20
const reserved = new Set(
  Object.values(
    JSON.parse(
      readFileSync(resolve(root, "protocol/defaults/gateway-ports.json"), "utf8"),
    ).stages,
  ),
)

function run(command, args) {
  return spawnSync(command, args, { encoding: "utf8" })
}

/** @returns {{ pid: number, command: string }[]} */
function listeners(port) {
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
 *   - A process may be launched through a wrapper or another path spelling;
 *     neither makes its lifetime belong to this checkout.
 *
 * `just start` runs the server with this tree as its working directory, so a
 * leftover of ours has it; anything else is somebody else's and is reported
 * rather than killed.
 */
function isOurDevServer({ pid, command }) {
  if (!isNessaServer(command)) return false
  return belongsToThisCheckout(pid, command)
}

function isNessaServer(command) {
  return /\bnessa\b.*\bserver\b/.test(command) || /nessa-server/.test(command)
}

/**
 * Whether a process belongs to this checkout.
 *
 * The working directory is the evidence, because a command line is not: a
 * sibling checkout at `/work/nessa-agent-other` contains `/work/nessa-agent` as
 * a substring.
 *
 * Windows has no cwd to read here (`lsof` is not there), so it falls back to a
 * path-boundary check on the command. That is weaker, and it is said out loud
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

async function waitUntilGone(port, pid, attempts = 20) {
  for (let i = 0; i < attempts; i += 1) {
    if (!listeners(port).some((row) => row.pid === pid)) return true
    await sleep(100)
  }
  return false
}

function label(command) {
  return command.length > 120 ? `${command.slice(0, 117)}...` : command
}

/**
 * Whether nothing at all can be listening on `port`. `lsof` sees only this
 * user's processes on some systems; a bind sees everyone's.
 */
function bindable(port) {
  return new Promise((done) => {
    const server = net.createServer()
    server.once("error", () => done(false))
    server.listen({ host: "127.0.0.1", port, exclusive: true }, () =>
      server.close(() => done(true)),
    )
  })
}

/**
 * The launchd label a gateway of this run's own namespace would have
 * (`so.nessa.gateway.<stage>[.<instance>]`, as the desktop host registers it).
 */
function ownServiceLabel() {
  const instance = (process.env.NESSA_INSTANCE ?? "").trim()
  return `so.nessa.gateway.${stageName()}${instance === "" ? "" : `.${instance}`}`
}

/**
 * Make `port` free if only a dev gateway of ours is on it.
 *
 * A holder is `movable` when another port is a real way round it: anything
 * that is not a Nessa gateway, or a gateway service registered under another
 * instance. A gateway in this run's own namespace is not — it keeps the same
 * credentials, stores and published endpoint, so a second one on the next port
 * would fight it for all three, and the window would follow whichever
 * published last.
 *
 * @returns {Promise<{ text: string, movable: boolean }[]>} one entry per
 *   holder; empty when the port is free.
 */
async function clear(port) {
  /** @type {{ text: string, movable: boolean }[]} */
  const held = []
  for (const row of listeners(port)) {
    const service = launchdLabel(row.pid)
    if (service !== null) {
      const sameNamespace = service === ownServiceLabel()
      held.push({
        movable: !sameNamespace,
        text: [
          `→ :${port} is held by the launchd service ${service} (pid ${row.pid}).`,
          `  ${label(row.command)}`,
          "  launchd restarts it the moment it is killed, so nothing here will stop it.",
          "  It is a background service, not a leftover dev server. If you truly want it",
          "  gone (it is registered again the next time its app starts):",
          `    launchctl bootout gui/$(id -u)/${service}`,
          ...(sameNamespace
            ? [
                "  It serves this run's own namespace, so a second gateway beside it would share its",
                "  credentials and stores. Stop it as above, or run under another NESSA_INSTANCE.",
              ]
            : []),
        ].join("\n"),
      })
      continue
    }

    if (isOurDevServer(row)) {
      console.error(`→ stopping the dev gateway this checkout left on :${port} (pid ${row.pid})`)
      killPid(row.pid, "SIGTERM")
      if (!(await waitUntilGone(port, row.pid))) {
        killPid(row.pid, "SIGKILL")
        if (!(await waitUntilGone(port, row.pid)))
          held.push({ text: `→ could not free :${port} (pid ${row.pid})`, movable: false })
      }
      continue
    }

    // Another checkout's gateway. Its namespace cannot be read from here, and
    // by default it is this one (`~/.nessa/<stage>`), so it is not stepped
    // around.
    const gateway = isNessaServer(row.command)
    held.push({
      movable: !gateway,
      text: [
        `→ :${port} is already in use by pid ${row.pid}, which is not ours:`,
        `  ${label(row.command)}`,
        ...(gateway
          ? ["  It is a Nessa gateway, probably sharing this namespace's credentials and stores."]
          : []),
      ].join("\n"),
    })
  }
  if (held.length === 0 && !(await bindable(port)))
    held.push({ text: `→ :${port} is in use by a process this user cannot see`, movable: true })
  return held
}

/**
 * The port this run uses, or `null` when it has none.
 *
 * `clear(port)` frees a port of ours and answers why it cannot be had; `say`
 * is stderr. Kept apart from the probes above so the choice can be tested
 * without a launchd service to hold a port.
 *
 * @param {{
 *   wanted: number,
 *   explicit: boolean,
 *   move: boolean,
 *   reserved: Set<number>,
 *   clear: (port: number) => Promise<{ text: string, movable: boolean }[]>,
 *   say: (line: string) => void,
 *   stage: string,
 * }} input
 * @returns {Promise<number | null>}
 */
export async function choosePort({ wanted, explicit, move, reserved, clear, say, stage }) {
  const spares = []
  for (let port = wanted + 1; port <= wanted + MOVE_RANGE && port <= 65535; port += 1)
    if (!reserved.has(port)) spares.push(port)

  const held = await clear(wanted)
  if (held.length === 0) {
    // A gateway of ours left on a spare by an earlier moved run would share
    // this one's namespace. Moving back is the moment to stop it.
    if (move && !explicit) for (const port of spares) await clear(port)
    return wanted
  }
  for (const { text } of held) say(text)

  if (!move || explicit || held.some(({ movable }) => !movable)) {
    say(
      explicit
        ? "  NESSA_PORT names this port on purpose, so it is not moved. Stop that process, or pick another."
        : move && held.some(({ movable }) => !movable)
          ? "  So this run does not move to another port either."
          : "  Stop it yourself, or give this run a different port: NESSA_PORT=... just server",
    )
    return null
  }

  for (const port of spares) {
    // A holder we will not touch on a spare port is not worth a paragraph: the
    // next one is tried quietly. Leftovers of ours are restarted by `clear`.
    if ((await clear(port)).length > 0) continue
    say(`→ so this run's dev gateway uses :${port} instead (stage ${stage}).`)
    return port
  }

  say(
    `→ no free port in :${wanted + 1}–${wanted + MOVE_RANGE} either. Free one, or set NESSA_PORT.`,
  )
  return null
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const port = await choosePort({
    wanted: wanted(),
    explicit: override() !== "",
    move,
    reserved,
    clear,
    say: (line) => console.error(line),
    stage: stageName(),
  })
  if (port === null) process.exit(1)
  if (move) process.stdout.write(`${port}\n`)
}
