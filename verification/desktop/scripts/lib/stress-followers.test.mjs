import { strict as assert } from "node:assert"
import { test } from "node:test"
import { Follower, replayEvents } from "./stress-followers.mjs"

function fixture({ room = 0, open } = {}) {
  const follower = new Follower({
    n: 0,
    limits: { conversationTargets: 2 + room },
    mainId: "main",
  })
  const held = { subscription: {}, lost: false }
  follower.events.set("event", held)
  follower.view.subscription = {}
  follower.list.conversations = ["main", "first", "second", "third"]
  const closed = []
  const observers = []
  follower.client = {
    connectionState: { status: "connected" },
    subscriptions: {
      async view(id, observer) {
        observers.push(observer)
        const result = await open?.({ follower, held, observers, id })
        if (result === "refused") throw { code: "subscription_capacity" }
        return {
          async close() {
            closed.push(id)
          },
        }
      },
    },
  }
  return { follower, closed }
}

test("a stable full connection proves its capacity", async () => {
  const { follower } = fixture({ open: () => "refused" })
  await follower.probe()
  assert.equal(follower.counts.refusedAsExpected, 1)
  assert.deepEqual(follower.failures, [])
})

test("a stable connection accepting an extra view fails", async () => {
  const { follower, closed } = fixture()
  await follower.probe()
  assert.equal(follower.failures.length, 1)
  assert.match(follower.failures[0], /past 2 was accepted/)
  assert.deepEqual(closed, ["first"])
})

for (const [name, change] of [
  [
    "an original event ending",
    ({ held }) => {
      held.lost = true
    },
  ],
  [
    "replacing an original event",
    ({ held }) => {
      held.subscription = {}
    },
  ],
  ["removing an original event", ({ follower }) => follower.events.delete("event")],
  [
    "the original main view ending",
    ({ follower }) => {
      follower.view.lost = true
    },
  ],
]) {
  test(`${name} invalidates the probe`, async () => {
    const { follower, closed } = fixture({ open: change })
    await follower.probe()
    assert.deepEqual(follower.failures, [])
    assert.equal(follower.counts.refusedAsExpected, 0)
    assert.deepEqual(closed, ["first"])
  })
}

test("an acquired view ending invalidates the probe", async () => {
  const { follower, closed } = fixture({
    room: 1,
    open({ observers }) {
      if (observers.length === 2) {
        observers[0].ended({ reason: "disconnected" })
        return "refused"
      }
    },
  })
  await follower.probe()
  assert.deepEqual(follower.failures, [])
  assert.equal(follower.counts.refusedAsExpected, 0)
  assert.deepEqual(closed, ["first"])
})

function follower() {
  return new Follower({
    n: 0,
    connect: async () => ({}),
    limits: { conversationTargets: 8 },
    random: () => 0,
    mainId: "main",
  })
}

test("a closed change source is followed again", () => {
  const followed = follower()
  let lost = false
  followed.ended("the main view", { reason: "source_closed" }, () => {
    lost = true
  })
  assert.equal(lost, true)
  assert.deepEqual(followed.failures, [])
  assert.equal(followed.counts.resumed, 1)
})

test("a frame that does not fit is still a failure and is not followed again", () => {
  const followed = follower()
  let lost = false
  followed.ended("the main view", { reason: "too_large" }, () => {
    lost = true
  })
  assert.equal(lost, false)
  assert.equal(followed.failures.length, 1)
  assert.match(followed.failures[0], /ended too_large/)
})

async function flushed() {
  for (let turn = 0; turn < 12; turn++) await Promise.resolve()
}

function replayFixture(
  t,
  { capture = true, delayedOpen = false, stalledClose = -1 } = {},
) {
  t.mock.timers.enable({ apis: ["setTimeout"] })
  const calls = []
  let release
  const client = {
    subscriptions: {
      async events(id, handlers, options) {
        const call = { id, handlers, options, closed: 0 }
        calls.push(call)
        let closing
        const subscription = {
          close() {
            if (!closing) {
              call.closed++
              closing =
                calls.indexOf(call) === stalledClose
                  ? new Promise(() => {})
                  : Promise.resolve()
            }
            return closing
          },
        }
        if (calls.length === 1 && capture)
          handlers.batch({ cursor: "captured-head", events: [] })
        if (calls.length === 1 && delayedOpen)
          return new Promise((resolve) => {
            release = () => resolve(subscription)
          })
        return subscription
      },
    },
  }
  const pending = replayEvents(client, "conversation")
  return { calls, pending, release: () => release() }
}

