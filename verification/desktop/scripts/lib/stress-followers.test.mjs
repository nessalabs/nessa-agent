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
