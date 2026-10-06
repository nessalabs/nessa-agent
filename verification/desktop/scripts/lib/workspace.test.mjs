import assert from "node:assert/strict"
import { it } from "node:test"
import { paneDropFailure } from "./workspace.mjs"

it("accepts a moved pane after the carried copy is gone", () => {
  assert.equal(paneDropFailure({ order: "1,2" }, { order: "2,1", ghost: 0 }), null)
})
it("rejects a no-op even when no carried copy remains", () => {
  assert.equal(
    paneDropFailure({ order: "1,2" }, { order: "1,2", ghost: 0 }),
    "the drop did not change pane order",
  )
})
it("rejects a moved pane while its carried copy remains", () => {
  assert.equal(
    paneDropFailure({ order: "1,2" }, { order: "2,1", ghost: 1 }),
    "the carried copy is still on the page after the drop",
  )
})
