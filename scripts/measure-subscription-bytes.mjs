#!/usr/bin/env node
/**
 * Bytes a followed conversation sends during one streamed turn, over a local
 * proxy shaped like a slow phone link: 150 ms round trip, 1.6 Mbps down
 * (1_600_000 bits/s = 200_000 bytes/s). The proxy stands in for the tunnel.
 *
 * A short conversation has no earlier turns. A long one has twelve finished
 * turns, then the measured stream. The provider is the conversation-smoke
 * fixture, not a live model. Counts are `conversation.view` frames the
 * follower received: how many, their total websocket-message bytes, and the
 * largest. Payload bytes are the event body the 64 KiB frame bound applies to
 * (`bounds` in the generated client; `product.max_payload_bytes` in
 * docs/limits.md).
 *
 * Stdout is one JSON document. Progress goes to stderr.
 *
 *   pnpm exec tsx scripts/measure-subscription-bytes.mjs
 */
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { randomUUID } from "node:crypto"
import { once } from "node:events"
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  symlinkSync,
  writeFileSync,
} from "node:fs"
import { createServer } from "node:net"
import { cpus, arch, platform, release } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { WebSocket, WebSocketServer } from "ws"
import { NessaClient, NessaClientConfig } from "@nessa/client"
import {
  createFixtureSupervisor,
  startGateway,
  stopGateway,
} from "./conversation-smoke/runtime.mjs"
import {
  linkDeliveryMs,
  summarizeViews,
  throttleProfile,
} from "./subscription-byte-summary.mjs"

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const historyTurns = 12

function agentTextBytes(view, executionId) {
  const message = view?.messages?.find((item) => item.executionId === executionId)
  if (!message) return 0
  const text = (message.parts ?? [])
    .filter((part) => part.kind === "text")
    .map((part) => part.text ?? "")
    .join("")
  return Buffer.byteLength(text)
}

const invokedDirectly =
  process.argv[1] && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)

if (invokedDirectly) {
  try {
    const report = await measure()
    process.stdout.write(`${JSON.stringify(report, null, 2)}\n`)
  } catch (error) {
    process.stderr.write(`${error.stack ?? error}\n`)
    process.exitCode = 1
  }
}

