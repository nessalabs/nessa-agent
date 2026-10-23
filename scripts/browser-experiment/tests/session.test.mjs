import test from "node:test"
import assert from "node:assert/strict"
import { createSession } from "../runtime/session.mjs"
import { launchOptions } from "../runtime/browser.mjs"

test("launch requires headful sandboxed Chromium and exposes no debugging TCP port", () => {
  const options = launchOptions()
  assert.equal(options.chromiumSandbox, true)
  assert.equal(options.headless, false)
  assert.equal(options.channel, "chromium")
  assert.ok(
    options.args.every(
      (arg) => !arg.startsWith("--remote-debugging") && !arg.includes("no-sandbox"),
    ),
  )
  assert.equal(options.args.length, 2)
})

test("commands serialize, queue is bounded, close fences queued and new work", async () => {
  let release
  const gate = new Promise((resolve) => {
    release = resolve
  })
  let clicks = 0
  const session = createSession(
    {
      locator: () => ({
        click: async () => {
          clicks++
          await gate
        },
      }),
    },
    "http://127.0.0.1:1234/",
  )
  const first = session.run({ type: "increment" })
  await Promise.resolve()
  const queued = Array.from({ length: 15 }, () => session.run({ type: "increment" }))
  const settled = Promise.allSettled(queued)
  await assert.rejects(session.run({ type: "increment" }), { code: "queue_full" })
  const closing = session.close()
  await assert.rejects(session.run({ type: "increment" }), { code: "session_closed" })
  release()
  await first
  await closing
  assert.equal(clicks, 1)
  assert.ok(
    (await settled).every(
      (result) => result.status === "rejected" && result.reason.code === "session_closed",
    ),
  )
  assert.deepEqual(session.status(), { state: "closed", pending: 0 })
})

test("invalid inputs cannot dispatch and prototype names are rejected", async () => {
  const session = createSession({}, "http://127.0.0.1:1234/")
  for (const [command, code] of [
    [{ type: "toString" }, "invalid_command"],
    [{ type: "__proto__" }, "invalid_command"],
    [{ type: "navigate", target: "constructor" }, "invalid_target"],
    [{ type: "navigate", target: { toString: null } }, "invalid_target"],
    [{ type: "navigate", target: "https://example.com" }, "invalid_target"],
    [{ type: "click", x: -1, y: 10 }, "invalid_coordinates"],
    [{ type: "click", x: 1000, y: 10 }, "invalid_coordinates"],
    [{ type: "click", x: NaN, y: 10 }, "invalid_coordinates"],
    [{ type: "text", text: "x".repeat(257) }, "invalid_text"],
    [{ type: "key", key: "F12" }, "invalid_key"],
    [{ type: "wheel", deltaY: 2001 }, "invalid_scroll"],
  ])
    await assert.rejects(session.run(command), { code })
  await session.close()
})

test("navigation failure is typed, next command recovers, disconnect refuses work", async () => {
  const targets = []
  const session = createSession(
    {
      goto: async (url) => {
        targets.push(url)
        if (url.endsWith(":1/")) throw new Error("transport detail")
      },
    },
    "http://127.0.0.1:1234/",
  )
  await assert.rejects(session.run({ type: "navigate", target: "unavailable" }), {
    code: "browser_operation_failed",
  })
  await session.run({ type: "navigate", target: "home" })
  assert.equal(targets.length, 2)
  session.disconnected()
  await assert.rejects(session.run({ type: "navigate", target: "home" }), {
    code: "session_closed",
  })
})
