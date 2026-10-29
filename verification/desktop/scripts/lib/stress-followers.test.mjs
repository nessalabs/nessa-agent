import assert from "node:assert/strict"
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
  follower.list.subscription = {}
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
  const closed = []
  const opened = []
  const subject = new Follower({
    n: 1,
    connect() {},
    limits: { conversationTargets: 3 },
    random: () => 0,
    mainId: "main",
  })
  subject.list.subscription = { id: "list", async close() {} }
  subject.view.subscription = { id: "main", async close() {} }
  subject.list.conversations = ["main", "first", "deleting", "extra"]
  subject.client = {
    connectionState: { status: "connected" },
    subscriptions: {
      async view(id) {
        opened.push(id)
        if (id === "extra")
          throw Object.assign(new Error("capacity"), { code: "subscription_capacity" })
        return {
          async close() {
            closed.push(id)
          },
        }
      },
    },
  }
  return { subject, opened, closed }
}

async function interruptedProbe({ marked, code = "conversation_deleted" }) {
  const f = follower()
  let entered, refuse
  const opening = new Promise((resolve) => {
    entered = resolve
  })
  const response = new Promise((_, reject) => {
    refuse = reject
  })
  const view = f.subject.client.subscriptions.view
  f.subject.client.subscriptions.view = async (id) => {
    if (id !== "deleting") return view(id)
    entered()
    return response
  }
  const pending = f.subject.probe()
  await opening
  if (marked) f.subject.deleted.add(marked)
  refuse(Object.assign(new Error(code), { code }))
  await pending
  assert.deepEqual(f.closed, ["first"])
  assert.equal(f.subject.counts.refusedAsExpected, 0)
  return {
    ...f,
    restore: () => {
      f.subject.client.subscriptions.view = view
    },
  }
}

test("probe deletion during awaited admission closes prior views and permits a later capacity probe", async () => {
  const f = await interruptedProbe({ marked: "deleting" })
  assert.deepEqual(f.subject.failures, [])
  f.restore()
  f.subject.list.conversations = ["main", "first", "replacement", "extra"]
  await f.subject.probe()
  assert.deepEqual(f.subject.failures, [])
  assert.equal(f.subject.counts.refusedAsExpected, 1)
  assert.deepEqual(f.closed, ["first", "first", "replacement"])
})

for (const marked of [undefined, "another-target"]) {
  test(`unmarked probe deletion remains a failure with marker ${marked}`, async () => {
    const f = await interruptedProbe({ marked })
    assert.equal(f.subject.failures.length, 1)
    assert.match(
      f.subject.failures[0],
      /subscription 3 of 3 for deleting failed conversation_deleted$/,
    )
    f.subject.deleted.add("deleting")
    assert.equal(
      f.subject.failures.length,
      1,
      "later deletion cannot erase a recorded failure",
    )
  })
}

for (const code of [
  "subscription_capacity",
  "permission_denied",
  "conversation_not_found",
]) {
  test(`known probe target deletion does not excuse ${code}`, async () => {
    const f = await interruptedProbe({ marked: "deleting", code })
    assert.equal(f.subject.failures.length, 1)
    assert.ok(f.subject.failures[0].endsWith(`failed ${code}`))
  })
}

test("live capacity probe requires admission within room and the exact refusal past it", async () => {
  const f = follower()
  await f.subject.probe()
  assert.deepEqual(f.subject.failures, [])
  assert.equal(f.subject.counts.refusedAsExpected, 1)
  assert.deepEqual(f.opened, ["first", "deleting", "extra"])
  assert.deepEqual(f.closed, ["first", "deleting"])
})

test("accepted excess subscription remains a failure and all opened views close", async () => {
  const f = follower()
  f.subject.client.subscriptions.view = async (id) => ({
    async close() {
      f.closed.push(id)
    },
  })
  await f.subject.probe()
  assert.equal(f.subject.failures.length, 1)
  assert.match(f.subject.failures[0], /past 3 was accepted$/)
  assert.equal(f.subject.counts.refusedAsExpected, 0)
  assert.deepEqual(f.closed, ["first", "deleting", "extra"])
})

