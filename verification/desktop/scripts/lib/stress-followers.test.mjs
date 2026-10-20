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
