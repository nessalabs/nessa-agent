import assert from "node:assert/strict"
import { test } from "node:test"
import { swipeGeometrySettled } from "./swipe-geometry.mjs"

test("repeated pixels cannot settle an unfinished geometry animation", () => {
  const previous = { offset: -168.5625, tray: 162.484375 }
  assert.equal(
    swipeGeometrySettled(previous, { ...previous, geometryActive: true }),
    false,
  )
  assert.equal(
    swipeGeometrySettled(previous, { ...previous, geometryActive: false }),
    true,
  )
  assert.equal(swipeGeometrySettled(null, previous), false)
  assert.equal(swipeGeometrySettled(previous, { ...previous, tray: 168.5625 }), false)
  assert.equal(
    swipeGeometrySettled({ offset: 0, tray: 0 }, { offset: 0, tray: 0 }),
    false,
  )
})
