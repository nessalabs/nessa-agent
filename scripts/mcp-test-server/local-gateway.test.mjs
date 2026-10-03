/**
 * The gateway's log, against a stand-in for the `nessa` binary
 * (`MCP_LIVE_NESSA`): what the gateway says as it stops is in `log()` once
 * `stop()` returns, and a gateway that fails to start leaves its output on
 * the error. `live-check.mjs` writes either to its evidence.
 */
import { strict as assert } from "node:assert"
import { spawnSync } from "node:child_process"
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { createServer } from "node:net"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { after, test } from "node:test"
import { fileURLToPath } from "node:url"

import { startLocalGateway } from "./local-gateway.mjs"

const scratch = mkdtempSync(join(tmpdir(), "nessa-local-gateway-test-"))
after(() => rmSync(scratch, { recursive: true, force: true }))

/**
 * A stand-in `nessa`: `auth init` makes the instance directory, and `server`
 * answers `/health`. Told to stop, it exits at once, leaving a child of its
 * own — as the gateway leaves the agent and the MCP server stopping — that
 * writes its last words to the same output 300 ms later, so its exit
 * arrives first unless the parent stalls for those 300 ms. With
 * `FAKE_NESSA_FAIL`, `server` says so and exits at once.
 */
const FAKE = `#!${process.execPath}
const { mkdirSync } = require("node:fs")
const { join } = require("node:path")
const [command] = process.argv.slice(2)
if (command === "auth") {
  mkdirSync(join(process.env.NESSA_DATA_DIR, "ci", "instances", process.env.NESSA_INSTANCE), { recursive: true })
  process.exit(0)
}
if (process.env.FAKE_NESSA_FAIL) {
  process.stderr.write("could not bind\\n", () => process.exit(3))
} else {
  const server = require("node:http")
    .createServer((request, response) => response.end("ok"))
    .listen(Number(process.env.NESSA_PORT), "127.0.0.1")
  process.stdout.write("started\\n")
  process.on("SIGTERM", () => {
    server.close()
    require("node:child_process").spawn(
      process.execPath,
      ["-e", "setTimeout(() => process.stdout.write('last words\\\\n'), 300)"],
      { stdio: ["ignore", "inherit", "inherit"] },
    )
    process.exit(0)
  })
}
`

const nessa = join(scratch, "nessa")
writeFileSync(nessa, FAKE)
chmodSync(nessa, 0o755)

async function freePort() {
  const server = createServer().listen(0, "127.0.0.1")
  await new Promise((done) => server.once("listening", done))
  const { port } = server.address()
  await new Promise((done) => server.close(done))
  return port
}

const start = async () =>
  startLocalGateway({
    agent: "claude",
    port: await freePort(),
    instance: "local-gateway-test",
    agentArgv: [process.execPath],
    model: "none",
    mcpServer: { command: process.execPath, args: [] },
  })

test("once stop() returns, the log holds what the gateway said as it stopped", async (t) => {
  t.after(() => delete process.env.MCP_LIVE_NESSA)
  process.env.MCP_LIVE_NESSA = nessa
  const gateway = await start()
  assert.ok(gateway.log().startsWith("started\n"))
  assert.equal(await gateway.stop(), true)
  assert.ok(gateway.log().endsWith("\nlast words\n"), gateway.log().slice(-40))
})

test("a gateway that fails to start leaves its output on the error", async (t) => {
  t.after(() => {
    delete process.env.MCP_LIVE_NESSA
    delete process.env.FAKE_NESSA_FAIL
  })
  process.env.MCP_LIVE_NESSA = nessa
  process.env.FAKE_NESSA_FAIL = "1"
  await assert.rejects(start(), (error) => error.gatewayLog === "could not bind\n")
})

/**
 * `live-check.mjs claude` against the stand-in, which speaks no WebSocket, so
 * the check fails just after the gateway starts — or, with `fail`, as it
 * starts. Returns the `gateway.log` it wrote to its evidence.
 */
async function liveCheckLog(fail) {
  // The harness need only exist: the stand-in never starts it.
  const harnesses = join(scratch, "harnesses")
  const entry = join(
    harnesses,
    "claude-acp/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js",
  )
  mkdirSync(dirname(entry), { recursive: true })
  writeFileSync(entry, "")
  const out = mkdtempSync(join(scratch, "evidence-"))
  const here = dirname(fileURLToPath(import.meta.url))
  const run = spawnSync(
    join(here, "../../node_modules/.bin/tsx"),
    [join(here, "live-check.mjs"), "claude", out],
    {
      encoding: "utf8",
      timeout: 60_000,
      env: {
        ...process.env,
        MCP_LIVE_NESSA: nessa,
        MCP_LIVE_HARNESSES: harnesses,
        MCP_LIVE_PORT: String(await freePort()),
        ...(fail ? { FAKE_NESSA_FAIL: "1" } : {}),
      },
    },
  )
  assert.equal(run.status, 1, run.stderr)
  return readFileSync(join(out, "claude", "gateway.log"), "utf8")
}

test("the live check's gateway.log is read after the stop, so it holds the gateway's last words", async () => {
  const log = await liveCheckLog(false)
  assert.ok(log.startsWith("started\n"), log)
  assert.ok(log.endsWith("\nlast words\n"), log)
})

test("the live check's gateway.log holds the output of a gateway that failed to start", async () => {
  assert.equal(await liveCheckLog(true), "could not bind\n")
})
