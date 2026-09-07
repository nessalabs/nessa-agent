/** Reproducible local gateway/SDK bounds probe; never seeds or edits registry records. */
import assert from "node:assert/strict"
import { spawn, spawnSync } from "node:child_process"
import { once } from "node:events"
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs"
import { tmpdir, cpus, platform, release } from "node:os"
import { join } from "node:path"
import { createServer } from "node:net"
import { setTimeout as sleep } from "node:timers/promises"
import { fileURLToPath } from "node:url"
import { WebSocket } from "ws"
import { NessaClient, NessaMutationError, NessaRpcError } from "@nessa/client"

globalThis.WebSocket = WebSocket
const root = fileURLToPath(new URL("../", import.meta.url))
const directory = mkdtempSync(join(tmpdir(), "nessa-auth-bounds-"))
const binary = join(
  root,
  "target/debug",
  process.platform === "win32" ? "nessa-server.exe" : "nessa-server",
)
const listener = createServer().listen(0, "127.0.0.1")
await once(listener, "listening")
const port = listener.address().port
await new Promise((resolve) => listener.close(resolve))
const env = {
  ...process.env,
  NESSA_DATA_DIR: directory,
  NESSA_STAGE: "ci",
  NESSA_INSTANCE: "auth-bounds",
  NESSA_HOST: "127.0.0.1",
  NESSA_PORT: String(port),
}
const ownerPath = join(directory, "owner.token")
const registryPath = join(directory, "ci/instances/auth-bounds/auth/credentials.v1.json")
const clients = []
let server
let logs = ""
let peakRssKiB = 0
let sampler
const samples = []
const report = {
  machine: {
    platform: platform(),
    release: release(),
    cpu: cpus()[0].model,
    logicalCpus: cpus().length,
    node: process.version,
  },
  build: "debug",
  config: "defaults: 1000 credentials, 4 MiB registry, 2000 receipts per collection",
  samples,
}
function memory() {
  if (process.platform === "win32") return null
  const result = spawnSync("ps", ["-o", "rss=", "-p", String(server.pid)], {
    encoding: "utf8",
  })
  const rss = Number(result.stdout.trim())
  if (result.status === 0 && Number.isFinite(rss)) peakRssKiB = Math.max(peakRssKiB, rss)
  return rss
}
function summarize(name, values, extra = {}) {
  const sorted = [...values].sort((a, b) => a - b)
  const percentile = (p) => +sorted[Math.ceil(sorted.length * p) - 1].toFixed(2)
  samples.push({
    name,
    n: values.length,
    p50Ms: percentile(0.5),
    p95Ms: percentile(0.95),
    maxMs: percentile(1),
    serverRssKiB: memory(),
    ...extra,
  })
}
async function timed(values, action) {
  const start = performance.now()
  const result = await action()
  values.push(performance.now() - start)
  return result
}
try {
  const init = spawnSync(binary, ["auth", "init", "--owner-token-file", ownerPath], {
    env,
    encoding: "utf8",
  })
  assert.equal(init.status, 0, init.stderr)
  const secret = readFileSync(ownerPath, "utf8").trim()
  server = spawn(binary, [], { cwd: root, env, stdio: ["ignore", "pipe", "pipe"] })
  server.stdout.on("data", (bytes) => {
    logs += bytes
  })
  server.stderr.on("data", (bytes) => {
    logs += bytes
  })
  let ready = false
  for (let i = 0; i < 100; i++) {
    assert.equal(server.exitCode, null, logs)
    try {
      if ((await fetch(`http://127.0.0.1:${port}/health`)).ok) {
        ready = true
        break
      }
    } catch {
      /* startup */
    }
    await sleep(50)
  }
  assert.ok(ready, "gateway startup timed out")
  sampler = setInterval(memory, 250)
  const connect = async () => {
    const client = await NessaClient.connect({
      url: `ws://127.0.0.1:${port}`,
      stage: "ci",
      profile: "product",
      role: "surface",
      surface: { kind: "cli", instance: "bounds" },
      client: { id: "bounds", version: "0.1.0", platform: "node" },
      auth: { credential: secret },
    })
    clients.push(client)
    return client
  }
  const owner = await connect()
  const identity = await owner.auth.session()
  let count = 2
  const request = (id) => ({
    requestId: `issue-${id}`,
    principal: { id: `reader-${id}`, kind: "integration" },
    membership: {
      id: `membership-${id}`,
      principalId: `reader-${id}`,
      organizationId: identity.organizationId,
      role: "member",
      state: "active",
    },
    grants: [
      {
        action: "server.read",
        resource: { organizationId: identity.organizationId, id: identity.gatewayId },
      },
    ],
  })
  const fill = async (target) => {
    const times = []
    while (count < target) {
      await timed(times, () => owner.credentials.issue(request(count)))
      count++
    }
    summarize(`fill-to-${target}`, times)
  }
  const list = async (size) => {
    const times = []
    let bytes
    for (let i = 0; i < 30; i++) {
      const result = await timed(times, () => owner.credentials.list())
      assert.equal(result.credentials.length, size)
      assert.ok(result.credentials.every((item) => !("secret" in item)))
      bytes = Buffer.byteLength(JSON.stringify(result))
    }
    summarize(`list-${size}`, times, {
      responseJsonBytes: bytes,
      registryBytes: statSync(registryPath).size,
    })
  }
  await fill(100)
  await list(100)
  await fill(500)
  await list(500)
  await fill(900)
  for (const connections of [1, 16, 64]) {
    const connectionTimes = []
    await Promise.all(
      Array.from({ length: connections - clients.length }, () =>
        timed(connectionTimes, connect),
      ),
    )
    if (connectionTimes.length) summarize(`connect-to-${connections}`, connectionTimes)
    const healthTimes = []
    await Promise.all(
      clients.map(async (client) => {
        for (let i = 0; i < 20; i++)
          assert.equal((await timed(healthTimes, () => client.server.health())).ok, true)
      }),
    )
    summarize(`health-${connections}-connections`, healthTimes)
  }
  const issueTimes = [],
    revokeTimes = [],
    healthTimes = []
  const start = performance.now()
  let writersRemaining = 8
  await Promise.all([
    ...clients.slice(0, 8).map(async (client, index) => {
      for (let i = 0; i < 8; i++) {
        const id = 900 + index * 8 + i
        const issued = await timed(issueTimes, () =>
          client.credentials.issue(request(id)),
        )
        await timed(revokeTimes, () =>
          client.credentials.revoke(issued.credential.id, `revoke-${id}`),
        )
      }
      writersRemaining--
    }),
    ...clients.slice(8).map(async (client) => {
      for (let i = 0; i < 30 || writersRemaining > 0; i++) {
        await sleep(10)
        assert.equal((await timed(healthTimes, () => client.server.health())).ok, true)
      }
    }),
  ])
  count += 64
  summarize("issue-8-writers", issueTimes)
  summarize("revoke-8-writers", revokeTimes)
  summarize("health-during-mutations-56-readers", healthTimes, {
    workloadWallMs: +(performance.now() - start).toFixed(2),
  })
  await fill(1000)
  await list(1000)
  await assert.rejects(
    owner.credentials.issue(request(1000)),
    (error) =>
      error instanceof NessaMutationError &&
      error.requestId === "issue-1000" &&
      error.cause instanceof NessaRpcError &&
      error.cause.code === "credential_capacity",
  )
  const retry = await owner.credentials.issue(request(900))
  assert.equal(retry.secretUnavailable, true)
  assert.ok(!("secret" in retry))
  assert.ok(retry.credential.revokedAt !== null)
  assert.equal((await clients[63].server.health()).ok, true)
  assert.equal((await owner.credentials.list()).credentials.length, 1000)
  assert.ok(!readFileSync(registryPath, "utf8").includes(secret))
  report.peakSampledServerRssKiB = process.platform === "win32" ? null : peakRssKiB
  report.capacityRejectionAndRetry =
    "passed; revoked records retained, old issue receipt resolves at capacity, unrelated session usable"
  console.log(JSON.stringify(report, null, 2))
} finally {
  clearInterval(sampler)
  for (const client of clients) client.close()
  if (server && server.exitCode === null) {
    const exited = once(server, "exit")
    server.kill("SIGTERM")
    await exited
  }
  rmSync(directory, { recursive: true, force: true })
}
