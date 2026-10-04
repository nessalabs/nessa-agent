/**
 * `apps.mjs`'s count of one call's inline mounts: `renders` continues only on
 * exactly one, so a call drawn twice (#418) fails rather than being skipped.
 * And `release`'s wait for a withdrawn review's card to go (rows W1–W4, #436).
 */
import assert from "node:assert/strict"
import { describe, it } from "node:test"

import { approvalGone, oneMount } from "./apps.mjs"

describe("oneMount", () => {
  it("accepts exactly one inline frame", () => {
    assert.equal(oneMount(1), null)
  })
  it("fails a call with no frame", () => {
    assert.match(oneMount(0), /no inline app frame/)
  })
  it("fails a call drawn more than once, naming the count", () => {
    assert.match(oneMount(2), /2 inline app frames, not one \(#418\)/)
    assert.match(oneMount(4), /4 inline app frames/)
  })
})

/**
 * A page whose one approval card goes `goesAfter` ms from the wait's start
 * (`0`: already gone, `Infinity`: never), the way Playwright's
 * `waitFor({ state: "hidden" })` settles: when it goes, or a `TimeoutError`
 * at the bound. `fails` makes the wait throw another error.
 */
function cardPage({ goesAfter, fails } = {}) {
  const asked = []
  const locator = {
    first: () => locator,
    waitFor: ({ state, timeout }) => {
      asked.push({ state, timeout })
      if (fails) return Promise.reject(new Error("Target page has been closed"))
      if (state !== "hidden") return Promise.resolve()
      if (goesAfter > timeout) {
        const error = new Error(`Timeout ${timeout}ms exceeded`)
        error.name = "TimeoutError"
        return new Promise((_, reject) => setTimeout(() => reject(error), timeout))
      }
      return new Promise((resolve) => setTimeout(resolve, goesAfter))
    },
  }
  return { asked, page: { locator: () => locator } }
}

describe("approvalGone", () => {
  it("W1: a card already gone has gone", async () => {
    assert.equal(await approvalGone(cardPage({ goesAfter: 0 }).page, "t", 50), true)
  })
  it("W2: a card still shown at the start is waited out, not sampled", async () => {
    const { asked, page } = cardPage({ goesAfter: 20 })
    assert.equal(await approvalGone(page, "t", 200), true)
    assert.deepEqual(asked, [{ state: "hidden", timeout: 200 }])
  })
  it("W3: a card still shown at the bound has not gone", async () => {
    assert.equal(
      await approvalGone(cardPage({ goesAfter: Infinity }).page, "t", 30),
      false,
    )
  })
  it("W4: an error other than the timeout is not the card's state", async () => {
    await assert.rejects(approvalGone(cardPage({ fails: true }).page, "t", 30), /closed/)
  })
})
