#!/usr/bin/env node
/**
 * End-to-end smoke: Rust nessa-server + @nessa/client connect + server.health.
 */
import { spawn, spawnSync } from "node:child_process"
import { setTimeout as sleep } from "node:timers/promises"
import { fileURLToPath } from "node:url"
import { dirname, join } from "node:path"
import { mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"

const root = join(dirname(fileURLToPath(import.meta.url)), "..")
const port = 19_421
const directory = mkdtempSync(join(tmpdir(), "nessa-session-smoke-"))
const serverEnv = {
  ...process.env,
  NESSA_STAGE: "ci",
  NESSA_PORT: String(port),
  NESSA_DATA_DIR: directory,
  NESSA_INSTANCE: "session-smoke",
}
const init = spawnSync(
  "cargo",
  [
    "run",
    "-q",
    "-p",
    "nessa-server",
    "--",
    "auth",
    "init",
    "--owner-token-file",
    join(directory, "owner.token"),
  ],
  { cwd: root, env: serverEnv, encoding: "utf8" },
)
if (init.status !== 0) {
  rmSync(directory, { recursive: true, force: true })
  throw new Error(`local smoke bootstrap failed: ${init.stderr}`)
}

async function waitForHealth(url, attempts = 40) {
  for (let i = 0; i < attempts; i += 1) {
    try {
      const res = await fetch(url)
      if (res.ok) return
    } catch {
      // server still starting
    }
    await sleep(100)
  }
  throw new Error(`server did not become healthy at ${url}`)
}

const server = spawn("cargo", ["run", "-q", "-p", "nessa-server"], {
  cwd: root,
  env: serverEnv,
  stdio: ["ignore", "pipe", "pipe"],
})

server.stderr.on("data", (chunk) => process.stderr.write(chunk))

let failed = false
try {
  await waitForHealth(`http://127.0.0.1:${port}/health`)

  const { NessaClient } = await import("@nessa/client")
  const client = await NessaClient.connect({
    stage: "ci",
    url: `ws://127.0.0.1:${port}`,
    role: "surface",
    surface: { kind: "panel", instance: "smoke" },
    client: { id: "smoke", version: "0.1.0", platform: "node" },
    profile: "product",
    auth: { credential: readFileSync(join(directory, "owner.token"), "utf8").trim() },
  })

  const health = await client.server.health()
  if (!health.ok || health.runtimeStatus !== "ready") {
    throw new Error(`unexpected health payload: ${JSON.stringify(health)}`)
  }

  client.close()
  console.log("smoke-connect passed")
} catch (error) {
  failed = true
  console.error("smoke-connect failed:", error instanceof Error ? error.message : error)
} finally {
  server.kill("SIGTERM")
  if (server.exitCode === null)
    await new Promise((resolve) => server.once("exit", resolve))
  rmSync(directory, { recursive: true, force: true })
}

process.exit(failed ? 1 : 0)
