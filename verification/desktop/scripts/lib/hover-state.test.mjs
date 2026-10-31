import assert from "node:assert/strict"
import { test } from "node:test"
import { hoverFailures } from "./hover-state.mjs"

const hover = "oklab(0.985 0 0 / 0.05)"
const held = {
  fill: hover,
  connected: true,
  hovered: true,
  pointerWithinRow: true,
  finiteActive: 0,
}
test("only the exact settled ink under the same pointer can pass", () => {
  assert.deepEqual(hoverFailures(held, [hover]), [])
  for (const change of [
    { fill: "oklab(0.985 0 0 / 0.0499794)" },
    { fill: "oklab(0.985 0 0 / 0.03)" },
    { connected: false },
    { hovered: false },
    { pointerWithinRow: false },
    { finiteActive: 1 },
  ])
    assert.notEqual(hoverFailures({ ...held, ...change }, [hover]).length, 0)
})
