import assert from "node:assert/strict"
import test from "node:test"

import { webdriverBudgets, webdriverRequest } from "./native-smoke-webdriver.mjs"

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
