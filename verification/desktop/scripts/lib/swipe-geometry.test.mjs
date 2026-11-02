import assert from "node:assert/strict"
import { test } from "node:test"
import { runInNewContext } from "node:vm"
import { settledOffset, swipeGeometrySettled } from "./swipe-geometry.mjs"

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

for (const [published, expected] of [
  ["160px", 160],
  ["", 400],
]) {
  test(`settled swipe reads published width ${JSON.stringify(published)} before the full-row tray fallback`, async () => {
    const action = {}
    const tray = {
      getBoundingClientRect: () => ({ width: 400 }),
      querySelector(selector) {
        assert.equal(selector, '[data-slot="workspace-swipe-row-action"]')
        return action
      },
    }
    const element = {
      firstElementChild: { getBoundingClientRect: () => ({ left: 152 }) },
      getBoundingClientRect: () => ({ left: 300 }),
      getAnimations: () => [],
      querySelector(selector) {
        assert.equal(selector, '[data-slot="workspace-swipe-row-actions"]')
        return tray
      },
    }
    let samples = 0
    const wrapper = {
      async evaluate(read, from) {
        samples++
        return runInNewContext(`(${read.toString()})(element, from)`, {
          element,
          from,
          getComputedStyle(target) {
            assert.equal(target, action)
            return {
              getPropertyValue(name) {
                assert.equal(name, "--nessa-swipe-open")
                return published
              },
            }
          },
        })
      },
    }
    const result = await settledOffset(wrapper, 12)
    assert.equal(result.offset, -160, "the closed baseline remains part of the offset")
    assert.equal(
      result.tray,
      expected,
      "the action publishes resting open width; its container spans the row",
    )
    assert.ok(samples >= 2)
  })
}
