/**
 * `apps.mjs`'s count of one call's inline mounts: `renders` continues only on
 * exactly one, so a call drawn twice (#418) fails rather than being skipped.
 */
import assert from "node:assert/strict"
import { describe, it } from "node:test"

import { oneMount } from "./apps.mjs"

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
