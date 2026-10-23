import test from "node:test"
import assert from "node:assert/strict"
import { createSession } from "../runtime/session.mjs"
import { navigationObserver } from "./support/navigation-page.mjs"

function deferred() {
  let resolve
  const promise = new Promise((accept) => {
    resolve = accept
  })
  return { promise, resolve }
}
const failedUrl = "http://127.0.0.1:1/"
const home = "http://127.0.0.1:1234/"
const frame = (loaderId, url = "chrome-error://chromewebdata/") => ({
  frame: { id: "main", loaderId, url, unreachableUrl: failedUrl },
})
const dom = (loaderId) => ({ frameId: "main", loaderId, name: "DOMContentLoaded" })

for (const domBeforeCommit of [false, true]) {
  test(
    `queued recovery waits for correlated new document and DOM event; DOM first=${domBeforeCommit}`,
    { timeout: 2000 },
    async () => {
      const dispatched = deferred(),
        calls = [],
        observers = []
      const primary = new Error("navigation refused before error document commit")
      let committed = false,
        loaded = false
      const session = createSession(
        {
          context: () => ({
            newCDPSession: async () => {
              const observer = navigationObserver()
              observers.push(observer)
              return observer
            },
          }),
          goto: async (url) => {
            calls.push(url)
            assert.ok(observers.at(-1).listenerCount("Page.frameNavigated") > 0)
            if (url === failedUrl) {
              dispatched.resolve()
              throw primary
            }
            assert.equal(committed, true)
            assert.equal(loaded, true, "late error document must not interrupt recovery")
          },
        },
        home,
      )
      const failed = assert.rejects(
        session.run({ type: "navigate", target: "unavailable" }),
        (error) => error.code === "browser_operation_failed" && error.cause === primary,
      )
      const recovery = session.run({ type: "navigate", target: "home" })
      await dispatched.promise
      const observer = observers[0]
      // Hash navigation and readiness from the old or unrelated document do not settle failure.
      observer.emit("Page.navigatedWithinDocument", {
        frameId: "main",
        url: home + "#human",
      })
      observer.emit("Page.frameNavigated", frame("old"))
      observer.emit("Page.frameNavigated", {
        frame: { id: "child", loaderId: "error", url: failedUrl },
      })
      observer.emit("Page.lifecycleEvent", dom("old"))
      observer.emit("Page.lifecycleEvent", dom("unrelated"))
      await new Promise(setImmediate)
      assert.deepEqual(calls, [failedUrl])
      if (domBeforeCommit) {
        loaded = true
        observer.emit("Page.lifecycleEvent", dom("error"))
        await new Promise(setImmediate)
        assert.deepEqual(calls, [failedUrl])
      }
      committed = true
      observer.emit("Page.frameNavigated", frame("error"))
      if (!domBeforeCommit) {
        await new Promise(setImmediate)
        assert.deepEqual(calls, [failedUrl])
        loaded = true
        observer.emit("Page.lifecycleEvent", dom("error"))
      }
      await failed
      await recovery
      assert.deepEqual(calls, [failedUrl, home])
      assert.ok(
        observers.every((item) => item.detached && item.eventNames().length === 0),
      )
      await session.close()
    },
  )
}

test("unconfirmed error commit or DOM readiness preserves failure and fences queued work", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] })
  for (const phase of ["commit", "dom"]) {
    const observer = navigationObserver(),
      dispatched = deferred()
    const primary = new Error("navigation failed")
    let calls = 0
    const session = createSession(
      {
        context: () => ({ newCDPSession: async () => observer }),
        goto: async () => {
          calls++
          dispatched.resolve()
          throw primary
        },
      },
      home,
    )
    const failed = assert.rejects(
      session.run({ type: "navigate", target: "unavailable" }),
      (error) =>
        error.code === "navigation_unsettled" &&
        error.cause === primary &&
        error.settlementError.message === "navigation_settlement_timeout",
    )
    const queued = assert.rejects(session.run({ type: "navigate", target: "home" }), {
      code: "session_closed",
    })
    await dispatched.promise
    if (phase === "dom") observer.emit("Page.frameNavigated", frame("error"))
    t.mock.timers.tick(5000)
    await failed
    await queued
    await assert.rejects(session.run({ type: "increment" }), { code: "session_closed" })
    assert.equal(calls, 1)
    assert.equal(observer.detached, true)
    await session.close()
  }
})

test("observer cleanup failure preserves navigation cause and fences further controls", async () => {
  const observer = navigationObserver()
  const primary = new Error("navigation refused"),
    cleanup = new Error("detach failed")
  observer.detach = async () => {
    throw cleanup
  }
  const session = createSession(
    {
      context: () => ({ newCDPSession: async () => observer }),
      goto: async () => {
        observer.emit("Page.frameNavigated", frame("error"))
        observer.emit("Page.lifecycleEvent", dom("error"))
        throw primary
      },
    },
    home,
  )
  await assert.rejects(
    session.run({ type: "navigate", target: "unavailable" }),
    (error) =>
      error.code === "navigation_unsettled" &&
      error.cause === primary &&
      error.settlementError === cleanup,
  )
  await assert.rejects(session.run({ type: "increment" }), { code: "session_closed" })
  assert.equal(observer.eventNames().length, 0)
  await session.close()
})
