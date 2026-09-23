#!/usr/bin/env node
/**
 * Stage → gateway port, for scripts and recipes.
 *
 * The table is `protocol/defaults/gateway-ports.json` — the same file
 * `nessa-server` and the desktop host compile in and `src/env/gateway-ports.ts`
 * imports. This module exists only because a `.mjs` script cannot import that
 * TypeScript accessor; the data has one home.
 *
 *   node scripts/gateway-port.mjs [stage]   # prints the port, default dev
 */
import { readFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const table = JSON.parse(
  readFileSync(resolve(root, "protocol/defaults/gateway-ports.json"), "utf8"),
)

/**
 * @param {string} stage
 * @returns {number}
 */
export function gatewayPort(stage) {
  // `Object.hasOwn` before the read, because a plain lookup finds `toString`
  // and `constructor` on every object — including the one this argument can be
  // when it came straight off the command line.
  const port = Object.hasOwn(table.stages, stage) ? table.stages[stage] : undefined
  if (typeof port !== "number") throw new Error(`No gateway port for stage ${stage}`)
  return port
}

/**
 * The stage this environment selects, spelled the way the table names it.
 *
 * The developer recipes start a debug `nessa-server` and use `dev` when no
 * stage is named. They also export the selected stage before starting it, so a
 * recipe told `ci` cannot free one socket and then wait for health on another.
 * Release binaries have their own `prod` default for offline commands; the
 * packaged desktop passes its resolved stage explicitly.
 */
export function selectedStage(env = process.env) {
  // An empty variable is how a shell spells unset — `NESSA_STAGE= just start` —
  // and is the dev stage like an absent one.
  const named = (env.NESSA_STAGE ?? "").trim()
  if (named === "") return "dev"
  // Matched exactly, because `Stage::parse` in the server matches exactly. A
  // rule more forgiving than the server's is worse than a stricter one: it
  // reads `Dev` as dev and frees and probes 7421, while the server refuses to
  // start at all and the host's launchd registration finds no port for `Dev`.
  // One value, three different failures, and none of them says what is wrong.
  // `Object.hasOwn`, not `in`: `in` walks the prototype chain, so `toString`
  // and `constructor` are in every table and `NESSA_STAGE=constructor` came out
  // of here as a stage. Kept identical to `parseStage` in
  // `src/env/gateway-ports.ts`, which is the same rule for the same variable.
  if (!Object.hasOwn(table.stages, named))
    throw new Error(
      `NESSA_STAGE=${named} is not a stage. ` +
        `The server accepts exactly: ${Object.keys(table.stages).join(", ")}.`,
    )
  return named
}

/**
 * The port this environment's gateway actually listens on: the stage's, unless
 * `NESSA_PORT` overrides it, which is what the server itself does.
 */
export function selectedPort(env = process.env) {
  const override = (env.NESSA_PORT ?? "").trim()
  if (override !== "") {
    const port = Number(override)
    if (!Number.isInteger(port) || port <= 0 || port > 65535)
      throw new Error(`NESSA_PORT is not a port: ${override}`)
    return port
  }
  return gatewayPort(selectedStage(env))
}

// A named stage still wins, for a caller that means a specific one. With no
// argument the answer is this environment's — the stage the server will read,
// and `NESSA_PORT` ahead of it — so a recipe and the server it starts cannot
// name different sockets.
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const named = process.argv[2]
  process.stdout.write(`${named ? gatewayPort(named) : selectedPort()}\n`)
}