function streams(kind, timing = "after-open-answer") {
  const f = follower()
  f.subject.view.subscription = null
  f.subject.list.subscription = null
  const calls = []
  const closed = []
  let entered, release
  const opening = new Promise((resolve) => {
    entered = resolve
  })
  const answer = new Promise((resolve) => {
    release = resolve
  })
  f.subject.client.subscriptions[kind] = async (...args) => {
    const handlers = kind === "list" ? args[0] : args[1]
    const options = kind === "list" ? args[1] : args[2]
    const n = calls.length
    const handle = {
      id: `${kind}-${n}`,
      async close() {
        closed.push(n)
      },
    }
    calls.push({ handlers, options, handle })
    entered()
    if (timing === "before-open-answer" && n === 0) await answer
    return handle
  }
  const open = () =>
    kind === "list"
      ? f.subject.openList()
      : kind === "view"
        ? f.subject.openView()
        : f.subject.follow("other")
  const held = () => (kind === "events" ? f.subject.events.get("other") : f.subject[kind])
  const deliver = (n, revision = "1") => {
    const handlers = calls[n].handlers
    if (kind === "view")
      handlers.view({
        cursor: { incarnation: "history", position: revision },
        view: { messages: [] },
      })
    else if (kind === "list")
      handlers.list({
        conversations: [{ conversationId: revision === "1" ? "main" : "stale" }],
      })
    else handlers.batch({ events: [{ id: "event-1" }], cursor: "event-cursor" })
  }
  return { ...f, calls, closed, opening, release, open, held, deliver }
}

for (const kind of ["view", "list", "events"]) {
  for (const timing of ["before-open-answer", "after-open-answer"]) {
    for (const reason of [
      "source_closed",
      "disconnected",
      "lagging",
      "invalid_frame",
      "too_large",
    ]) {
      test(`${kind}: ${reason} ${timing} retires, classifies reopening and rejects stale callbacks`, async () => {
        const f = streams(kind, timing)
        const pending = f.open()
        await f.opening
        if (timing === "after-open-answer") await pending
        f.deliver(0)
        f.calls[0].handlers.ended({ reason })
        f.release()
        await pending
        assert.equal(f.held().subscription, null)
        const resumable = ["source_closed", "disconnected", "lagging"].includes(reason)
        assert.equal(f.held().lost, resumable)
        assert.equal(f.subject.held(), 0)
        assert.deepEqual(f.closed, timing === "before-open-answer" ? [0] : [])
        const failed = resumable ? 0 : 1
        assert.equal(f.subject.failures.length, failed)
        await f.open()
        assert.equal(f.held().subscription, f.calls[1].handle)
        if (kind === "view")
          assert.deepEqual(f.calls[1].options, {
            after: { incarnation: "history", position: "1" },
          })
        if (kind === "events")
          assert.deepEqual(f.calls[1].options, { after: "event-cursor" })
        assert.equal(f.subject.held(), kind === "list" ? 0 : 1)
        // An old callback cannot replace application progress or lose the new handle.
        const applied =
          kind === "view"
            ? f.held().frame
            : kind === "list"
              ? f.held().conversations
              : f.held().cursor
        f.deliver(0, "2")
        assert.deepEqual(
          kind === "view"
            ? f.held().frame
            : kind === "list"
              ? f.held().conversations
              : f.held().cursor,
          applied,
        )
        f.calls[0].handlers.ended({ reason: "invalid_frame" })
        assert.equal(f.held().subscription, f.calls[1].handle)
        assert.equal(f.held().lost, false)
        assert.equal(f.subject.failures.length, failed)
        if (kind === "events") assert.deepEqual(f.held().ids, ["event-1"])
      })
    }
  }
}

