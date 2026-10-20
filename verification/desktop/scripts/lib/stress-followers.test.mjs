import { strict as assert } from "node:assert"
import { test } from "node:test"
import { Follower } from "./stress-followers.mjs"

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
