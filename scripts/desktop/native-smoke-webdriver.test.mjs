import assert from "node:assert/strict"
import test from "node:test"

import {
  observeWebdriverStartup,
  webdriverBudgets,
  webdriverRequest,
} from "./native-smoke-webdriver.mjs"

const response = (value) => ({
  ok: true,
  status: 200,
  text: async () => JSON.stringify({ value }),
})

test("session startup and ordinary commands own different budgets", async () => {
  const budgets = []
  const timeoutSignal = (milliseconds) => {
    budgets.push(milliseconds)
    return new AbortController().signal
  }
  const fetchRequest = async (_url, { signal }) => {
    signal.throwIfAborted()
    return response({ sessionId: "session" })
  }

  await webdriverRequest({
    port: 4444,
    method: "POST",
    path: "/session",
    body: {},
    phase: "create WebDriver session",
    lifecycle: "session",
    fetchRequest,
    timeoutSignal,
  })
  await webdriverRequest({
    port: 4444,
    method: "GET",
    path: "/session/session/title",
    phase: "read title",
    fetchRequest,
    timeoutSignal,
  })

  assert.deepEqual(budgets, [webdriverBudgets.session, webdriverBudgets.command])
})

test("a timed-out command retains its phase, elapsed time, budget, and cause", async () => {
  const timeout = new DOMException("request timed out", "TimeoutError")
  let clock = 100
  const fetchRequest = async (_url, { signal }) => {
    clock = 127
    signal.throwIfAborted()
    return response({})
  }

  await assert.rejects(
    webdriverRequest({
      port: 4444,
      method: "POST",
      path: "/session/session/execute/sync",
      body: { script: "return true" },
      phase: "inspect connected panel",
      fetchRequest,
      timeoutSignal: () => AbortSignal.abort(timeout),
      now: () => clock,
    }),
    (error) => {
      assert.match(error.message, /inspect connected panel/)
      assert.match(error.message, /POST \/session\/session\/execute\/sync/)
      assert.match(error.message, /after 27ms/)
      assert.match(error.message, /command budget 10000ms/)
      assert.equal(error.cause, timeout)
      return true
    },
  )
})

function pendingUntilAborted(events, name) {
  return (signal) =>
    new Promise((_resolve, reject) => {
      signal.addEventListener(
        "abort",
        () => {
          events.push(`${name} aborted`)
          reject(signal.reason)
        },
        { once: true },
      )
    }).finally(() => events.push(`${name} settled`))
}

test("a session failure aborts and settles application observation before returning", async () => {
  const events = []
  const failure = new Error("session handshake failed")

  await assert.rejects(
    observeWebdriverStartup({
      createSession: async () => {
        events.push("session failed")
        throw failure
      },
      observeApplication: pendingUntilAborted(events, "application"),
    }),
    failure,
  )
  assert.deepEqual(events, [
    "session failed",
    "application aborted",
    "application settled",
  ])
})

test("an application failure aborts and settles session creation before returning", async () => {
  const events = []
  const failure = new Error("application PID never appeared")

  await assert.rejects(
    observeWebdriverStartup({
      createSession: pendingUntilAborted(events, "session"),
      observeApplication: async () => {
        events.push("application failed")
        throw failure
      },
    }),
    failure,
  )
  assert.deepEqual(events, ["application failed", "session aborted", "session settled"])
})

test("successful startup returns both independently observed facts", async () => {
  assert.deepEqual(
    await observeWebdriverStartup({
      createSession: async () => ({ sessionId: "session" }),
      observeApplication: async () => 4321,
    }),
    { session: { sessionId: "session" }, application: 4321 },
  )
})
