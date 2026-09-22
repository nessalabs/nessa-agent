import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { once } from "node:events"
import { createServer } from "node:net"
import { WebSocket, WebSocketServer } from "ws"
import { waitFor } from "./evidence.mjs"

export async function reservePort() {
  const listener = createServer().listen(0, "127.0.0.1")
  await once(listener, "listening")
  const address = listener.address()
  assert.ok(address && typeof address === "object")
  const port = address.port
  await new Promise((resolve) => listener.close(resolve))
  return port
}

export function startGateway(binary, root, env, port) {
  const child = spawn(binary, ["server"], {
    cwd: root,
    env,
    stdio: ["ignore", "pipe", "pipe"],
  })
  let logs = ""
  for (const stream of [child.stdout, child.stderr])
    stream.on("data", (bytes) => {
      logs = `${logs}${bytes.toString()}`.slice(-16_000)
    })
  return {
    child,
    logs: () => logs,
    ready: () =>
      waitFor(
        async () => {
          if (child.exitCode !== null) throw new Error("gateway exited during startup")
          try {
            return (await fetch(`http://127.0.0.1:${port}/health`)).ok
          } catch {
            return false
          }
        },
        Boolean,
        "gateway health",
        20_000,
      ),
  }
}

export async function stopGateway(gateway) {
  const { child } = gateway
  if (child.exitCode !== null) return
  await new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error("gateway did not stop after SIGTERM")),
      10_000,
    )
    child.once("exit", () => {
      clearTimeout(timer)
      resolve()
    })
    child.kill("SIGTERM")
  })
}

export async function createLossyProxy(upstreamUrl, droppedRequestId) {
  const server = new WebSocketServer({ host: "127.0.0.1", port: 0 })
  await once(server, "listening")
  const address = server.address()
  assert.ok(address && typeof address === "object")
  const evidence = { matchingSendFrames: 0, droppedResponses: 0 }
  const sockets = new Set()
  server.on("connection", (downstream) => {
    const upstream = new WebSocket(`${upstreamUrl}/session`)
    sockets.add(downstream)
    sockets.add(upstream)
    let sendWireId
    const queued = []
    downstream.on("message", (bytes, binary) => {
      const frame = JSON.parse(bytes.toString())
      if (
        frame.method === "conversation.send" &&
        frame.params?.requestId === droppedRequestId
      ) {
        evidence.matchingSendFrames += 1
        sendWireId = frame.id
      }
      if (upstream.readyState === WebSocket.OPEN) upstream.send(bytes, { binary })
      else queued.push([bytes, binary])
    })
    upstream.on("open", () => {
      for (const [bytes, binary] of queued.splice(0)) upstream.send(bytes, { binary })
    })
    upstream.on("message", (bytes, binary) => {
      const frame = JSON.parse(bytes.toString())
      if (sendWireId && frame.id === sendWireId && evidence.droppedResponses === 0) {
        assert.equal(frame.ok, true)
        assert.equal(frame.payload.executionId, "execution-answer")
        evidence.droppedResponses += 1
        downstream.terminate()
        upstream.terminate()
        return
      }
      if (downstream.readyState === WebSocket.OPEN) downstream.send(bytes, { binary })
    })
    const forget = (socket) => sockets.delete(socket)
    downstream.on("close", () => {
      forget(downstream)
      if (upstream.readyState < WebSocket.CLOSING) upstream.close()
    })
    upstream.on("close", () => {
      forget(upstream)
      if (downstream.readyState < WebSocket.CLOSING) downstream.close()
    })
    upstream.on("error", () => downstream.terminate())
    downstream.on("error", () => upstream.terminate())
  })
  return {
    url: `ws://127.0.0.1:${address.port}`,
    evidence,
    close: async () => {
      for (const socket of sockets) socket.terminate()
      await new Promise((resolve) => server.close(resolve))
    },
  }
}

export function processIsGone(processId) {
  try {
    process.kill(processId, 0)
    return false
  } catch (error) {
    if (error?.code === "ESRCH") return true
    throw error
  }
}
