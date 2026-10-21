import assert from "node:assert/strict"
import { test } from "node:test"
import { Follower } from "./stress-followers.mjs"

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
      /subscription 3 of 3 failed conversation_deleted$/,
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
