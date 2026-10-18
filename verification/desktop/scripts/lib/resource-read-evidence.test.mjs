import assert from "node:assert/strict"
import { createHash } from "node:crypto"
import { test } from "node:test"
import { EventEmitter } from "node:events"
import { classifyResourceFailure, recordFailedRequest } from "./browser.mjs"
import { resourceReadEvidence } from "./resource-read-evidence.mjs"
import { createResourceObserver } from "./resource-read-observer.mjs"
import {
  decorateResourceModule,
  installResourceObservation,
} from "./resource-read-modules.mjs"
import { watchLines } from "./page-lines.mjs"

const ticket = "S".repeat(43)
const bytes = new Uint8Array([60, 112, 62, 120, 60, 47, 112, 62])
const expected = {
  size: bytes.length,
  sha256: createHash("sha256").update(bytes).digest("hex"),
}
const request = (overrides = {}) => ({
  url: () => "http://127.0.0.1:2345/mcp-resources",
  method: () => "GET",
  headers: () => ({ "x-nessa-resource-ticket": ticket }),
  failure: () => ({ errorText: "net::ERR_ABORTED" }),
  existingResponse: () => ({
    status: () => 200,
    headers: () => ({ "content-length": String(bytes.length) }),
  }),
  sizes: () => ({ responseBodySize: bytes.length }),
  ...overrides,
})

function proof({ change = () => {}, requestFirst = false, req = request() } = {}) {
  const collector = resourceReadEvidence("page")
  if (requestFirst) collector.observeRequest(req)
  const events = [
    {
      type: "begin",
      read: 1,
      owner: 1,
      ticket,
      size: expected.size,
      digest: expected.sha256,
    },
    { type: "chunk", read: 1, bytes: [...bytes] },
    { type: "eof", read: 1 },
    { type: "transport", read: 1 },
    { type: "accepted", read: 1 },
    { type: "live", read: 1 },
    { type: "cutoff", failed: false, activation: { bridge: 1, api: 1, transport: 1 } },
  ]
  change(events)
  events.forEach((event, index) => {
    if (!requestFirst && index === events.length - 1) collector.observeRequest(req)
    collector.receive({ generation: "page", sequence: index + 1, ...event })
  })
  return { collector, req }
}