for (const kind of ["view", "list", "events"]) {
  test(`a missing ${kind} defers capacity measurement`, async () => {
    const f = follower()
    if (kind === "events") f.subject.events.set("other", { subscription: null })
    else f.subject[kind].subscription = null
    await f.subject.probe()
    assert.deepEqual(f.opened, [])
    assert.deepEqual(f.subject.failures, [])
  })
}

for (const end of ["owned", "probe"]) {
  for (const answer of ["accepted", "refused"]) {
    test(`${end} stream ends during ${answer} probe admission: retire the measurement and close prior handles`, async () => {
      const f = follower()
      const normal = f.subject.client.subscriptions.view
      f.subject.client.subscriptions.view = async (id, handlers) => {
        const handle = await normal(id)
        if (end === "owned") f.subject.list.subscription = null
        else handlers.ended({ reason: "source_closed" })
        if (answer === "refused") {
          await handle.close()
          throw Object.assign(new Error("capacity"), { code: "subscription_capacity" })
        }
        return handle
      }
      await f.subject.probe()
      assert.deepEqual(f.opened, ["first"])
      assert.deepEqual(f.closed, ["first"])
      assert.deepEqual(f.subject.failures, [])
      assert.equal(f.subject.counts.refusedAsExpected, 0)
    })
  }
}

test("deletion before an events open answer closes the late handle without retaining a target", async () => {
  const f = streams("events", "before-open-answer")
  const pending = f.open()
  await f.opening
  f.subject.deleted.add("other")
  f.release()
  await pending
  assert.equal(f.subject.events.size, 0)
  assert.deepEqual(f.closed, [0])
  assert.deepEqual(f.subject.failures, [])
})

test("intentional events churn retires callbacks before close and keeps its applied cursor", async () => {
  const f = streams("events")
  await f.open()
  f.deliver(0)
  f.calls[0].handle.close = async () => {
    f.closed.push(0)
    f.calls[0].handlers.ended({ reason: "invalid_frame" })
  }
  await f.subject.churn("other")
  assert.deepEqual(f.closed, [0])
  assert.deepEqual(f.calls[1].options, { after: "event-cursor" })
  assert.equal(f.subject.counts.churned, 1)
  assert.equal(f.held().subscription, f.calls[1].handle)
  assert.deepEqual(f.subject.failures, [])
})

test("an events admission refusal cannot retain callback ownership into the next opening", async () => {
  const f = streams("events")
  const normal = f.subject.client.subscriptions.events
  f.subject.client.subscriptions.events = async (...args) => {
    await normal(...args)
    throw Object.assign(new Error("capacity"), { code: "subscription_capacity" })
  }
  await f.open()
  assert.equal(f.held().subscription, null)
  assert.equal(f.held().lost, true)
  assert.equal(f.subject.failures.length, 1)
  f.calls[0].handlers.ended({ reason: "invalid_frame" })
  assert.equal(f.subject.failures.length, 1)
  f.subject.client.subscriptions.events = normal
  await f.open()
  f.calls[0].handlers.ended({ reason: "invalid_frame" })
  assert.equal(f.held().subscription, f.calls[1].handle)
  assert.equal(f.subject.failures.length, 1)
})

test("desired event targets keep their reserved room when failed reopening leaves no live handles", async () => {
  const f = follower()
  f.subject.events.set("first", { subscription: null, lost: true })
  f.subject.events.set("second", { subscription: null, lost: true })
  f.subject.list.conversations = ["new"]
  const followed = []
  f.subject.follow = async (id) => {
    followed.push(id)
  }
  f.subject.random = () => {
    f.subject.stopping = true
    return 1
  }
  await f.subject.run()
  assert.deepEqual(followed, ["first", "second"])
  assert.equal(f.subject.held(), 1)
})

