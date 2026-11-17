import assert from "node:assert/strict"
import { it } from "node:test"
import { runInNewContext } from "node:vm"
import {
  captureTeardownRequest,
  captureTeardownAnswer,
  prepareFixtureTeardown,
  teardownEvidence,
  withHeldSampleReply,
} from "./mcp-fixture.mjs"

function observer(install, capture) {
  let listener
  let tabs = 2
  let state = "live"
  const reports = []
  const parent = {}
  const proxy = {}
  const window = {
    parent,
    report(record) {
      reports.push(record)
      return Promise.resolve()
    },
    addEventListener(name, read, options) {
      assert.equal(name, "message")
      assert.equal(options.capture === true, capture)
      options.signal.release = () => {
        listener = undefined
      }
      listener = read
    },
  }
  window.top = window
  const args = {
    origin: "https://proxy.test",
    binding: "report",
    frame: { contentWindow: proxy, isConnected: true },
    tabs: "tabs",
    slot: "bootstrap",
  }
  const owner = runInNewContext(`(${install.toString()})(args)`, {
    window,
    args,
    Date: { now: () => 1 },
    AbortController: class {
      signal = {}
      abort() {
        this.signal.release()
      }
    },
    document: {
      body: { getAttribute: () => state },
      querySelectorAll: () => Array(tabs),
    },
  })
  if (install === captureTeardownAnswer) {
    assert.equal(window.bootstrap, owner)
    owner.pin(args)
    delete window.bootstrap
    assert.equal(Object.hasOwn(window, "bootstrap"), false)
  }
  return {
    parent,
    proxy,
    reports,
    receive(event) {
      listener(event)
    },
    fixtureReceivedRequest() {
      state = "tearing-down"
    },
    repin() {
      owner.pin({ ...args, frame: { contentWindow: {}, isConnected: true } })
    },
    removeTab() {
      tabs -= 1
    },
    release() {
      owner.abort()
      assert.equal(listener, undefined)
    },
  }
}
const request = { jsonrpc: "2.0", id: "actual-request", method: "ui/resource-teardown" }
const answer = { jsonrpc: "2.0", id: "actual-request", result: {} }

it("retains the original parent's request after the fixture publishes its actual state", () => {
  const read = observer(captureTeardownRequest, false)
  read.fixtureReceivedRequest()
  read.receive({ source: read.parent, origin: "https://proxy.test", data: request })
  assert.deepEqual(JSON.parse(JSON.stringify(read.reports)), [
    {
      kind: "request",
      id: request.id,
      state: "tearing-down",
      at: 1,
      origin: "https://proxy.test",
    },
  ])
  read.release()
})
it("unrelated sources/origins/methods and inherited fields cannot fabricate a request", () => {
  const read = observer(captureTeardownRequest, false)
  for (const event of [
    { source: {}, origin: "https://proxy.test", data: request },
    { source: read.parent, origin: "https://other.test", data: request },
    {
      source: read.parent,
      origin: "https://proxy.test",
      data: { ...request, method: "ui/initialize" },
    },
    { source: read.parent, origin: "https://proxy.test", data: Object.create(request) },
    { source: read.parent, origin: "https://proxy.test", data: { ...request, id: null } },
  ])
    read.receive(event)
  assert.equal(read.reports.length, 0)
  read.release()
})
it("drops other sources and origins before touching their payload", () => {
  for (const [install, capture] of [
    [captureTeardownRequest, false],
    [captureTeardownAnswer, true],
  ]) {
    const read = observer(install, capture)
    let reads = 0
    const original = install === captureTeardownRequest ? read.parent : read.proxy
    for (const [source, origin] of [
      [{}, "https://proxy.test"],
      [original, "https://other.test"],
    ]) {
      read.receive({
        source,
        origin,
        get data() {
          reads += 1
          throw new Error("untrusted payload was read")
        },
      })
    }
    assert.equal(reads, 0)
    read.release()
  }
})
it("captures tabs at the original proxy answer before host bubble cleanup", () => {
  const read = observer(captureTeardownAnswer, true)
  read.receive({ source: read.proxy, origin: "https://proxy.test", data: answer })
  read.removeTab()
  assert.equal(read.reports[0].tabs, 2)
  assert.equal(read.reports[0].id, request.id)
  assert.equal(read.reports[0].origin, "https://proxy.test")
  read.release()
})
it("refuses a replacement proxy before it can relabel the captured answer owner", () => {
  const read = observer(captureTeardownAnswer, true)
  assert.throws(() => read.repin(), /already captured/)
  read.receive({ source: read.proxy, origin: "https://proxy.test", data: answer })
  assert.equal(read.reports[0].connected, true)
  read.release()
})
it("substituted proxies, wrong origins, inherited fields and requests are not answers", () => {
  const read = observer(captureTeardownAnswer, true)
  for (const event of [
    { source: {}, origin: "https://proxy.test", data: answer },
    { source: read.proxy, origin: "https://other.test", data: answer },
    { source: read.proxy, origin: "https://proxy.test", data: request },
    { source: read.proxy, origin: "https://proxy.test", data: Object.create(answer) },
  ])
    read.receive(event)
  assert.equal(read.reports.length, 0)
  read.release()
})
const requested = { kind: "request", id: request.id, state: "tearing-down", at: 100 }
const answered = {
  kind: "answer",
  id: request.id,
  result: {},
  tabs: 2,
  connected: true,
  at: 500,
}
it("accepts exact retained evidence after retirement, independent of callback delivery order", () => {
  for (const records of [
    [requested, answered],
    [answered, requested],
  ])
    assert.deepEqual(teardownEvidence(records, 2).failures, [])
})
const withoutCaptureTime = (record) => {
  const copy = { ...record }
  delete copy.at
  return copy
}
for (const [name, records] of [
  ["ACK before request", [requested, { ...answered, at: 99 }]],
  ["equal capture times", [requested, { ...answered, at: 100 }]],
  ["missing request time", [withoutCaptureTime(requested), answered]],
  ["missing ACK time", [requested, withoutCaptureTime(answered)]],
  ...[NaN, Infinity, -Infinity, undefined, "100"].flatMap((at) => [
    [
      `nonfinite or nonnumeric request time ${String(at)}`,
      [{ ...requested, at }, answered],
    ],
    [`nonfinite or nonnumeric ACK time ${String(at)}`, [requested, { ...answered, at }]],
  ]),
])
  it(`rejects ${name} even when identity, tab and reply fields agree`, () => {
    assert.ok(
      teardownEvidence(records, 2).failures.some((failure) =>
        failure.startsWith(
          "the teardown answer did not follow its original request in capture time",
        ),
      ),
    )
  })
