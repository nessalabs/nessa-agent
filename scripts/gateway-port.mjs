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

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  process.stdout.write(`${gatewayPort(process.argv[2] ?? "dev")}\n`)
}
