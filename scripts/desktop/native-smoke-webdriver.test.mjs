import assert from "node:assert/strict"
import { JSDOM } from "jsdom"
import test from "node:test"

import {
  nativePanelObservationScript,
  nativePanelReady,
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

test("panel readiness requires the rendered connection contract", () => {
  const connected = {
    readyState: "complete",
    root: {
      present: true,
      width: 400,
      height: 320,
      display: "block",
      visibility: "visible",
    },
    fallback: { present: false },
    connectionText: "Connected",
  }

  assert.equal(nativePanelReady(connected), true)
  for (const observation of [
    { ...connected, readyState: "interactive" },
    { ...connected, root: { present: false } },
    { ...connected, root: { ...connected.root, width: 0 } },
    { ...connected, root: { ...connected.root, visibility: "hidden" } },
    { ...connected, fallback: { present: true } },
    { ...connected, connectionText: "Connecting to the local server…" },
    { ...connected, connectionText: "Not connected" },
  ])
    assert.equal(nativePanelReady(observation), false, JSON.stringify(observation))
})

test("the page observation reads the connection from the empty transcript", () => {
  const dom = new JSDOM(
    `<title>Nessa</title><body><main data-nessa-root>
      <div aria-label="New conversation transcript, 0 sent">
        <p class="nessa-text-3">Connected</p>
      </div>
    </main></body>`,
    { url: "tauri://localhost/index.html?surface=main", runScripts: "outside-only" },
  )
  const root = dom.window.document.querySelector("[data-nessa-root]")
  root.getBoundingClientRect = () => ({ width: 400, height: 320 })
  dom.window.document.body.innerText = "Nessa Connected"

  const observation = new dom.window.Function(nativePanelObservationScript)()

  assert.equal(observation.url, "tauri://localhost/index.html?surface=main")
  assert.equal(observation.surface, "main")
  assert.equal(observation.connectionText, "Connected")
  assert.equal(observation.root.width, 400)
  assert.equal(observation.root.height, 320)
  assert.equal(observation.fallback.present, false)
  assert.equal(observation.bodyText, "Nessa Connected")
})