for (const requestFirst of [true, false])
  test(`exact Request joins in either delivery order (${requestFirst})`, () => {
    const { collector, req } = proof({ requestFirst })
    assert.equal(collector.readState(req), "verified")
    assert.equal(collector.readState(request()), "invalid")
    const legacy = { errors: [], harmless: [] }
    recordFailedRequest(req, "http://127.0.0.1:1438/desktop.html", legacy)
    assert.equal(
      legacy.errors.length,
      1,
      "fail-before: original cross-origin classifier rejects the completed read",
    )
    const actual = { errors: [], harmless: [] }
    classifyResourceFailure(req, collector, actual)
    assert.deepEqual(actual.errors, [])
    assert.match(actual.harmless[0], /consumed EOF.*#996/)
  })

const invalid = {
  corrupt: (events) => {
    events[1].bytes[0] ^= 1
  },
  truncated: (events) => {
    events[1].bytes.pop()
  },
  overlong: (events) => {
    events[1].bytes.push(0)
  },
  missingEOF: (events) => {
    events.splice(2, 1)
  },
  missingTransport: (events) => {
    events.splice(3, 1)
  },
  missingAcceptance: (events) => {
    events.splice(4, 1)
  },
  missingLive: (events) => {
    events.splice(5, 1)
  },
  cancelAfterLive: (events) => {
    events.splice(6, 0, { type: "failed", read: 1 })
  },
  cancelBeforeEOF: (events) => {
    events.splice(2, 0, { type: "failed", read: 1 })
  },
  wrongRead: (events) => {
    events[4].read = 2
  },
  wrongOwner: (events) => {
    events[0].owner = 0
  },
  wrongDigest: (events) => {
    events[0].digest = "0".repeat(64)
  },
  wrongSize: (events) => {
    events[0].size++
  },
  wrongGeneration: (events) => {
    events[1].generation = "other"
  },
  outOfOrder: (events) => {
    ;[events[2], events[4]] = [events[4], events[2]]
  },
  overflow: (events) => {
    events.at(-1).failed = true
  },
  inactive: (events) => {
    events.at(-1).activation.transport = 0
  },
  noBarrier: (events) => {
    events.pop()
  },
  forgedField: (events) => {
    events[0].proof = true
  },
}
for (const [name, change] of Object.entries(invalid))
  test(`${name} remains an error despite full same-origin sizes and a live pattern`, () => {
    const { collector, req } = proof({ change, requestFirst: true })
    assert.equal(collector.readState(req), "invalid")
    const into = watchLines()
    into.noteHarmless(/.*/)
    classifyResourceFailure(req, collector, into)
    assert.equal(into.errors.length, 1)
    assert.deepEqual(into.harmless, [])
  })

test("a duplicate Request poisons an otherwise complete read through cutoff", () => {
  const { collector, req } = proof({
    requestFirst: true,
    change: (events) => events.pop(),
  })
  const duplicate = request()
  collector.observeRequest(duplicate)
  collector.receive({
    generation: "page",
    sequence: 7,
    type: "cutoff",
    failed: false,
    activation: { bridge: 1, api: 1, transport: 1 },
  })
  assert.equal(collector.readState(req), "invalid")
  assert.equal(collector.readState(duplicate), "invalid")
})

test("non-abort, wrong status and other endpoints never gain the exception", () => {
  for (const req of [
    request({ failure: () => ({ errorText: "ERR_FAILED" }) }),
    request({ existingResponse: () => ({ status: () => 204 }) }),
    request({ url: () => "http://127.0.0.1:2345/other" }),
    request({ method: () => "OPTIONS" }),
  ]) {
    const into = { errors: [], harmless: [] }
    const { collector } = proof({ req, requestFirst: true })
    classifyResourceFailure(req, collector, into)
    assert.equal(into.errors.length, 1)
  }
})

test("served export decoration fails closed on changed or duplicated contracts", () => {
  assert.match(
    decorateResourceModule(
      "export function factory() {}",
      "factory",
      "api",
      "/observer.mjs",
    ),
    /__nessaObservation.api\(factory\)/,
  )
  for (const source of [
    "export const factory = () => {}",
    "export function factory(){}; export function factory(){}",
    "function factory(){}",
  ])
    assert.throws(
      () => decorateResourceModule(source, "factory", "api", "/observer.mjs"),
      /unsupported/,
    )
})

async function observedRead({
  corrupt = false,
  canceled = false,
  delayed = false,
  chunk = bytes,
  pieces = [chunk],
  size = expected.size,
  cancelDigest = false,
  callbackThrows = false,
} = {}) {
  const collector = resourceReadEvidence("page")
  const req = request()
  collector.observeRequest(req)
  let release
  const gate = new Promise((resolve) => {
    release = resolve
  })
  const events = []
  let cutoffFailed
  let consumed = 0
  const observer = createResourceObserver(async (event) => {
    if (delayed) await gate
    events.push(event.type)
    if (event.type === "cutoff") cutoffFailed = event.failed
    collector.receive(event)
  }, "page")
  const outer = new AbortController()
  const inner = new AbortController()
  let originalFetchPromise
  let originalReadPromise
  let originalApiPromise
  const transport = observer.transport((_url, fetch) => ({
    get({ ticket: token, signal }) {
      const fetching = fetch("http://127.0.0.1:2345/mcp-resources", {
        method: "GET",
        headers: { "x-nessa-resource-ticket": token },
        signal,
      })
      assert.equal(fetching, originalFetchPromise, "fetch Promise identity")
      return fetching.then(async (response) => {
        const reader = response.body.getReader()
        const result = []
        for (;;) {
          const reading = reader.read()
          assert.equal(reading, originalReadPromise, "reader Promise identity")
          const read = await reading
          if (read.done) break
          result.push(read.value)
          consumed++
          if (canceled) await reader.cancel()
        }
        const assembled = new Uint8Array(
          result.reduce((sum, value) => sum + value.length, 0),
        )
        let offset = 0
        for (const value of result) {
          assembled.set(value, offset)
          offset += value.length
        }
        return { status: 200, bytes: assembled }
      })
    },
  }))("unused", () => {
    let count = 0
    const reader = {
      read() {
        originalReadPromise = Promise.resolve(
          count < pieces.length
            ? { done: false, value: pieces[count++] }
            : { done: true },
        )
        return originalReadPromise
      },
      cancel() {
        return Promise.resolve()
      },
    }
    originalFetchPromise = Promise.resolve({
      status: 200,
      body: { getReader: () => reader },
    })
    return originalFetchPromise
  })
  const api = observer.api(() => ({
    fetchResource(token) {
      originalApiPromise = transport
        .get({ ticket: token, maxBytes: size, signal: inner.signal })
        .then(async (reply) => {
          if (cancelDigest) outer.abort()
          if (corrupt) throw new Error("synthetic integrity refusal")
          return reply.bytes
        })
      return originalApiPromise
    },
  }))()
  let returnedPromise
  const bridge = observer.bridge((options) => {
    returnedPromise = options.ports.server.readResource(
      {},
      "ui://synthetic",
      outer.signal,
    )
    assert.equal(
      returnedPromise,
      originalApiPromise,
      "API wrapper preserves original Promise",
    )
    return {
      ready: returnedPromise.then(() => {
        options.onView({ lifecycle: { kind: "live" } })
      }),
    }
  })({
    ports: {
      server: {
        readResource(_address, _uri, signal) {
          return api.fetchResource(ticket, { ...expected, size }, { signal })
        },
      },
    },
    onView() {
      if (callbackThrows) throw new Error("synthetic callback fault")
    },
  })
  await bridge.ready.catch(() => {})
  release()
  await observer.finalize()
  return {
    collector,
    req,
    events,
    outer,
    originalFetchPromise,
    originalReadPromise,
    cutoffFailed,
    consumed,
  }
}

test("producer observes constructor reads and live callbacks; delayed binding keeps browser causal order", async () => {
  for (const delayed of [false, true]) {
    const { collector, req, events } = await observedRead({ delayed })
    assert.equal(collector.readState(req), "verified")
    assert.deepEqual(events, [
      "begin",
      "chunk",
      "eof",
      "transport",
      "accepted",
      "live",
      "cutoff",
    ])
  }
})
for (const options of [
  { corrupt: true },
  { canceled: true },
  { cancelDigest: true },
  { callbackThrows: true },
  { size: 1 },
  { chunk: new Uint8Array(4194305) },
])
  test(`producer refuses ${JSON.stringify(options, (key, value) => (key === "chunk" ? "oversize" : value))}`, async () => {
    const { collector, req } = await observedRead(options)
    assert.equal(collector.readState(req), "invalid")
  })

test("credential and body sentinels never enter the safe summary", async () => {
  const { collector } = await observedRead()
  const summary = JSON.stringify(collector.summary())
  assert.equal(summary.includes(ticket), false)
  assert.equal(summary.includes(expected.sha256), false)
  assert.equal(summary.includes(Buffer.from(bytes).toString()), false)
})

test("forged read states and collector-shaped objects cannot mint classification", () => {
  const req = request()
  for (const forged of [
    "verified",
    { readState: () => "verified" },
    { verified: true },
    null,
  ]) {
    const into = { errors: [], harmless: [] }
    classifyResourceFailure(req, forged, into)
    assert.equal(into.errors.length, 1)
    assert.deepEqual(into.harmless, [])
  }
})

test("disposed observation rejects late delivery and new identities without retaining them", () => {
  const { collector } = proof({ requestFirst: true })
  collector.dispose()
  collector.receive({
    generation: "page",
    sequence: 8,
    type: "begin",
    read: 2,
    owner: 2,
    ticket: "L".repeat(43),
    ...expected,
  })
  assert.equal(collector.observeRequest(request()), false)
  assert.equal(collector.summary().reads, 0)
  assert.equal(collector.summary().requests, 0)
})

test("a tiny-chunk flood with stalled receipts fails bounded observation while the app continues", async () => {
  const result = await observedRead({
    pieces: Array.from({ length: 200 }, () => new Uint8Array([1])),
    size: 200,
    delayed: true,
  })
  assert.equal(
    result.consumed,
    200,
    "the observer never throttles or stops the app reader",
  )
  assert.equal(result.cutoffFailed, true, "queued-message overflow is permanent")
  assert.ok(result.events.length <= 129, "bounded queued events plus the final cutoff")
  assert.equal(result.collector.readState(result.req), "invalid")
})

test("the retained Request budget fails explicitly instead of evicting duplicate evidence", () => {
  const collector = resourceReadEvidence("page")
  for (let i = 0; i < 513; i++)
    collector.observeRequest(
      request({
        headers: () => ({ "x-nessa-resource-ticket": String(i).padStart(43, "A") }),
      }),
    )
  assert.equal(collector.summary().failed, true)
  assert.equal(collector.summary().requests, 512)
})

test("an unowned transport read fails observation without changing its original Promise", async () => {
  let cutoff
  const observer = createResourceObserver((event) => {
    if (event.type === "cutoff") cutoff = event
  }, "page")
  const original = Promise.resolve({ status: 200, bytes })
  const transport = observer.transport(() => ({ get: () => original }))(
    "unused",
    () => {},
  )
  assert.equal(transport.get({ ticket, maxBytes: bytes.length }), original)
  await observer.finalize()
  assert.equal(cutoff.failed, true)
})

test("foreign-frame bindings and a second main-frame navigation invalidate the generation", async () => {
  for (const fault of ["frame", "navigation"]) {
    const page = new EventEmitter()
    const frame = {}
    let binding
    page.mainFrame = () => frame
    page.exposeBinding = async (_name, callback) => {
      binding = callback
    }
    page.route = async () => {}
    const observed = await installResourceObservation(page)
    if (fault === "frame") binding({ page, frame: {} }, {})
    else {
      page.emit("framenavigated", frame)
      assert.equal(observed.collector.summary().failed, false)
      page.emit("framenavigated", frame)
    }
    assert.equal(observed.collector.summary().failed, true)
  }
})

test("new endpoint admission after cutoff cannot borrow a completed live read", () => {
  const { collector, req } = proof({ requestFirst: true })
  assert.equal(collector.readState(req), "verified")
  collector.observeRequest(
    request({ headers: () => ({ "x-nessa-resource-ticket": "N".repeat(43) }) }),
  )
  assert.equal(collector.readState(req), "invalid")
})

test("many small reads cannot exceed the retained read budget", () => {
  const collector = resourceReadEvidence("page")
  for (let id = 1; id <= 257; id++)
    collector.receive({
      generation: "page",
      sequence: id,
      type: "begin",
      read: id,
      owner: 1,
      ticket: String(id).padStart(43, "A"),
      size: 0,
      digest: createHash("sha256").digest("hex"),
    })
  assert.equal(collector.summary().reads, 256)
  assert.equal(collector.summary().failed, true)
})

test("mount overflow fails observation while still invoking every original factory", async () => {
  let cutoff
  let calls = 0
  const observer = createResourceObserver((event) => {
    if (event.type === "cutoff") cutoff = event
  }, "page")
  const factory = observer.bridge(() => {
    calls++
    return {}
  })
  for (let id = 0; id < 129; id++) factory({ ports: { server: {} }, onView() {} })
  await observer.finalize()
  assert.equal(calls, 129)
  assert.equal(cutoff.failed, true)
})
