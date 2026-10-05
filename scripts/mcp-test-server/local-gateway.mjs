/**
 * A real gateway for a live check: provisioned in a temporary `ci` namespace
 * of its own, with one agent runtime and the test MCP server configured as
 * `mcptest` (or the servers it is given), started on 127.0.0.1, and stopped
 * and removed again — its owner token with it. `live-check.mjs` and the
 * desktop's real-gateway checks (`verification/desktop/scripts/lib/gateway-stack.mjs`)
 * start theirs here.
 *
 * It uses whatever sign-in the agent already has on this machine, or, started
 * `signedOut`, none at all (`signedOutEnvironment`). It writes no credential
 * of its own beyond the gateway's owner token, which stays in the temporary
 * directory and is never printed.
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

/** The model each agent runs in a check against a local gateway, by agent. */
export const MODELS = {
  claude: "claude-sonnet-5",
  codex: "gpt-5.6-terra",
  opencode: "opencode/nemotron-3-ultra-free",
}

/**
 * What a check asks the agent to do: call each of `tools` (`{ name, args }`,
 * `args` an object or undefined) once, in order, then reply DONE. Codex
 * defers MCP tools behind its tool search (#500), so the prompt does not
 * forbid the agent's own tools, only other tools of the server. The prompt
 * enforces nothing; what each check enforces is its own, and a harness may
 * run a tool without asking (Codex ran `review_rows`, which declares
 * `readOnlyHint`, unasked). The single-tool wording is the
 * prompt of the recorded turn in the SDK's Codex fixture
 * (`toolSearchTurn`), held to it by `local-gateway.test.mjs`.
 */
export function toolPrompt(tools) {
  if (tools.length === 0) throw new Error("toolPrompt needs at least one tool")
  // Arguments as JSON, a space after each top-level colon: `{"id": 2}`.
  const json = (args) =>
    `{${Object.entries(args)
      .map(([key, value]) => `${JSON.stringify(key)}: ${JSON.stringify(value)}`)
      .join(", ")}}`
  const call = ({ name, args }) =>
    args === undefined || Object.keys(args).length === 0
      ? `${name} (no arguments)`
      : `${name} with ${json(args)}`
  const find =
    `Use the tools of the "${SERVER}" MCP server. ` +
    "If they are not among the tools you were given, find them with your tool search."
  if (tools.length === 1)
    return (
      `${find} Call ${call(tools[0])} exactly once, and wait for its result. ` +
      "Call no other tool of that server. When it has returned, reply with DONE."
    )
  return (
    `${find} Call each of these exactly once, in this order, waiting for each result ` +
    `before the next: ${tools.map(call).join(", ")}. Call no other tool of that server. ` +
    `When all ${tools.length} have returned, reply with DONE.`
  )
}

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
        model: MODELS.claude,
        path: null,
      }
    case "codex":
      return {
        argv: [
          process.execPath,
          entry("codex-acp", "@agentclientprotocol/codex-acp/dist/index.js"),
        ],
        model: MODELS.codex,
        path: join(harnesses, "codex-acp", "node_modules", ".bin"),
      }
    case "opencode": {
      const binary = process.env.MCP_LIVE_OPENCODE
      if (!binary || !existsSync(binary))
        throw new Error("set MCP_LIVE_OPENCODE to an Opencode 1.18.31 binary")
      return {
        argv: [binary, "acp"],
        model: MODELS.opencode,
        path: null,
      }
    }
    default:
      throw new Error(`unknown agent ${agent}`)
  }
}

/**
 * What a signed-out gateway is started with, over `NESSA_*`: this process's
 * `PATH` (`path` first, when given), `TMPDIR` and `RUST_LOG`, and a `HOME` of
 * its own under `directory`, so neither a credential variable nor a sign-in
 * kept under the real home reaches it or the agent it starts. Built from
 * what it needs rather than by removing the credentials it might find.
 * `ANTHROPIC_API_KEY` is a placeholder: with no credential in the
 * environment, the gateway reads Claude's from the login keychain
 * (`crates/nessa-server/src/agents/infrastructure/agent_credentials.rs`).
 */
export function signedOutEnvironment(directory, path) {
  const kept = Object.fromEntries(
    ["TMPDIR", "RUST_LOG"]
      .filter((name) => process.env[name] !== undefined)
      .map((name) => [name, process.env[name]]),
  )
  return {
    ...kept,
    PATH: path ? `${path}:${process.env.PATH}` : process.env.PATH,
    HOME: join(directory, "home"),
    ANTHROPIC_API_KEY: "signed-out-gateway-placeholder",
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
 * @param {{ command: string, args: string[] }} [o.mcpServer] how the gateway starts `mcptest`
 * @param {object[]} [o.mcpServers] the stored servers the gateway starts with,
 *   as `agents.mcpServers` holds them; by default `mcptest` alone, started as
 *   `o.mcpServer` says. `[]` starts it with none (the desktop's Settings check,
 *   `verification/desktop/scripts/mcp-servers-gateway.mjs`, adds its own).
 * @param {boolean} [o.signedOut] start the gateway with no sign-in to hand an
 *   agent (`signedOutEnvironment`), for an agent that needs none
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
    if (o.signedOut) mkdirSync(join(directory, "home"))
    const env = {
      ...(o.signedOut
        ? signedOutEnvironment(directory, o.path)
        : {
            ...process.env,
            ...(o.path ? { PATH: `${o.path}:${process.env.PATH}` } : {}),
          }),
      NESSA_STAGE: "ci",
      NESSA_PORT: String(o.port),
      NESSA_DATA_DIR: directory,
      NESSA_INSTANCE: o.instance,
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
          mcpServers: o.mcpServers ?? [{ name: SERVER, ...o.mcpServer }],
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
