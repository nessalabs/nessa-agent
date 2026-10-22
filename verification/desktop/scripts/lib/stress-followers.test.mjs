import { strict as assert } from "node:assert"
import { test } from "node:test"

import { Follower } from "./stress-followers.mjs"

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

function probing(open) {
  const followed = follower()
  followed.list.conversations = Array.from({ length: 8 }, (_, n) => `other-${n}`)
  followed.client = { subscriptions: { view: open } }
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