test("reference replay waits through quiet and all pages until the captured head", async (t) => {
  const { calls, pending } = replayFixture(t)
  let completed = false
  void pending.then(() => {
    completed = true
  })
  await flushed()
  assert.equal(calls.length, 2)
  assert.equal(calls[0].options.replay, undefined)
  assert.equal(calls[0].closed, 1, "head capture releases before replay opens")
  assert.equal(calls[1].options.replay, true)
  const first = Array.from({ length: 31 }, (_, index) => ({ id: `event-${index}` }))
  calls[1].handlers.batch({ cursor: "first-page", events: first })
  t.mock.timers.tick(2000)
  await flushed()
  assert.equal(completed, false, "silence after a prefix is not completion")
  const last = Array.from({ length: 24 }, (_, index) => ({ id: `event-${index + 31}` }))
  calls[1].handlers.batch({ cursor: "last-event", events: last })
  t.mock.timers.tick(100)
  await flushed()
  assert.equal(completed, false, "last semantic event is not the outer-save head")
  calls[1].handlers.batch({ cursor: "captured-head", events: [] })
  t.mock.timers.tick(100)
  assert.deepEqual(
    await pending,
    [...first, ...last].map((event) => event.id),
  )
  assert.equal(calls[1].closed, 1)
})

test("an empty checkpoint other than the captured head does not complete replay", async (t) => {
  const { calls, pending } = replayFixture(t)
  let completed = false
  void pending.then(
    () => {
      completed = true
    },
    () => {},
  )
  await flushed()
  calls[1].handlers.batch({ cursor: "other-head", events: [] })
  t.mock.timers.tick(100)
  await flushed()
  assert.equal(completed, false)
  t.mock.timers.tick(59_900)
  await assert.rejects(
    pending,
    /did not complete replay to captured head within 60000 ms/,
  )
  assert.equal(calls[1].options.signal.aborted, true)
  assert.equal(calls[1].closed, 1)
})

test("a stalled prefix fails at the existing overall deadline instead of passing a count", async (t) => {
  const { calls, pending } = replayFixture(t)
  const failed = assert.rejects(
    pending,
    /did not complete replay to captured head within 60000 ms/,
  )
  await flushed()
  calls[1].handlers.batch({ cursor: "prefix", events: [{ id: "first" }] })
  t.mock.timers.tick(59_999)
  await flushed()
  assert.equal(calls[1].closed, 0)
  t.mock.timers.tick(1)
  await failed
  assert.equal(calls[1].closed, 1)
})

test("an empty history completes only with the matching captured checkpoint", async (t) => {
  const { calls, pending } = replayFixture(t)
  await flushed()
  calls[1].handlers.batch({ cursor: "captured-head", events: [] })
  t.mock.timers.tick(100)
  assert.deepEqual(await pending, [])
})

test("head capture and replay share one deadline", async (t) => {
  const { calls, pending } = replayFixture(t, { capture: false })
  const failed = assert.rejects(
    pending,
    /did not complete replay to captured head within 60000 ms/,
  )
  t.mock.timers.tick(59_000)
  await flushed()
  calls[0].handlers.batch({ cursor: "captured-head", events: [] })
  t.mock.timers.tick(100)
  await flushed()
  assert.equal(calls.length, 2)
  t.mock.timers.tick(900)
  await failed
  assert.equal(calls[1].closed, 1)
})

test("missing capture and a late opening both fail and release their sources", async (t) => {
  const { calls, pending, release } = replayFixture(t, {
    capture: false,
    delayedOpen: true,
  })
  const failed = assert.rejects(pending, /did not complete head capture within 60000 ms/)
  t.mock.timers.tick(60_000)
  await failed
  release()
  await flushed()
  assert.equal(calls[0].closed, 1)
  assert.equal(calls.length, 1, "no replay starts after deadline")
})

test("terminal replay failure stays a failure before convergence", async (t) => {
  const { calls, pending } = replayFixture(t)
  const failed = assert.rejects(pending, /ended refused/)
  await flushed()
  calls[1].handlers.ended({ reason: "refused", code: "audit_unavailable" })
  t.mock.timers.tick(100)
  await failed
  assert.equal(calls[1].closed, 1)
})

for (const [stalledClose, phase] of [
  [0, "head capture cleanup"],
  [1, "replay cleanup"],
]) {
  test(`a stalled ${phase} fails without misreporting convergence`, async (t) => {
    const { calls, pending } = replayFixture(t, { stalledClose })
    const failed = assert.rejects(
      pending,
      new RegExp(`did not complete ${phase} within 60000 ms`),
    )
    await flushed()
    if (stalledClose === 1) {
      calls[1].handlers.batch({ cursor: "captured-head", events: [] })
      t.mock.timers.tick(100)
      await flushed()
    }
    t.mock.timers.tick(60_000)
    await failed
    assert.equal(calls[stalledClose].closed, 1)
    assert.equal(calls[stalledClose].options.signal.aborted, true)
    if (stalledClose === 0) assert.equal(calls.length, 1)
  })
}
