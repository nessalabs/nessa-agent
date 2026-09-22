import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { randomBytes } from "node:crypto"
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
  let spawnFailure
  child.once("error", (error) => {
    spawnFailure = error
  })
  for (const stream of [child.stdout, child.stderr])
    stream.on("data", (bytes) => {
      logs = `${logs}${bytes.toString()}`.slice(-16_000)
    })
  return {
    child,
    logs: () => logs,
    ready: ({ timeoutMs = 20_000, requestTimeoutMs = 500 } = {}) =>
      waitFor(
        async () => {
          if (spawnFailure)
            throw new Error("gateway process could not start", { cause: spawnFailure })
          if (child.exitCode !== null) throw new Error("gateway exited during startup")
          try {
            return (
              await fetch(`http://127.0.0.1:${port}/health`, {
                signal: AbortSignal.timeout(requestTimeoutMs),
              })
            ).ok
          } catch {
            if (spawnFailure)
              throw new Error("gateway process could not start", { cause: spawnFailure })
            if (child.exitCode !== null) throw new Error("gateway exited during startup")
            return false
          }
        },
        Boolean,
        "gateway health",
        timeoutMs,
      ),
  }
}

function waitForExit(child, timeoutMs) {
  if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve(true)
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      child.off("exit", exited)
      resolve(false)
    }, timeoutMs)
    const exited = () => {
      clearTimeout(timer)
      resolve(true)
    }
    child.once("exit", exited)
  })
}

export async function stopGateway(
  gateway,
  { gracefulTimeoutMs = 10_000, killTimeoutMs = 5_000 } = {},
) {
  const { child } = gateway
  if (child.exitCode !== null || child.signalCode !== null) return
  child.kill("SIGTERM")
  if (await waitForExit(child, gracefulTimeoutMs)) return
  child.kill("SIGKILL")
  if (!(await waitForExit(child, killTimeoutMs)))
    throw new Error("gateway survived SIGKILL")
  throw new Error("gateway required SIGKILL after its shutdown deadline")
}

export async function createLossyProxy(upstreamUrl, droppedRequestId) {
  const server = new WebSocketServer({ host: "127.0.0.1", port: 0 })
  await once(server, "listening")
  const address = server.address()
  assert.ok(address && typeof address === "object")
  const evidence = { matchingSendFrames: 0, droppedResponses: 0 }
  const sockets = new Set()
  let failureError
  let resolveFailure
  let closing = false
  const failure = new Promise((resolve) => {
    resolveFailure = resolve
  })
  const fail = (cause) => {
    if (failureError || closing) return
    failureError =
      cause instanceof Error ? cause : new Error("lossy proxy failed", { cause })
    resolveFailure(failureError)
    for (const socket of sockets) socket.terminate()
  }
  const guarded =
    (handler) =>
    (...args) => {
      try {
        handler(...args)
      } catch (error) {
        fail(error)
      }
    }

  server.on("connection", (downstream) => {
    const upstream = new WebSocket(`${upstreamUrl}/session`)
    sockets.add(downstream)
    sockets.add(upstream)
    let sendWireId
    let expectedDisconnect = false
    const queued = []
    downstream.on(
      "message",
      guarded((bytes, binary) => {
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
      }),
    )
    upstream.on(
      "open",
      guarded(() => {
        for (const [bytes, binary] of queued.splice(0)) upstream.send(bytes, { binary })
      }),
    )
    upstream.on(
      "message",
      guarded((bytes, binary) => {
        const frame = JSON.parse(bytes.toString())
        if (sendWireId && frame.id === sendWireId && evidence.droppedResponses === 0) {
          assert.equal(frame.ok, true)
          assert.equal(frame.payload.executionId, "execution-answer")
          evidence.droppedResponses += 1
          expectedDisconnect = true
          downstream.terminate()
          upstream.terminate()
          return
        }
        if (downstream.readyState === WebSocket.OPEN) downstream.send(bytes, { binary })
      }),
    )
    const forget = (socket) => sockets.delete(socket)
    downstream.on(
      "close",
      guarded(() => {
        forget(downstream)
        if (upstream.readyState < WebSocket.CLOSING) upstream.close()
      }),
    )
    upstream.on(
      "close",
      guarded(() => {
        forget(upstream)
        if (downstream.readyState < WebSocket.CLOSING) downstream.close()
      }),
    )
    upstream.on("error", (error) => {
      if (!expectedDisconnect && !closing) fail(error)
    })
    downstream.on("error", (error) => {
      if (!expectedDisconnect && !closing) fail(error)
    })
  })

  const assertHealthy = () => {
    if (failureError) throw failureError
  }
  return {
    url: `ws://127.0.0.1:${address.port}`,
    evidence,
    failure,
    assertHealthy,
    guard: (operation) =>
      Promise.race([
        operation,
        failure.then((error) => {
          throw error
        }),
      ]),
    close: async ({ timeoutMs = 2_000 } = {}) => {
      closing = true
      for (const socket of sockets) socket.terminate()
      await new Promise((resolve, reject) => {
        const timer = setTimeout(
          () => reject(new Error("lossy proxy did not close")),
          timeoutMs,
        )
        server.close(() => {
          clearTimeout(timer)
          resolve()
        })
      })
    },
  }
}

export async function createFixtureSupervisor() {
  const nonce = randomBytes(32).toString("hex")
  const registered = new Map()
  const sockets = new Set()
  let closing = false
  const server = createServer((socket) => {
    sockets.add(socket)
    let registration = ""
    socket.on("data", (bytes) => {
      if (registered.has(socket)) return
      registration += bytes.toString()
      if (registration.length > 1024) {
        socket.destroy()
        return
      }
      const newline = registration.indexOf("\n")
      if (newline < 0) return
      try {
        const message = JSON.parse(registration.slice(0, newline))
        assert.equal(message.nonce, nonce)
        assert.ok(Number.isSafeInteger(message.processId) && message.processId > 0)
        assert.equal(registration.slice(newline + 1), "")
        registered.set(socket, message.processId)
      } catch {
        socket.destroy()
      }
    })
    socket.on("close", () => {
      sockets.delete(socket)
      registered.delete(socket)
    })
    socket.on("error", () => {
      if (!closing) socket.destroy()
    })
  })
  server.listen(0, "127.0.0.1")
  await once(server, "listening")
  const address = server.address()
  assert.ok(address && typeof address === "object")

  return {
    port: address.port,
    nonce,
    activeProcessIds: () => [...registered.values()],
    shutdown: async ({ processTimeoutMs = 2_000, serverTimeoutMs = 2_000 } = {}) => {
      closing = true
      for (const socket of registered.keys()) socket.write("shutdown\n")
      let processFailure
      try {
        await waitFor(
          () => registered.size,
          (size) => size === 0,
          "fixture control connections to close",
          processTimeoutMs,
        )
      } catch (error) {
        processFailure = error
        for (const socket of sockets) socket.destroy()
      }
      for (const socket of sockets) socket.destroy()
      await new Promise((resolve, reject) => {
        const timer = setTimeout(
          () => reject(new Error("fixture supervisor did not close")),
          serverTimeoutMs,
        )
        server.close(() => {
          clearTimeout(timer)
          resolve()
        })
      })
      if (processFailure) throw processFailure
    },
  }
}

export async function collectCleanupFailures(cleanups) {
  const failures = []
  for (const { name, run } of cleanups) {
    try {
      await run()
    } catch (cause) {
      const failure = new Error(`${name} cleanup failed`, { cause })
      failure.cleanupName = name
      failures.push(failure)
    }
  }
  return failures
}
