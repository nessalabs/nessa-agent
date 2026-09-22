import assert from "node:assert/strict"
import { once } from "node:events"
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { createServer } from "node:http"
import { createConnection } from "node:net"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import { WebSocket, WebSocketServer } from "ws"
import { waitFor } from "./evidence.mjs"
import {
  collectCleanupFailures,
  createFixtureSupervisor,
  createLossyProxy,
  reservePort,
  startGateway,
  stopGateway,
} from "./runtime.mjs"

test("lossy proxy turns a thrown frame decoder into an awaited failure", async () => {
  const upstream = new WebSocketServer({ host: "127.0.0.1", port: 0 })
  await once(upstream, "listening")
  const address = upstream.address()
  assert.ok(address && typeof address === "object")
  upstream.on("connection", (socket) => socket.send("{malformed"))
  const proxy = await createLossyProxy(
    `ws://127.0.0.1:${address.port}`,
    "request-that-will-not-arrive",
  )
  const downstream = new WebSocket(proxy.url)
  try {
    const failure = await proxy.failure
    assert.match(failure.message, /JSON/)
    assert.throws(() => proxy.assertHealthy(), /JSON/)
  } finally {
    downstream.terminate()
    await proxy.close()
    await new Promise((resolve) => upstream.close(resolve))
  }
})

test("gateway readiness bounds a hanging health request", async () => {
  const directory = mkdtempSync(join(tmpdir(), "nessa-hung-gateway-"))
  const binary = join(directory, "gateway-fixture")
  writeFileSync(binary, "#!/usr/bin/env node\nsetInterval(() => {}, 1000)\n")
  chmodSync(binary, 0o700)
  const server = createServer(() => {})
  server.listen(0, "127.0.0.1")
  await once(server, "listening")
  const address = server.address()
  assert.ok(address && typeof address === "object")
  const gateway = startGateway(binary, directory, process.env, address.port)
  try {
    await assert.rejects(
      gateway.ready({ timeoutMs: 180, requestTimeoutMs: 40 }),
      /gateway health/,
    )
  } finally {
    await stopGateway(gateway, { gracefulTimeoutMs: 500, killTimeoutMs: 500 })
    await new Promise((resolve) => server.close(resolve))
    rmSync(directory, { recursive: true, force: true })
  }
})

test("gateway readiness reports a child spawn failure", async () => {
  const directory = mkdtempSync(join(tmpdir(), "nessa-missing-gateway-"))
  const port = await reservePort()
  const gateway = startGateway(
    join(directory, "does-not-exist"),
    directory,
    process.env,
    port,
  )
  try {
    await assert.rejects(gateway.ready({ timeoutMs: 500 }), /could not start/)
  } finally {
    await stopGateway(gateway, { gracefulTimeoutMs: 50, killTimeoutMs: 50 })
    rmSync(directory, { recursive: true, force: true })
  }
})

test("cleanup keeps sequencing after independent failures", async () => {
  const order = []
  const failures = await collectCleanupFailures([
    {
      name: "socket",
      run: () => {
        order.push("socket")
        throw new Error("socket refused")
      },
    },
    {
      name: "gateway",
      run: async () => {
        order.push("gateway")
      },
    },
    {
      name: "fixture",
      run: () => {
        order.push("fixture")
        throw new Error("fixture survived")
      },
    },
  ])
  assert.deepEqual(order, ["socket", "gateway", "fixture"])
  assert.deepEqual(
    failures.map((failure) => failure.cleanupName),
    ["socket", "fixture"],
  )
  assert.equal(failures[0].cause.message, "socket refused")
  assert.equal(failures[1].cause.message, "fixture survived")
})

test("fixture supervisor shuts down only the nonce-authenticated live process", async () => {
  const supervisor = await createFixtureSupervisor()
  const owned = createConnection(supervisor.port, "127.0.0.1")
  const unrelated = createConnection(supervisor.port, "127.0.0.1")
  let ownedCommand = ""
  let unrelatedCommand = ""
  try {
    await Promise.all([once(owned, "connect"), once(unrelated, "connect")])
    owned.on("data", (bytes) => {
      ownedCommand += bytes.toString()
      if (ownedCommand === "shutdown\n") owned.end()
    })
    unrelated.on("data", (bytes) => {
      unrelatedCommand += bytes.toString()
    })
    const unrelatedClose = once(unrelated, "close")
    owned.write(`${JSON.stringify({ nonce: supervisor.nonce, processId: 4321 })}\n`)
    unrelated.write(`${JSON.stringify({ nonce: "wrong-owner", processId: 4321 })}\n`)
    await unrelatedClose
    await waitFor(
      () => supervisor.activeProcessIds(),
      (processIds) => processIds.length === 1,
      "authenticated fixture registration",
    )
    await supervisor.shutdown()
    assert.equal(ownedCommand, "shutdown\n")
    assert.equal(unrelatedCommand, "")
  } finally {
    owned.destroy()
    unrelated.destroy()
  }
})