async function measure() {
  const binary = process.env.NESSA_SMOKE_BINARY ?? join(root, "target/debug/nessa")
  assert.ok(existsSync(binary), `gateway binary does not exist: ${binary}`)
  const temporary = realpathSync(mkdtempSync(join("/tmp", "nessa-subscription-bytes-")))
  const dataRoot = join(temporary, "data")
  const workspace = join(temporary, "workspace")
  const selectedWorkspace = join(temporary, "selected-workspace")
  const ownerPath = join(temporary, "owner.token")
  const port = await reservePort()
  const gatewayUrl = `ws://127.0.0.1:${port}`
  const instance = "subscription-bytes"
  const env = {
    ...process.env,
    NESSA_DATA_DIR: dataRoot,
    NESSA_INSTANCE: instance,
    NESSA_STAGE: "ci",
    NESSA_PORT: String(port),
    NESSA_HOST: "127.0.0.1",
  }
  mkdirSync(workspace)
  symlinkSync(workspace, selectedWorkspace, "dir")
  const fixtureSupervisor = await createFixtureSupervisor()
  const clients = []
  let proxy
  let gateway
  // The supervisor is already listening. Auth init and the config write can
  // fail before a gateway exists; cleanup still has to close that server or
  // the process stays up after the error is reported.
  try {
    const initialized = spawnSync(
      binary,
      ["auth", "init", "--local", "--owner-token-file", ownerPath],
      { cwd: root, env, encoding: "utf8" },
    )
    if (initialized.status !== 0) {
      throw new Error(initialized.stderr || "auth init failed")
    }
    const configPath = join(dataRoot, "ci", "instances", instance, "config.json")
    writeFileSync(
      configPath,
      JSON.stringify({
        agents: {
          catalog: join(root, "crates/nessa-sdk/data/models.json"),
          workspace: selectedWorkspace,
          mcpServers: [],
          selected: "claude",
          runtimes: {
            claude: {
              command: process.execPath,
              args: [
                join(root, "scripts/conversation-smoke/claude-acp-fixture.mjs"),
                "provider-evidence.jsonl",
                "provider-state.json",
                String(fixtureSupervisor.port),
                fixtureSupervisor.nonce,
              ],
              model: "claude-haiku-4-5-20251001",
              toolsEnabled: true,
              contextTokens: 100000,
              outputTokens: 4096,
            },
          },
        },
        session: {
          handshakeTimeoutMs: 5000,
          writeTimeoutMs: 15000,
          currentStateIntervalMs: 50,
        },
      }),
      { mode: 0o600 },
    )
    chmodSync(configPath, 0o600)
    gateway = startGateway(binary, root, env, port)
    await gateway.ready({ timeoutMs: 30_000 })
    const credential = readFileSync(ownerPath, "utf8").trim()
    const connect = (url) =>
      NessaClient.connect({
        stage: "ci",
        url,
        role: "surface",
        surface: { kind: "panel", instance },
        client: { id: "subscription-bytes", version: "0.1.0", platform: "node" },
        profile: "product",
        auth: { credential },
        config: new NessaClientConfig({
          reconnect: { enabled: false },
          requestTimeoutMs: 20_000,
        }),
      })
    const sender = await connect(gatewayUrl)
    clients.push(sender)
    const runs = []
    for (const name of ["short", "long"]) {
      process.stderr.write(`measuring ${name}\n`)
      runs.push(await oneConversation(sender, connect, gatewayUrl, name))
    }
    return {
      profile: throttleProfile,
      link: "local WebSocket proxy; 75 ms each way and 200000 bytes/s toward the follower",
      provider: "scripts/conversation-smoke/claude-acp-fixture.mjs",
      machine: {
        platform: platform(),
        release: release(),
        arch: arch(),
        cpuModel: cpus()[0]?.model ?? "unknown",
        cpus: cpus().length,
      },
      runs,
    }
  } finally {
    if (proxy) await proxy.close().catch(() => {})
    for (const client of clients) client.close()
    if (gateway) {
      await stopGateway(gateway).catch((error) => {
        process.stderr.write(`gateway stop: ${error.message}\n`)
      })
    }
    await fixtureSupervisor.shutdown().catch((error) => {
      process.stderr.write(`fixture stop: ${error.message}\n`)
    })
  }

  async function oneConversation(sender, connect, gatewayUrl, name) {
    const conversationId = randomUUID()
    await sender.conversation.create({
      conversationId,
      requestId: `create-${name}`,
      agent: "claude",
    })
    process.stderr.write(`  ${name}: created\n`)
    if (name === "long") {
      for (let index = 0; index < historyTurns; index++) {
        const executionId = `measure-history-${name}-${index}`
        await sendTurn(sender, conversationId, executionId, `history ${index}`)
        await waitForRead(sender, conversationId, executionId, "history turn")
        process.stderr.write(`  ${name}: history ${index + 1}/${historyTurns}\n`)
      }
    }
    proxy = await throttleProxy(gatewayUrl)
    const follower = await connect(proxy.url)
    clients.push(follower)
    const followed = follow(follower, conversationId)
    process.stderr.write(`  ${name}: follower connected\n`)
    await followed.ready
    process.stderr.write(`  ${name}: follower view\n`)
    const seen = proxy.views()
    const initial = seen.at(-1) ?? null
    proxy.beginTurn()
    const executionId = `measure-stream-${name}`
    process.stderr.write(`  ${name}: sending\n`)
    await sendTurn(sender, conversationId, executionId, `stream ${name}`)
    process.stderr.write(`  ${name}: sent\n`)
    let message
    try {
      message = await waitForStatus(followed, executionId, `${name} stream`)
    } catch (error) {
      if (!followed.ended()) throw error
      message = { status: followed.ended().reason }
    }
    const frames = proxy.endTurn()
    const ended = followed.ended()
    process.stderr.write(`  ${name}: ${frames.length} view frames\n`)
    follower.close()
    await proxy.close()
    proxy = null
    return {
      name,
      historyTurns: name === "long" ? historyTurns : 0,
      status: message.status,
      subscriptionEnded: ended?.reason ?? null,
      initialView: initial
        ? { bytes: initial.bytes, payloadBytes: initial.payloadBytes }
        : null,
      turn: summarizeViews(frames, agentTextBytes(followed.current(), executionId)),
      frameBytes: frames.map((frame) => frame.bytes),
    }
  }
}

function follow(client, conversationId) {
  let view = null
  let ended = null
  let rejectReady
  const ready = new Promise((resolve, reject) => {
    rejectReady = reject
    client.subscriptions
      .view(conversationId, {
        view(frame) {
          view = frame.view
          resolve()
        },
        ended(end) {
          ended = end
          rejectReady(new Error(`subscription ended before a view: ${end.reason}`))
        },
      })
      .catch(reject)
  })
  return {
    ready,
    current: () => view,
    ended: () => ended,
  }
}

