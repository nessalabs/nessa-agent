import assert from "node:assert/strict"
import { createHash } from "node:crypto"
import { test } from "node:test"
import { createResourceObserver } from "./resource-read-observer.mjs"
import { resourceReadEvidence } from "./resource-read-evidence.mjs"
import { classifyResourceFailure } from "./browser.mjs"

const bytes = new Uint8Array([60, 112, 62, 111, 107, 60, 47, 112, 62])
const sha256 = (value) => createHash("sha256").update(value).digest("hex")
const expected = { size: bytes.length, sha256: sha256(bytes) }
const gate = () => {
  let release
  const promise = new Promise((resolve) => {
    release = resolve
  })
  return { promise, release }
}

// Actual observer/collector around synthetic originals. Gates schedule original
// client acceptance independently of view publication, without timers or I/O.
function fixture() {
  const collector = resourceReadEvidence("liveness")
  const events = []
  const requests = []
  const controls = new Map()
  let acceptances = 0
  const observer = createResourceObserver((event) => {
    events.push(event)
    collector.receive(event)
  }, "liveness")
  const transport = observer.transport((_url, fetch) => ({
    async get({ ticket, signal }) {
      const response = await fetch("synthetic-only", {
        method: "GET",
        headers: { "x-nessa-resource-ticket": ticket },
        signal,
      })
      const reader = response.body.getReader()
      const chunks = []
      for (;;) {
        const next = await reader.read()
        if (next.done) break
        chunks.push(...next.value)
      }
      return { status: response.status, bytes: new Uint8Array(chunks) }
    },
  }))("synthetic-only", (_url, init) => {
    const control = controls.get(init.headers["x-nessa-resource-ticket"])
    let consumed = false
    return Promise.resolve({
      status: 200,
      body: {
        getReader: () => ({
          read: () =>
            Promise.resolve(
              consumed
                ? { done: true }
                : ((consumed = true), { done: false, value: control.bytes }),
            ),
          cancel: () => Promise.resolve(),
        }),
      },
    })
  })
  const api = observer.api(() => ({
    fetchResource(ticket, description, { signal }) {
      const control = controls.get(ticket)
      control.original = transport
        .get({ ticket, maxBytes: description.size, signal })
        .then(async (reply) => {
          control.consumed.release()
          await control.accept.promise
          assert.equal(reply.status, 200)
          assert.equal(reply.bytes.length, description.size)
          assert.equal(sha256(reply.bytes), description.sha256)
          if (control.reject) throw new Error("synthetic client refusal")
          acceptances++
          return reply.bytes
        })
      return control.original
    },
  }))()
  const mount = () => {
    const owner = new AbortController()
    const address = {}
    let options
    observer.bridge((value) => {
      options = value
      return {}
    })({
      ports: {
        server: {
          readResource(_address, ticket, signal) {
            return api.fetchResource(ticket, expected, { signal })
          },
        },
      },
      onView() {},
    })
    return {
      owner,
      view(kind) {
        options.onView({ lifecycle: { kind } })
      },
      read({ payload = bytes.slice(), reject = false } = {}) {
        const ticket = String(controls.size + 1).padStart(43, "T")
        const control = { bytes: payload, reject, accept: gate(), consumed: gate() }
        controls.set(ticket, control)
        const req = {
          url: () => "http://127.0.0.1:1234/mcp-resources",
          method: () => "GET",
          headers: () => ({ "x-nessa-resource-ticket": ticket }),
          failure: () => ({ errorText: "net::ERR_ABORTED" }),
          existingResponse: () => ({ status: () => 200 }),
        }
        requests.push(req)
        collector.observeRequest(req)
        const promise = options.ports.server.readResource(address, ticket, owner.signal)
        assert.equal(promise, control.original, "original client Promise is returned")
        return { ...control, req, promise }
      },
    }
  }
  return {
    mount,
    collector,
    events,
    requests,
    acceptances: () => acceptances,
    async finish() {
      await observer.finalize()
    },
    state: (read) => collector.readState(read.req),
  }
}