it("keeps valid physical chronology when callbacks arrive in reverse order", () => {
  const answer = { ...answered, at: 500.25 }
  const request = { ...requested, at: 100.25 }
  const result = teardownEvidence([answer, request], 2)
  assert.deepEqual(result.failures, [])
  assert.equal(result.answer.at, 500.25)
  assert.equal(result.request.at, 100.25)
})
for (const [name, records] of [
  ["no request", [answered]],
  ["no answer", [requested]],
  ["different id", [requested, { ...answered, id: "another" }]],
  ["never tearing down", [{ ...requested, state: "live" }, answered]],
  ["duplicate requests", [requested, requested, answered]],
  ["duplicate answers", [requested, answered, answered]],
  ["error answer", [requested, { ...answered, error: { code: -1 } }]],
  ["missing result", [requested, { ...answered, result: undefined }]],
  ["early close", [requested, { ...answered, tabs: 1 }]],
  [
    "retired proxy with unchanged tab count",
    [requested, { ...answered, connected: false }],
  ],
  [
    "absent request identity",
    [
      { kind: "request", state: "tearing-down" },
      { ...answered, id: undefined },
    ],
  ],
])
  it(`rejects ${name}; retirement alone cannot supply acknowledgement`, () =>
    assert.notEqual(teardownEvidence(records, 2).failures.length, 0))
it("freezes the analysis cut against later queue and initial-state changes", () => {
  const queue = structuredClone([requested, answered])
  const initial = { tabs: 2, connected: true, at: 1 }
  const result = teardownEvidence(queue, 2, false, initial)
  queue[0].id = "changed-after-analysis"
  queue[1].result.changed = true
  queue.push({ ...answered, id: "late" })
  initial.tabs = 0
  assert.deepEqual(result.records, [requested, answered])
  assert.equal(result.request.id, requested.id)
  assert.deepEqual(result.answer.result, {})
  assert.deepEqual(result.initial, { tabs: 2, connected: true, at: 1 })
  assert.deepEqual(result.failures, [])
})
it("retains an unmatched original answer while refusing its claimed correlation", () => {
  const wrong = { ...answered, id: "another-request", origin: "https://proxy.test" }
  const records = [requested, wrong]
  const result = teardownEvidence(records, 2)
  assert.equal(result.answer, undefined)
  assert.deepEqual(result.records, records)
  assert.notEqual(result.failures.length, 0)
})
it("rejects observation overflow even alongside otherwise valid evidence", () =>
  assert.notEqual(teardownEvidence([requested, answered], 2, true).failures.length, 0))