function sendTurn(client, conversationId, executionId, text) {
  return client.conversation.submit(
    client.conversation.prepare(
      conversationId,
      `${text}\nfixtureCorrelation:${executionId}`,
      [],
      [],
      { requestId: `request-${executionId}`, executionId },
    ),
  )
}

async function waitForRead(client, conversationId, executionId, description) {
  const deadline = Date.now() + 60_000
  let message
  while (Date.now() < deadline) {
    const view = await client.conversation.read(conversationId)
    message = view.messages?.find((item) => item.executionId === executionId)
    if (message && ["completed", "failed", "cancelled"].includes(message.status))
      return message
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  throw new Error(
    `timed out waiting for ${description}: ${JSON.stringify(message ?? null)}`,
  )
}

async function waitForStatus(followed, executionId, description) {
  const deadline = Date.now() + 60_000
  let message
  while (Date.now() < deadline) {
    if (followed.ended()) {
      throw new Error(`${description} subscription ended: ${followed.ended().reason}`)
    }
    message = followed
      .current()
      ?.messages?.find((item) => item.executionId === executionId)
    if (message && ["completed", "failed", "cancelled"].includes(message.status))
      return message
    await new Promise((resolve) => setTimeout(resolve, 50))
  }
  throw new Error(
    `timed out waiting for ${description}: ${JSON.stringify(message ?? null)}`,
  )
}

function reservePort() {
  return new Promise((resolve, reject) => {
    const server = createServer()
    server.once("error", reject)
    server.listen(0, "127.0.0.1", () => {
      const address = server.address()
      server.close(() => resolve(address.port))
    })
  })
}

async function throttleProxy(upstreamUrl) {
  const server = new WebSocketServer({ host: "127.0.0.1", port: 0 })
  await once(server, "listening")
  const address = server.address()
  assert.ok(address && typeof address === "object")
  const views = []
  let captureFrom = null
  // Downlink serialization only. Propagation is not on this clock: each
  // message waits its own one-way delay after its bytes have been serialized,
  // overlapping the next message. Uplink does not use this clock.
  let downFreeAt = 0
  const propagationMs = throttleProfile.rttMs / 2
  server.on("connection", (downstream) => {
    const upstream = new WebSocket(new URL("/session", upstreamUrl).href)
    const queued = []
    const after = (at, send) => {
      const wait = Math.max(0, at - Date.now())
      setTimeout(send, wait)
    }
    downstream.on("message", (bytes, binary) => {
      const size = Buffer.isBuffer(bytes) ? bytes.length : Buffer.byteLength(bytes)
      const planned = linkDeliveryMs([{ direction: "up", bytes: size }], {
        propagationMs,
        downBytesPerSecond: throttleProfile.downBytesPerSecond,
        nowMs: Date.now(),
        downFreeAt,
      })
      after(planned.at[0], () => {
        if (upstream.readyState === WebSocket.OPEN) upstream.send(bytes, { binary })
        else queued.push([bytes, binary])
      })
    })
    upstream.on("open", () => {
      for (const [bytes, binary] of queued.splice(0)) upstream.send(bytes, { binary })
    })
    upstream.on("error", () => {})
    downstream.on("error", () => {})
    upstream.on("message", (bytes, binary) => {
      const buffer = Buffer.isBuffer(bytes) ? bytes : Buffer.from(bytes)
      const now = Date.now()
      const planned = linkDeliveryMs([{ direction: "down", bytes: buffer.length }], {
        propagationMs,
        downBytesPerSecond: throttleProfile.downBytesPerSecond,
        nowMs: now,
        downFreeAt,
      })
      downFreeAt = planned.downFreeAt
      after(planned.at[0], () => {
        record(buffer)
        if (downstream.readyState === WebSocket.OPEN) downstream.send(buffer, { binary })
      })
    })
    downstream.on("close", () => upstream.close())
    upstream.on("close", () => downstream.close())
  })
  function record(buffer) {
    let parsed
    try {
      parsed = JSON.parse(buffer.toString())
    } catch {
      return
    }
    if (parsed.type !== "event" || parsed.event !== "conversation.view") return
    views.push({
      bytes: buffer.length,
      payloadBytes: Buffer.byteLength(JSON.stringify(parsed.payload ?? null)),
    })
  }
  return {
    url: `ws://127.0.0.1:${address.port}`,
    views: () => views.slice(),
    beginTurn() {
      captureFrom = views.length
    },
    endTurn() {
      return views.slice(captureFrom ?? views.length)
    },
    close: () =>
      new Promise((resolve) => {
        for (const socket of server.clients) socket.terminate()
        server.close(() => resolve())
      }),
  }
}