for (const order of ["accept-first", "live-first", "live-before-read"])
  test(`original acceptance and live owner join in either order: ${order}`, async () => {
    const f = fixture()
    const owner = f.mount()
    if (order === "live-before-read") owner.view("live")
    const read = owner.read()
    await read.consumed.promise
    if (order === "live-first") owner.view("live")
    read.accept.release()
    await read.promise
    if (order === "accept-first") owner.view("live")
    await f.finish()
    assert.equal(f.state(read), "verified")
    const classified = { errors: [], harmless: [] }
    classifyResourceFailure(read.req, f.collector, classified)
    assert.equal(classified.harmless.length, 1)
    assert.equal(classified.errors.length, 0)
    assert.equal(f.acceptances(), 1)
    assert.deepEqual(
      f.events.filter((e) => ["accepted", "live"].includes(e.type)).map((e) => e.type),
      ["accepted", "live"],
    )
  })

for (const order of ["successive", "concurrent-forward", "concurrent-reverse"])
  test(`later ${order} reads need no view republish`, async () => {
    const f = fixture()
    const owner = f.mount()
    const first = owner.read()
    first.accept.release()
    await first.promise
    owner.view("live")
    const second = owner.read()
    let third
    if (order !== "successive") third = owner.read()
    if (order === "concurrent-reverse") {
      await third.consumed.promise
      third.accept.release()
      await third.promise
    }
    second.accept.release()
    await second.promise
    if (order === "successive") third = owner.read()
    third.accept.release()
    await third.promise
    await f.finish()
    assert.deepEqual([first, second, third].map(f.state), [
      "verified",
      "verified",
      "verified",
    ])
  })

for (const stop of ["abort", "ending", "gone", "failed"])
  for (const when of ["before-acceptance", "after-acceptance"])
    test(`later read cannot survive ${stop} ${when} or borrow replacement liveness`, async () => {
      const f = fixture()
      const owner = f.mount()
      owner.view("live")
      const read = owner.read()
      await read.consumed.promise
      const end = () => (stop === "abort" ? owner.owner.abort() : owner.view(stop))
      if (when === "before-acceptance") end()
      read.accept.release()
      await read.promise
      if (when === "after-acceptance") end()
      const replacement = f.mount()
      const next = replacement.read()
      next.accept.release()
      await next.promise
      replacement.view("live")
      // Even an obsolete callback on the original cannot repair its failed read.
      owner.view("live")
      await f.finish()
      assert.equal(f.state(read), "invalid")
      assert.equal(f.state(next), "verified")
      const own = f.events.filter((event) => event.read === 1)
      const failure = own.findIndex((event) => event.type === "failed")
      assert.ok(failure >= 0)
      assert.equal(
        own.slice(failure + 1).some((event) => event.type === "live"),
        false,
      )
    })

test("acceptance without an original live owner publishes no live evidence", async () => {
  const f = fixture()
  const owner = f.mount()
  const read = owner.read()
  read.accept.release()
  await read.promise
  await f.finish()
  assert.equal(f.state(read), "invalid")
  assert.equal(
    f.events.some((event) => event.type === "live"),
    false,
  )
})

for (const fault of ["corrupt", "truncated", "rejected", "unaccepted"])
  test(`live owner cannot lend acceptance to a later ${fault} read`, async () => {
    const f = fixture()
    const owner = f.mount()
    owner.view("live")
    const read = owner.read({
      payload:
        fault === "corrupt"
          ? new Uint8Array(bytes.length)
          : fault === "truncated"
            ? bytes.slice(1)
            : bytes.slice(),
      reject: fault === "rejected",
    })
    await read.consumed.promise
    if (fault !== "unaccepted") {
      read.accept.release()
      await read.promise.catch(() => {})
    }
    await f.finish()
    if (fault === "unaccepted") {
      read.accept.release()
      await read.promise
    }
    assert.equal(f.state(read), "invalid")
    const classified = { errors: [], harmless: [] }
    classifyResourceFailure(read.req, f.collector, classified)
    assert.equal(classified.errors.length, 1)
    assert.equal(classified.harmless.length, 0)
  })
