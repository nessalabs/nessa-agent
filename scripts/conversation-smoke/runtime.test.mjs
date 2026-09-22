import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { once } from "node:events"
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { createServer } from "node:http"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import { WebSocket, WebSocketServer } from "ws"
import {
  collectCleanupFailures,
  createLossyProxy,
  processIsGone,
  reservePort,
  startGateway,
  stopGateway,
  stopOwnedFixtureProcesses,
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

test("fixture cleanup terminates only journaled processes without an end event", async () => {
  const directory = mkdtempSync(join(tmpdir(), "nessa-fixture-cleanup-"))
  const evidencePath = join(directory, "evidence.ndjson")
  const unfinished = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"])
  const ended = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"])
  await Promise.all([once(unfinished, "spawn"), once(ended, "spawn")])
  const unfinishedExit = once(unfinished, "exit")
  const endedExit = once(ended, "exit")
  writeFileSync(
    evidencePath,
    [
      { type: "process-start", processId: unfinished.pid },
      { type: "process-start", processId: ended.pid },
      { type: "process-end", processId: ended.pid },
    ]
      .map((event) => JSON.stringify(event))
      .join("\n") + "\n",
  )
  try {
    await stopOwnedFixtureProcesses(evidencePath)
    await unfinishedExit
    assert.equal(processIsGone(unfinished.pid), true)
    assert.equal(processIsGone(ended.pid), false)
  } finally {
    if (!processIsGone(unfinished.pid)) unfinished.kill("SIGKILL")
    if (!processIsGone(ended.pid)) ended.kill("SIGKILL")
    await Promise.allSettled([unfinishedExit, endedExit])
    rmSync(directory, { recursive: true, force: true })
  }
})