test("settlement reopens the list as well as the view and events, then agrees with reads", async () => {
  const f = follower()
  f.subject.list.subscription = null
  f.subject.list.lost = true
  f.subject.client.subscriptions.list = async (handlers) => {
    handlers.list({ conversations: [{ conversationId: "main" }] })
    return { id: "new-list", async close() {} }
  }
  f.subject.view.frame = { view: { messages: [] } }
  await f.subject.settle()
  assert.equal(f.subject.list.subscription.id, "new-list")
  const failures = await f.subject.check({
    conversation: {
      async read() {
        return { messages: [] }
      },
      async list() {
        return { conversations: [{ conversationId: "main" }] }
      },
    },
  })
  assert.deepEqual(failures, [])
})

for (const kind of ["view", "list", "events"]) {
  test(`${kind}: transient refusal then success retires lost state without duplicate admission`, async () => {
    const f = streams(kind)
    await f.open()
    f.deliver(0)
    f.calls[0].handlers.ended({ reason: "source_closed" })
    let attempts = 0
    let accepted = false
    const subscribe = f.subject.client.subscriptions[kind]
    f.subject.client.subscriptions[kind] = async (...args) => {
      attempts += 1
      if (attempts === 1)
        throw Object.assign(new Error("temporarily_unavailable"), {
          code: "temporarily_unavailable",
        })
      if (accepted)
        throw Object.assign(new Error("subscription_duplicate"), {
          code: "subscription_duplicate",
        })
      const handle = await subscribe(...args)
      accepted = true
      return handle
    }
    await f.open()
    assert.equal(f.held().subscription, f.calls[1].handle)
    assert.equal(f.held().lost, false)
    if (kind === "view")
      assert.deepEqual(f.calls[1].options, {
        after: { incarnation: "history", position: "1" },
      })
    if (kind === "events") assert.deepEqual(f.calls[1].options, { after: "event-cursor" })
    f.subject.list.conversations = []
    f.subject.random = () => 0.95
    const stop = setTimeout(() => {
      f.subject.stopping = true
    }, 20)
    try {
      await f.subject.run()
    } finally {
      clearTimeout(stop)
    }
    assert.equal(attempts, 2, "a live replacement is not subscribed again")
    assert.deepEqual(f.subject.failures, [])
  })
}

async function flushed() {
  for (let turn = 0; turn < 20; turn++) await Promise.resolve()
}