it("scopes the preboot binding to captured Frames, bounds records and aborts captured owners", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] })
  const cleared = t.mock.method(globalThis, "clearTimeout")
  let receive
  const starts = []
  const context = {
    async addInitScript(install) {
      starts.push("preboot observer")
      assert.equal(install, captureTeardownAnswer)
    },
    async exposeBinding(name, callback) {
      starts.push("binding")
      assert.equal(name, "__mcpFixtureTeardown")
      receive = callback
    },
  }
  const recorder = await prepareFixtureTeardown(context)
  assert.deepEqual(starts, ["binding", "preboot observer"])
  const disposed = [],
    aborted = []
  const handle = (name) => ({
    async evaluate(read) {
      return read({
        abort: () => aborted.push(name),
        initial: { tabs: 2, connected: true, at: 1 },
      })
    },
    async dispose() {
      disposed.push(name)
    },
  })
  const app = {
    isDetached: () => false,
    async evaluateHandle() {
      return handle("app")
    },
  }
  const parent = { isDetached: () => false }
  const element = {
    async dispose() {
      disposed.push("element")
    },
  }
  const proxy = {
    url: () => "https://proxy.test/sandbox",
    async frameElement() {
      return element
    },
  }
  const page = {
    mainFrame: () => parent,
    async evaluateHandle() {
      return handle("page")
    },
  }
  const observed = await recorder.observe(page, { app, proxy }, "tabs")
  const wait = observed.waitWithin(2000)
  let outcome
  void wait.then((value) => {
    outcome = value
  })
  receive({ frame: {} }, requested)
  assert.equal(observed.records.length, 0)
  t.mock.timers.tick(1999)
  for (let index = 0; index < 5; index++) await Promise.resolve()
  assert.equal(outcome, undefined)
  t.mock.timers.tick(1)
  for (let index = 0; index < 5; index++) await Promise.resolve()
  assert.deepEqual(outcome, { kind: "timed-out", withinMs: 2000 })
  assert.equal(cleared.mock.callCount(), 1)
  // A valid late request stays available, but cannot rewrite the earlier bound.
  receive({ frame: app }, requested)
  const late = await observed.waitWithin(2000)
  assert.equal(late.kind, "observed")
  assert.equal(late.request, requested)
  assert.deepEqual(outcome, { kind: "timed-out", withinMs: 2000 })
  assert.equal(cleared.mock.callCount(), 2)
  receive({ frame: parent }, answered)
  assert.deepEqual(teardownEvidence(observed.records, 2).failures, [])
  for (let index = 0; index < 10; index++) receive({ frame: app }, requested)
  assert.equal(observed.records.length, 4)
  assert.equal(observed.overflow, true)
  await observed.close()
  assert.deepEqual(aborted.sort(), ["app", "page"])
  assert.deepEqual(disposed.sort(), ["app", "element", "page"])
})
it("holds before action and resumes after a result or falsy throw", async () => {
  for (const throws of [false, true]) {
    const calls = []
    const page = {
      context: () => ({
        clock: {
          pauseAt: async () => calls.push("paused"),
          resume: async () => calls.push("resumed"),
        },
      }),
    }
    const action = () =>
      withHeldSampleReply(page, async () => {
        calls.push("action")
        if (throws) throw undefined
        return "busy verified"
      })
    if (throws) await assert.rejects(action, (error) => error === undefined)
    else assert.equal(await action(), "busy verified")
    assert.deepEqual(calls, ["paused", "action", "resumed"])
  }
})

it("releases acquired observers when reading the initial snapshot fails", async () => {
  const failure = new Error("initial snapshot unavailable")
  const aborted = [],
    disposed = []
  const context = { async exposeBinding() {}, async addInitScript() {} }
  const recorder = await prepareFixtureTeardown(context)
  const handle = (name) => {
    let calls = 0
    return {
      async evaluate(read) {
        if (name === "page" && calls++ === 0) throw failure
        return read({ abort: () => aborted.push(name) })
      },
      async dispose() {
        disposed.push(name)
      },
    }
  }
  const app = {
    isDetached: () => false,
    async evaluateHandle() {
      return handle("app")
    },
  }
  const parent = { isDetached: () => false }
  const element = {
    async dispose() {
      disposed.push("element")
    },
  }
  const proxy = {
    url: () => "https://proxy.test/sandbox",
    async frameElement() {
      return element
    },
  }
  const page = {
    mainFrame: () => parent,
    async evaluateHandle() {
      return handle("page")
    },
  }
  await assert.rejects(
    () => recorder.observe(page, { app, proxy }, "tabs"),
    (error) => error === failure,
  )
  assert.deepEqual(aborted.sort(), ["app", "page"])
  assert.deepEqual(disposed.sort(), ["app", "element", "page"])
})
