// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { stagedReveal } from "./staged-reveal"
import { marks } from "./marks"

let scope: HTMLDivElement
let queued: Map<number, FrameRequestCallback>
let next: number
beforeEach(() => {
  scope = document.createElement("div")
  scope.innerHTML =
    '<article data-pane-key="a"></article><article data-pane-key="b"></article><article data-pane-key="c"></article>'
  document.body.append(scope)
  queued = new Map()
  next = 0
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    const id = ++next
    queued.set(id, callback)
    return id
  })
  vi.stubGlobal("cancelAnimationFrame", (id: number) => queued.delete(id))
})
afterEach(() => {
  scope.remove()
  vi.unstubAllGlobals()
})
const waiting = () => scope.querySelectorAll(`[${marks.restoring}]`).length
const frame = () => {
  const pending = [...queued.values()]
  queued.clear()
  pending.forEach((run) => run(0))
}

it("restores one body per frame and schedules only one pending frame", () => {
  const reveal = stagedReveal(scope, marks.restoring)
  reveal.hold()
  reveal.start()
  reveal.start()
  expect(queued.size).toBe(1)
  expect(waiting()).toBe(3)
  frame()
  expect(waiting()).toBe(2)
  frame()
  expect(waiting()).toBe(1)
  frame()
  expect(waiting()).toBe(0)
  expect(queued.size).toBe(0)
  reveal.dispose()
})

it("a new hold rejects the old callback without revealing its new layout", () => {
  const reveal = stagedReveal(scope, marks.restoring)
  reveal.hold()
  reveal.start()
  const stale = [...queued.values()][0]
  frame()
  expect(waiting()).toBe(2)
  reveal.hold()
  reveal.start()
  stale(0)
  expect(waiting()).toBe(3)
  expect(queued.size).toBe(1)
  frame()
  expect(waiting()).toBe(2)
  reveal.dispose()
})

it("disposal clears its bodies, rejects late callbacks and leaves a new owner alone", () => {
  const old = stagedReveal(scope, marks.restoring)
  old.hold()
  old.start()
  const stale = [...queued.values()][0]
  old.dispose()
  expect(waiting()).toBe(0)
  expect(queued.size).toBe(0)
  const current = stagedReveal(scope, marks.restoring)
  current.hold()
  old.dispose()
  old.hold()
  old.start()
  stale(0)
  expect(waiting()).toBe(3)
  expect(queued.size).toBe(0)
  current.start()
  frame()
  expect(waiting()).toBe(2)
  current.dispose()
})

it("stopping retains its waiting bodies and does not clear another adapter's mark", () => {
  const reveal = stagedReveal(scope, marks.restoring)
  const drag = stagedReveal(scope, marks.settling)
  reveal.hold()
  drag.hold()
  reveal.start()
  reveal.stop()
  expect(waiting()).toBe(3)
  reveal.dispose()
  expect(scope.querySelectorAll(`[${marks.settling}]`)).toHaveLength(3)
  drag.dispose()
})