function replayFixture(
  t,
  { capture = true, heads = ["head"], delayedOpen = false, stalledClose = -1 } = {},
) {
  t.mock.timers.enable({ apis: ["setTimeout"] })
  const calls = []
  let release,
    lastHead = "head"
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
        if (!options.replay && options.after === undefined && capture) {
          lastHead = heads.shift() ?? lastHead
          handlers.batch({ cursor: lastHead, events: [] })
        }
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

const batch = (call, cursor, ids = []) =>
  call.handlers.batch({ cursor, events: ids.map((id) => ({ id })) })

async function advanced(t, ms = 100) {
  t.mock.timers.tick(ms)
  await flushed()
}

test("reference replay waits through quiet and every page until fresh-head convergence", async (t) => {
  const { calls, pending } = replayFixture(t)
  let completed = false
  void pending.then(() => {
    completed = true
  })
  await flushed()
  assert.equal(calls.length, 1)
  assert.equal(calls[0].options.replay, true)
  const first = Array.from({ length: 31 }, (_, index) => `event-${index}`)
  batch(calls[0], "first-page", first)
  await advanced(t, 2000)
  assert.equal(completed, false, "silence after a prefix is not completion")
  const last = Array.from({ length: 24 }, (_, index) => `event-${index + 31}`)
  batch(calls[0], "last-event", last)
  await advanced(t)
  assert.equal(completed, false, "event count is not the outer-save head")
  batch(calls[0], "head")
  await advanced(t)
  assert.deepEqual(await pending, [...first, ...last])
  assert.equal(calls.length, 2)
  assert.equal(calls[0].closed, 1)
  assert.equal(calls[1].options.replay, undefined)
  assert.equal(calls[1].closed, 1)
})

test("an intermediate empty checkpoint resumes instead of claiming completion", async (t) => {
  const { calls, pending } = replayFixture(t)
  let completed = false
  void pending.then(
    () => {
      completed = true
    },
    () => {},
  )
  await flushed()
  batch(calls[0], "intermediate", ["first"])
  batch(calls[0], "intermediate")
  await advanced(t)
  assert.equal(completed, false)
  assert.equal(calls[2].options.after, "intermediate")
  assert.equal(calls[2].options.replay, undefined)
  const failed = assert.rejects(
    pending,
    /did not complete replay to checkpoint within 60000 ms/,
  )
  t.mock.timers.tick(59_900)
  await failed
  assert.equal(calls[2].options.signal.aborted, true)
  assert.equal(calls[2].closed, 1)
})

test("head growth resumes after the applied candidate without duplicating late callbacks", async (t) => {
  const { calls, pending } = replayFixture(t, { heads: ["new-head", "new-head"] })
  await flushed()
  batch(calls[0], "last-old-event", ["one", "two"])
  batch(calls[0], "old-head")
  batch(calls[0], "late-frame", ["three"])
  await advanced(t)
  assert.equal(calls.length, 3)
  assert.equal(calls[2].options.after, "old-head")
  assert.equal(calls[2].options.replay, undefined)
  assert.equal(calls[1].closed, 1, "capture releases before resumed replay")
  batch(calls[0], "retired-frame", ["three"])
  batch(calls[2], "new-event", ["three"])
  batch(calls[2], "new-head")
  await advanced(t)
  assert.deepEqual(await pending, ["one", "two", "three"])
  assert.ok(calls.every((call) => call.closed === 1))
})

test("a stalled prefix fails at the existing overall deadline", async (t) => {
  const { calls, pending } = replayFixture(t)
  const failed = assert.rejects(
    pending,
    /did not complete replay to checkpoint within 60000 ms/,
  )
  await flushed()
  batch(calls[0], "prefix", ["first"])
  await advanced(t, 59_999)
  assert.equal(calls[0].closed, 0)
  t.mock.timers.tick(1)
  await failed
  assert.equal(calls[0].closed, 1)
})

test("empty history still requires matching fresh-head authority", async (t) => {
  const { calls, pending } = replayFixture(t)
  await flushed()
  batch(calls[0], "head")
  await advanced(t)
  assert.deepEqual(await pending, [])
  assert.equal(calls.length, 2)
})

test("capture and growing-head resumes share the original deadline", async (t) => {
  const { calls, pending } = replayFixture(t, { capture: false })
  const failed = assert.rejects(
    pending,
    /did not complete replay to checkpoint within 60000 ms/,
  )
  await flushed()
  await advanced(t, 59_000)
  batch(calls[0], "old-head", ["one"])
  batch(calls[0], "old-head")
  await advanced(t)
  assert.equal(calls.length, 2)
  batch(calls[1], "new-head")
  await advanced(t)
  assert.equal(calls.length, 3)
  assert.equal(calls[2].options.after, "old-head")
  t.mock.timers.tick(800)
  await failed
  assert.equal(calls[2].closed, 1)
})

test("a missing head capture fails rather than accepting an empty candidate", async (t) => {
  const { calls, pending } = replayFixture(t, { capture: false })
  const failed = assert.rejects(pending, /did not complete head capture within 60000 ms/)
  await flushed()
  batch(calls[0], "candidate")
  await advanced(t)
  t.mock.timers.tick(59_900)
  await failed
  assert.equal(calls[1].closed, 1)
})

test("a late opening after deadline releases its source without starting a capture", async (t) => {
  const { calls, pending, release } = replayFixture(t, { delayedOpen: true })
  const failed = assert.rejects(
    pending,
    /did not complete replay to checkpoint within 60000 ms/,
  )
  t.mock.timers.tick(60_000)
  await failed
  release()
  await flushed()
  assert.equal(calls[0].closed, 1)
  assert.equal(calls.length, 1)
})

for (const target of ["replay", "head capture"]) {
  test(`a terminal ${target} refusal stays a failure`, async (t) => {
    const { calls, pending } = replayFixture(t, { capture: false })
    const failed = assert.rejects(pending, /ended refused/)
    await flushed()
    if (target === "head capture") {
      batch(calls[0], "candidate")
      await advanced(t)
    }
    calls.at(-1).handlers.ended({ reason: "refused", code: "audit_unavailable" })
    t.mock.timers.tick(100)
    await failed
    assert.ok(calls.every((call) => call.closed === 1))
  })
}

for (const [stalledClose, phase] of [
  [0, "replay cleanup"],
  [1, "head capture cleanup"],
]) {
  test(`a stalled ${phase} fails without misreporting convergence`, async (t) => {
    const { calls, pending } = replayFixture(t, { stalledClose })
    const failed = assert.rejects(
      pending,
      new RegExp(`did not complete ${phase} within 60000 ms`),
    )
    await flushed()
    batch(calls[0], "head")
    await advanced(t)
    t.mock.timers.tick(59_900)
    await failed
    assert.equal(calls[stalledClose].closed, 1)
    assert.equal(calls[stalledClose].options.signal.aborted, true)
    if (stalledClose === 0) assert.equal(calls.length, 1)
  })
}
function probing(open) {
  const followed = follower().subject
  followed.limits = { conversationTargets: 8 }
  followed.list.conversations = Array.from({ length: 8 }, (_, n) => `other-${n}`)
  followed.view.subscription = {}
  followed.client = {
    connectionState: { status: "connected" },
    subscriptions: { view: open },
  }
  return followed
}

const refused = (code) => Object.assign(new Error(code), { code })

test("a probe interrupted by agent retirement closes its views and the next probe still proves capacity", async () => {
  let opens = 0
  let closes = 0
  let retiring = true
  const followed = probing(async () => {
    opens += 1
    if (retiring && opens === 2) throw refused("conversation_closed")
    if (!retiring && opens === 8) throw refused("subscription_capacity")
    return {
      close: async () => {
        closes += 1
      },
    }
  })
  await followed.probe()
  assert.equal(opens, 2)
  assert.equal(closes, 1)
  assert.deepEqual(followed.failures, [])
  assert.equal(followed.counts.refusedAsExpected, 0)
  retiring = false
  opens = 0
  closes = 0
  await followed.probe()
  assert.equal(opens, 8)
  assert.equal(closes, 7)
  assert.deepEqual(followed.failures, [])
  assert.equal(followed.counts.refusedAsExpected, 1)
})

for (const code of [
  "permission_denied",
  "subscription_duplicate",
  "subscription_capacity",
]) {
  test(`a probe still fails for ${code} before its capacity boundary`, async () => {
    let opens = 0
    let closes = 0
    const followed = probing(async () => {
      opens += 1
      if (opens === 2) throw refused(code)
      return {
        close: async () => {
          closes += 1
        },
      }
    })
    await followed.probe()
    assert.equal(closes, 1)
    assert.equal(followed.failures.length, 1)
    assert.match(followed.failures[0], new RegExp(code))
    assert.equal(followed.counts.refusedAsExpected, 0)
  })
}

test("a probe still fails if a subscription past capacity is accepted", async () => {
  let closes = 0
  const followed = probing(async () => ({
    close: async () => {
      closes += 1
    },
  }))
  await followed.probe()
  assert.equal(closes, 8)
  assert.equal(followed.failures.length, 1)
  assert.match(followed.failures[0], /past 8 was accepted/)
  assert.equal(followed.counts.refusedAsExpected, 0)
})
