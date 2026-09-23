import assert from "node:assert/strict"
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
      claimSession: () => assert.fail("a failed session cannot be claimed"),
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
      claimSession: () => assert.fail("an aborted session cannot be claimed"),
      observeApplication: async () => {
        events.push("application failed")
        throw failure
      },
    }),
    failure,
  )
  assert.deepEqual(events, ["application failed", "session aborted", "session settled"])
})

test("a created session is claimed before a later application failure", async () => {
  const failure = new Error("application PID disappeared")
  let claimed
  await assert.rejects(
    observeWebdriverStartup({
      createSession: async () => ({ sessionId: "session" }),
      claimSession: (session) => {
        claimed = session
      },
      observeApplication: async () => {
        await Promise.resolve()
        throw failure
      },
    }),
    failure,
  )
  assert.deepEqual(claimed, { sessionId: "session" })
})

test("a session acknowledged after application failure is still claimed", async () => {
  const events = []
  const failure = new Error("application PID never appeared")
  let sessionSignal
  let finishSession
  const startup = observeWebdriverStartup({
    createSession: (signal) =>
      new Promise((resolve) => {
        sessionSignal = signal
        finishSession = () => resolve({ sessionId: "late-session" })
      }),
    claimSession: (session) => events.push(`claimed ${session.sessionId}`),
    observeApplication: async () => {
      events.push("application failed")
      throw failure
    },
  })

  assert.ok(sessionSignal)
  await new Promise((resolve) => {
    if (sessionSignal.aborted) resolve()
    else sessionSignal.addEventListener("abort", resolve, { once: true })
  })
  assert.equal(sessionSignal.aborted, true)
  assert.equal(sessionSignal.reason, failure)
  finishSession()
  await assert.rejects(startup, failure)
  assert.deepEqual(events, ["application failed", "claimed late-session"])
})

test("a claim failure cannot erase a session already transferred to its owner", async () => {
  const events = []
  const failure = new Error("session owner rejected its shape")
  let claimed

  await assert.rejects(
    observeWebdriverStartup({
      createSession: async () => ({ sessionId: "owned-session" }),
      claimSession: (session) => {
        claimed = session
        events.push("session claimed")
        throw failure
      },
      observeApplication: pendingUntilAborted(events, "application"),
    }),
    failure,
  )
  assert.deepEqual(claimed, { sessionId: "owned-session" })
  assert.deepEqual(events, [
    "session claimed",
    "application aborted",
    "application settled",
  ])
})

test("successful startup claims the session and returns the application fact", async () => {
  const claimed = []
  assert.equal(
    await observeWebdriverStartup({
      createSession: async () => ({ sessionId: "session" }),
      claimSession: (session) => claimed.push(session),
      observeApplication: async () => 4321,
    }),
    4321,
  )
  assert.deepEqual(claimed, [{ sessionId: "session" }])
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

function pageElement({ width, height, textContent = "" }) {
  return {
    textContent,
    style: { display: "block", visibility: "visible" },
    getBoundingClientRect: () => ({ width, height }),
  }
}

function observePage({ root, fallback, status, width, height }) {
  const transcript = status
    ? { querySelector: (selector) => (selector === "p.nessa-text-3" ? status : null) }
    : null
  const document = {
    title: "Nessa",
    readyState: "complete",
    body: { innerText: "Nessa Connected" },
    querySelector: (selector) =>
      ({
        "[data-nessa-root]": root,
        "[data-nessa-load-fallback]": fallback,
        '[aria-label$=" transcript, 0 sent"]': transcript,
      })[selector] ?? null,
  }
  return new Function(
    "document",
    "location",
    "URL",
    "getComputedStyle",
    "innerWidth",
    "innerHeight",
    nativePanelObservationScript,
  )(
    document,
    { href: "tauri://localhost/index.html?surface=main" },
    URL,
    (element) => element.style,
    width,
    height,
  )
}

test("the page observation reads the root, status, surface, and viewport", () => {
  const observation = observePage({
    root: pageElement({ width: 400, height: 320 }),
    fallback: null,
    status: pageElement({ width: 20, height: 10, textContent: "Connected" }),
    width: 1440,
    height: 900,
  })

  assert.equal(observation.url, "tauri://localhost/index.html?surface=main")
  assert.equal(observation.surface, "main")
  assert.equal(observation.connectionText, "Connected")
  assert.equal(observation.root.width, 400)
  assert.equal(observation.root.height, 320)
  assert.equal(observation.fallback.present, false)
  assert.equal(observation.bodyText, "Nessa Connected")
  assert.deepEqual(observation.viewport, { width: 1440, height: 900 })
})

test("the page observation retains a fallback when root and status are absent", () => {
  const observation = observePage({
    root: null,
    fallback: pageElement({
      width: 320,
      height: 320,
      textContent: "Loading Nessa… If this stays on screen",
    }),
    status: null,
    width: 400,
    height: 320,
  })

  assert.deepEqual(observation.root, { present: false })
  assert.equal(observation.fallback.present, true)
  assert.equal(observation.fallback.width, 320)
  assert.match(observation.fallback.text, /Loading Nessa/)
  assert.equal(observation.connectionText, null)
  assert.deepEqual(observation.viewport, { width: 400, height: 320 })
})
