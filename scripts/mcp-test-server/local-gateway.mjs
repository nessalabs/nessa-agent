/**
 * A real gateway for a live check: provisioned in a temporary `ci` namespace
 * of its own, with one agent runtime and the test MCP server configured as
 * `mcptest`, started on 127.0.0.1, and stopped and removed again — its owner
 * token with it. `live-check.mjs` and the desktop's real-gateway checks
 * (`verification/desktop/scripts/lib/gateway-stack.mjs`) start theirs here.
 *
 * It uses whatever sign-in the agent already has on this machine and writes
 * no credential of its own beyond the gateway's owner token, which stays in
 * the temporary directory and is never printed.
 */
import { spawn, spawnSync } from "node:child_process"
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { setTimeout as sleep } from "node:timers/promises"
import { fileURLToPath } from "node:url"

const here = dirname(fileURLToPath(import.meta.url))
export const repoRoot = resolve(here, "../..")
/** The name the test server is configured under. */
export const SERVER = "mcptest"
/** The test MCP server itself. */
export const serverScript = join(here, "server.mjs")

/**
 * How to start `agent`'s harness: its argv, the model it runs, and a
 * directory to put first on its `PATH` (or `null`). `MCP_LIVE_HARNESSES`
 * names the directory holding `claude-acp/` and `codex-acp/`.
 */
export function agentCommand(agent) {
  const harnesses =
    process.env.MCP_LIVE_HARNESSES ?? join(repoRoot, "crates/nessa-sdk/harnesses")
  const entry = (name, path) => {
    const file = join(harnesses, name, "node_modules", path)
    if (!existsSync(file)) throw new Error(`no ${name} harness at ${file}`)
    return file
  }
  switch (agent) {
    case "claude":
      return {
        argv: [
          process.execPath,
          entry("claude-acp", "@agentclientprotocol/claude-agent-acp/dist/index.js"),
        ],
        model: "claude-sonnet-5",
        path: null,
      }
    case "codex":
      return {
        argv: [
          process.execPath,
          entry("codex-acp", "@agentclientprotocol/codex-acp/dist/index.js"),
        ],
        model: "gpt-5.6-terra",
        path: join(harnesses, "codex-acp", "node_modules", ".bin"),
      }
    case "opencode": {
      const binary = process.env.MCP_LIVE_OPENCODE
      if (!binary || !existsSync(binary))
        throw new Error("set MCP_LIVE_OPENCODE to an Opencode 1.18.31 binary")
      return {
        argv: [binary, "acp"],
        model: "opencode/nemotron-3-ultra-free",
        path: null,
      }
    }
    default:
      throw new Error(`unknown agent ${agent}`)
  }
}

/** Wait up to `ms` for `child` to exit; whether it did. */
export async function exited(child, ms) {
  if (child.exitCode !== null || child.signalCode !== null) return true
  return Promise.race([
    new Promise((done) => child.once("exit", () => done(true))),
    // Unref'd: once the race is decided, the losing timer holds nothing open.
    sleep(ms, false, { ref: false }),
  ])
}

/**
 * Provisions and starts a gateway, and waits until it is healthy.
 *
 * @param {object} o
 * @param {string} o.agent the one agent configured, and selected
 * @param {number} o.port the gateway's port on 127.0.0.1
 * @param {string} o.instance the gateway instance's name
 * @param {string[]} o.agentArgv how the gateway starts the agent's harness
 *   (`agentCommand(agent).argv`, possibly wrapped)
 * @param {string} o.model the agent's model
 * @param {string|null} [o.path] a directory to put first on the gateway's `PATH`
 * @param {{ command: string, args: string[] }} o.mcpServer how the gateway starts `mcptest`
 * @returns the gateway: `{ directory, token, url, log, server, stop }`. `token`
 *   is the owner token file's path; `log()` the gateway's output so far; `stop()`
 *   stops it, waits for it and for the rest of its output (up to 2 s more), and
 *   removes its directory, and says whether it exited. On a failed start, the
 *   error carries the gateway's output as `gatewayLog`.
 */
export async function startLocalGateway(o) {
  const nessa = process.env.MCP_LIVE_NESSA ?? join(repoRoot, "target/debug/nessa")
  if (!existsSync(nessa))
    throw new Error(
      "build the gateway first (cargo build -p nessa-server), or set MCP_LIVE_NESSA",
    )
  const directory = mkdtempSync(join(tmpdir(), `nessa-mcp-live-${o.agent}-`))
  const output = []
  let server = null
  // Its output streams closed: everything it wrote has been read.
  let closed = null
  const stop = async () => {
    let stopped = true
    // Stop the gateway, and wait for it, before its directory is removed: it
    // stops the agent and the MCP server it started, and its last words go
    // in the log.
    if (server) {
      server.kill("SIGTERM")
      if (!(await exited(server, 10_000))) {
        server.kill("SIGKILL")
        stopped = await exited(server, 5_000)
      }
      // Its exit can arrive before the last of its output is read. A child
      // that kept the streams open does not hold the stop up for long.
      if (stopped) await Promise.race([closed, sleep(2_000, undefined, { ref: false })])
    }
    rmSync(directory, { recursive: true, force: true })
    return stopped
  }
  try {
    const workspace = join(directory, "workspace")
    mkdirSync(workspace)
    const env = {
      ...process.env,
      NESSA_STAGE: "ci",
      NESSA_PORT: String(o.port),
      NESSA_DATA_DIR: directory,
      NESSA_INSTANCE: o.instance,
      ...(o.path ? { PATH: `${o.path}:${process.env.PATH}` } : {}),
    }
    const token = join(directory, "owner.token")
    const init = spawnSync(
      nessa,
      ["auth", "init", "--local", "--owner-token-file", token],
      { env, encoding: "utf8" },
    )
    if (init.status !== 0) throw new Error(`provisioning failed: ${init.stderr}`)
    const configPath = join(directory, "ci", "instances", o.instance, "config.json")
    writeFileSync(
      configPath,
      JSON.stringify({
        agents: {
          catalog: join(repoRoot, "crates/nessa-sdk/data/models.json"),
          workspace,
          selected: o.agent,
          mcpServers: [{ name: SERVER, ...o.mcpServer }],
          runtimes: {
            [o.agent]: {
              command: o.agentArgv[0],
              args: o.agentArgv.slice(1),
              model: o.model,
              toolsEnabled: true,
            },
          },
        },
      }),
    )
    chmodSync(configPath, 0o600)
    server = spawn(nessa, ["server"], { env, stdio: ["ignore", "pipe", "pipe"] })
    closed = new Promise((done) => server.once("close", done))
    for (const stream of [server.stdout, server.stderr])
      stream.on("data", (chunk) => output.push(chunk.toString()))
    const log = () => output.join("")
    for (let i = 0; ; i += 1) {
      // A gateway that already exited is not the one answering on the port.
      if (server.exitCode !== null)
        throw new Error(`gateway exited (${server.exitCode}):\n${log().slice(-4000)}`)
      try {
        if ((await fetch(`http://127.0.0.1:${o.port}/health`)).ok) break
      } catch {}
      if (i > 240) throw new Error(`gateway never became healthy:\n${log().slice(-4000)}`)
      await sleep(250)
    }
    return { directory, token, url: `http://127.0.0.1:${o.port}`, log, server, stop }
  } catch (error) {
    await stop()
    error.gatewayLog = output.join("")
    throw error
  }
}
