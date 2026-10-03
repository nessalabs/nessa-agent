/**
 * `apps.mjs`'s count of one call's inline mounts: `renders` continues only on
 * exactly one, so a call drawn twice (#418) fails rather than being skipped.
 */
import assert from "node:assert/strict"
import { describe, it } from "node:test"

import { oneCard, oneMount } from "./apps.mjs"

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

describe("oneCard", () => {
  it("accepts exactly one step", () => {
    assert.equal(oneCard(1), null)
  })
  it("fails a call with no step", () => {
    assert.match(oneCard(0), /no step in the transcript/)
  })
  it("fails a call drawn as more than one step, naming the count", () => {
    assert.match(oneCard(3), /3 transcript steps, not one \(#418\)/)
  })
})
