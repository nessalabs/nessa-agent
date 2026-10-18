import assert from "node:assert/strict"
import { EventEmitter } from "node:events"
import { mkdtemp, readFile, rm } from "node:fs/promises"
import { join } from "node:path"
import { tmpdir } from "node:os"
import { test } from "node:test"
import { runInNewContext } from "node:vm"
import {
  captureOpenFailure,
  diagnosticUrl,
  openingSnapshot,
  sampleFailureRaf,
  watchOpenRequests,
} from "./open-failure-evidence.mjs"

const secret = "SYNTHETICcredentialSENTINEL"
const request = (path = "/mcp-resources") => ({
  method: () => "GET",
  resourceType: () => "fetch",
  url: () => `http://user:${secret}@127.0.0.1:1234${path}?ticket=${secret}#${secret}`,
})
test("pending HTTP and RPC remain distinct, redacted, and scoped to their socket", () => {
  const page = new EventEmitter()
  const tracker = watchOpenRequests(page)
  const req = request()
  page.emit("request", req)
  const sockets = [new EventEmitter(), new EventEmitter()]
  for (const socket of sockets) {
    socket.url = () => `ws://user:${secret}@127.0.0.1:1234/browser?auth=${secret}`
    page.emit("websocket", socket)
    socket.emit("framesent", {
      payload: JSON.stringify({
        type: "req",
        id: "8",
        method: "mcp.callTool",
        params: { credential: secret },
      }),
    })
  }
  sockets[0].emit("framereceived", {
    payload: JSON.stringify({ type: "res", id: "8", payload: { credential: secret } }),
  })
  sockets[1].emit("framesent", {
    payload: JSON.stringify({ type: "req", id: "9", method: secret, params: {} }),
  })
  page.emit("requestfinished", req)
  const snapshot = tracker.snapshot()
  assert.deepEqual(snapshot.http, [])
  assert.deepEqual(
    snapshot.rpc.map((socket) => socket.pending.map((pending) => pending.method)),
    [[], ["mcp.callTool", "[unrecognized]"]],
  )
  assert.equal(JSON.stringify(snapshot).includes(secret), false)
  assert.equal(snapshot.complete, true)
  tracker.dispose()
  assert.equal(page.listenerCount("request"), 0)
  assert.equal(sockets[0].listenerCount("framesent"), 0)
})
test("limits and unsupported frames mark an incomplete inventory without evicting it", () => {
  const page = new EventEmitter()
  const tracker = watchOpenRequests(page, 1)
  page.emit("request", request())
  page.emit("request", request("/other"))
  const socket = new EventEmitter()
  socket.url = () => "ws://127.0.0.1/browser"
  page.emit("websocket", socket)
  socket.emit("framesent", { payload: "x".repeat(65537) })
  const snapshot = tracker.snapshot()
  assert.equal(snapshot.http.length, 1)
  assert.equal(snapshot.overflow, 1)
  assert.equal(snapshot.unsupportedFrames, 1)
  assert.equal(snapshot.complete, false)
  tracker.dispose()
})

test("malformed RPC identities cannot throw out of the passive diagnostic listener", () => {
  const page = new EventEmitter()
  const tracker = watchOpenRequests(page)
  const socket = new EventEmitter()
  socket.url = () => "ws://127.0.0.1/browser"
  page.emit("websocket", socket)
  for (const id of [null, {}, { toString: secret, valueOf: secret }, 1, secret])
    assert.doesNotThrow(() =>
      socket.emit("framesent", {
        payload: JSON.stringify({ type: "req", id, method: "mcp.callTool" }),
      }),
    )
  assert.equal(tracker.snapshot().unsupportedFrames, 5)
  assert.equal(tracker.snapshot().complete, false)
  tracker.dispose()
})
test("URLs never retain arbitrary paths, userinfo, query, fragment or remote hostnames", () => {
  for (const url of [
    request(`/${secret}`).url(),
    `https://${secret}.example/${secret}`,
    `data:${secret}`,
    secret,
  ])
    assert.equal(diagnosticUrl(url).includes(secret), false)
})

test("animation snapshots cap structural entries and preserve Infinity without DOM text", () => {
  const snapshot = runInNewContext(`(${openingSnapshot.toString()})()`, {
    document: {
      visibilityState: "visible",
      readyState: "complete",
      fonts: { status: "loaded" },
      getAnimations: () =>
        Array.from({ length: 70 }, () => ({
          playState: "running",
          pending: false,
          currentTime: Infinity,
          effect: {
            target: { tagName: secret },
            getTiming: () => ({
              duration: "auto",
              iterations: Infinity,
              delay: 0,
              endDelay: 0,
            }),
          },
        })),
      querySelectorAll: () => [{ getAttribute: () => secret }],
    },
  })
  assert.equal(snapshot.animationCount, 70)
  assert.equal(snapshot.animations.length, 64)
  assert.equal(snapshot.animations[0].timing.iterations, "Infinity")
  assert.equal(snapshot.animations[0].currentTime, "Infinity")
  assert.equal(snapshot.animations[0].timing.duration, "auto")
  assert.equal(JSON.stringify(snapshot).includes(secret), false)
})
test("failure capture precedes page disposal; failures in capture never replace readiness failure", async () => {
  const root = await mkdtemp(join(tmpdir(), "nessa-opening-test-"))
  try {
    const order = []
    const page = {
      async evaluate(fn) {
        order.push(fn === openingSnapshot ? "snapshot" : "raf")
        if (fn === sampleFailureRaf) throw new Error(secret)
        return { animationCount: 1 }
      },
      async screenshot() {
        order.push("screenshot")
        throw new Error(secret)
      },
    }
    const progress = {
      lastCompleted: "windowOpened",
      wait: "settled",
      startedAt: 1,
      failedAt: 3001,
    }
    const directory = await captureOpenFailure(
      page,
      { snapshot: () => ({ http: [], rpc: [] }) },
      { evidence: root },
      progress,
    )
    order.push("close")
    assert.deepEqual(order, ["snapshot", "raf", "screenshot", "close"])
    const document = JSON.parse(await readFile(join(directory, "opening.json"), "utf8"))
    assert.deepEqual(document.progress, progress)
    assert.deepEqual(document.raf, { unavailable: true })
    assert.equal(document.screenshot, "unavailable")
    assert.equal(JSON.stringify(document).includes(secret), false)
    assert.equal(
      await captureOpenFailure(
        page,
        {
          snapshot() {
            throw new Error(secret)
          },
        },
        { evidence: root },
        progress,
      ),
      null,
    )
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})
