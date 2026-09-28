/** Packaged gateway + real native downloads + slim bundled adapters, in a private namespace.
 * Run with: pnpm exec tsx scripts/agents/smoke-native-downloads.mjs /absolute/runtime
 * Uses the debug gateway and network; does not issue a model request.
 */
import assert from "node:assert/strict"
import { spawn, spawnSync } from "node:child_process"
import { randomBytes, randomUUID } from "node:crypto"
import { once } from "node:events"
import { mkdtempSync, mkdirSync, readFileSync, rmSync } from "node:fs"
import { createServer } from "node:net"
import { tmpdir } from "node:os"
import { join, resolve } from "node:path"
import { setTimeout as sleep } from "node:timers/promises"
import { WebSocket } from "ws"
import { NessaClient } from "@nessa/client"
import { cargoTargetDirectory } from "../cargo-target.mjs"
import { verifyRuntimeFingerprint } from "../desktop/runtime-fingerprint.mjs"

const root = resolve(import.meta.dirname, "../..")
const runtime = resolve(process.argv[2])
const fingerprint = verifyRuntimeFingerprint(runtime)
const agents = process.argv.length > 3 ? process.argv.slice(3) : ["claude", "codex"]
assert.ok(agents.every((agent) => ["claude", "codex"].includes(agent)))
const directory = mkdtempSync(join(tmpdir(), "nessa-native-downloads-"))
const binary = join(cargoTargetDirectory(root), "debug/nessa")
const listener = createServer().listen(0, "127.0.0.1")
await once(listener, "listening")
const port = listener.address().port
await new Promise((done) => listener.close(done))
const env = {
  ...process.env,
  HOME: directory,
  CODEX_HOME: join(directory, "codex"),
  CLAUDE_CONFIG_DIR: join(directory, "claude"),
  NESSA_STAGE: "ci",
  NESSA_DATA_DIR: join(directory, "data"),
  NESSA_INSTANCE: "native-downloads",
  NESSA_PORT: String(port),
  NESSA_HOST: "127.0.0.1",
  NESSA_RUNTIME_FINGERPRINT: fingerprint,
  NESSA_SERVICE_GENERATION: randomBytes(32).toString("hex"),
}
for (const path of [env.CODEX_HOME, env.CLAUDE_CONFIG_DIR])
  mkdirSync(path, { mode: 0o700 })
const owner = join(directory, "owner.token")
const initialized = spawnSync(
  binary,
  ["auth", "init", "--local", "--owner-token-file", owner],
  { env, encoding: "utf8" },
)
assert.equal(initialized.status, 0, initialized.stderr)
globalThis.WebSocket = WebSocket
let server
let client
let logs = ""
async function start() {
  server = spawn(binary, ["server", "--desktop-runtime", runtime], {
    env,
    stdio: ["ignore", "pipe", "pipe"],
  })
  server.stdout.on("data", (bytes) => {
    logs += bytes
  })
  server.stderr.on("data", (bytes) => {
    logs += bytes
  })
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      if ((await fetch(`http://127.0.0.1:${port}/health`)).ok) break
    } catch {
      /* startup */
    }
    assert.equal(server.exitCode, null, logs)
    await sleep(100)
  }
  client = await NessaClient.connect({
    stage: "ci",
    url: `ws://127.0.0.1:${port}`,
    role: "surface",
    surface: { kind: "panel", instance: "native-downloads" },
    client: { id: "native-downloads", version: "0.1.0", platform: "node" },
    profile: "product",
    auth: { credential: readFileSync(owner, "utf8").trim() },
  })
}
async function stop() {
  client?.close()
  if (!server || server.exitCode !== null) return
  const exited = once(server, "exit")
  server.kill("SIGTERM")
  await exited
}
async function initializeAdapter(agent, executable) {
  const entry = join(
    runtime,
    `${agent}-acp/node_modules/@agentclientprotocol/${agent === "claude" ? "claude-agent-acp" : "codex-acp"}/dist/index.js`,
  )
  const child = spawn(join(runtime, "node"), [entry], {
    env: {
      ...env,
      [agent === "claude" ? "CLAUDE_CODE_EXECUTABLE" : "CODEX_PATH"]: executable,
    },
    detached: true,
    stdio: ["pipe", "pipe", "pipe"],
  })
  let errors = ""
  child.stderr.on("data", (bytes) => {
    errors += bytes
  })
  const answer = new Promise((done, reject) => {
    let buffer = ""
    child.stdout.on("data", (bytes) => {
      buffer += bytes
      let end
      while ((end = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, end)
        buffer = buffer.slice(end + 1)
        try {
          const message = JSON.parse(line)
          if (message.id === 1) done(message)
        } catch {
          /* diagnostics */
        }
      }
    })
    child.on("error", reject)
    child.on("exit", (code) => reject(new Error(`${agent} exited ${code}: ${errors}`)))
  })
  child.stdin.write(
    JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: {
        protocolVersion: 1,
        clientCapabilities: {},
        clientInfo: { name: "nessa-native-smoke", version: "1" },
      },
    }) + "\n",
  )
  let timer
  try {
    const result = await Promise.race([
      answer,
      new Promise((_, reject) => {
        timer = setTimeout(
          () => reject(new Error(`${agent} initialization timed out: ${errors}`)),
          20_000,
        )
      }),
    ])
    assert.ok(result.result, JSON.stringify(result))
  } finally {
    clearTimeout(timer)
    const exited = child.exitCode === null ? once(child, "exit") : Promise.resolve()
    try {
      process.kill(-child.pid, "SIGKILL")
    } catch (error) {
      if (error.code !== "ESRCH") throw error
    }
    await exited
  }
}
try {
  await start()
  const initial = await client.agents.installOptions()
  for (const agent of agents) {
    assert.equal(initial.agents.find((offer) => offer.agent === agent)?.installed, false)
    const installed = await client.agents.install(agent, randomUUID())
    assert.equal(installed.agent, agent)
    assert.equal(installed.downloaded, true)
    const checked = await client.agents.installOptions()
    assert.equal(checked.agents.find((offer) => offer.agent === agent)?.installed, true)
    // The CLI shares the OS owner and exact managed publication with the gateway.
    const reused = spawnSync(binary, ["install-agent", agent], {
      env,
      encoding: "utf8",
      timeout: 60_000,
    })
    assert.equal(reused.status, 0, reused.stderr)
    const report = JSON.parse(reused.stdout)
    assert.equal(report.downloaded, false)
    const version = spawnSync(report.executable, ["--version"], {
      env,
      encoding: "utf8",
      timeout: 20_000,
    })
    assert.equal(version.status, 0, version.stderr)
    await initializeAdapter(agent, report.executable)
    console.log(
      `${agent}: downloaded, verified, reused by CLI, native version and slim ACP initialization passed`,
    )
  }
  await stop()
  await start()
  const restored = await client.agents.installOptions()
  for (const agent of agents)
    assert.equal(restored.agents.find((offer) => offer.agent === agent)?.installed, true)
  console.log("native-download smoke passed; installations survive gateway restart")
} catch (error) {
  console.error(logs)
  throw error
} finally {
  await stop()
  rmSync(directory, { recursive: true, force: true })
}
