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
  const port = table.stages[stage]
  if (typeof port !== "number") throw new Error(`No gateway port for stage ${stage}`)
  return port
}

/**
 * The stage this environment selects, spelled the way the table names it.
 *
 * `nessa-server` reads `NESSA_STAGE` and treats it as `dev` when it says
 * nothing, so anything resolving a port for that server has to agree — a
 * recipe that assumes `dev` while the server is told `ci` frees one socket and
 * then waits for health on another.
 */
export function selectedStage(env = process.env) {
  // An empty variable is how a shell spells unset — `NESSA_STAGE= just start` —
  // and is the dev stage like an absent one.
  const stage = (env.NESSA_STAGE ?? "").trim().toLowerCase() || "dev"
  if (!(stage in table.stages)) throw new Error(`No gateway port for stage ${stage}`)
  return stage
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
