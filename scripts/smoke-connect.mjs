#!/usr/bin/env node
/**
 * End-to-end smoke: Rust nessa-server + @nessa/client connect + server.health.
 */
import { spawn } from "node:child_process"
import { setTimeout as sleep } from "node:timers/promises"
import { fileURLToPath } from "node:url"
import { dirname, join } from "node:path"

const root = join(dirname(fileURLToPath(import.meta.url)), "..")
const port = 19_421
const token = "smoke-token"

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
  env: {
    ...process.env,
    NESSA_STAGE: "ci",
    NESSA_PORT: String(port),
    NESSA_TOKEN: token,
  },
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
    auth: { token },
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
  await new Promise((resolve) => server.once("exit", resolve))
}

process.exit(failed ? 1 : 0)
