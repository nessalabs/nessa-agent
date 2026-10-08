import assert from "node:assert/strict"
import { afterEach, it } from "node:test"
import { recordShapeFrames, stopShapeFrames } from "./shape-sampler.mjs"

const sel = {
  dragGhost: ".ghost",
  dragPlaceholder: ".placeholder",
  titleText: ".title",
  dragTitle: ".title",
  dropAnnouncer: ".status",
  pane: ".pane",
  transcript: ".transcript",
  dock: ".dock",
  lifted: ".lifted",
}

/** A page with nothing to measure: the tick records a frame and schedules the next. */
function install() {
  const queued = []
  const previous = {
    window: globalThis.window,
    document: globalThis.document,
    requestAnimationFrame: globalThis.requestAnimationFrame,
    addEventListener: globalThis.addEventListener,
  }
  globalThis.window = {}
  globalThis.document = {
    querySelector() {
      return null
    },
    querySelectorAll() {
      return []
    },
    timeline: { currentTime: 0 },
  }
  globalThis.requestAnimationFrame = (fn) => {
    queued.push(fn)
    return queued.length
  }
  globalThis.addEventListener = () => {}
  return {
    queued,
    /** Runs every tick already asked for, and no tick that ask schedules. */
    flush() {
      const batch = queued.splice(0, queued.length)
      for (const fn of batch) fn(16)
      return batch.length
    },
    restore() {
      globalThis.window = previous.window
      globalThis.document = previous.document
      globalThis.requestAnimationFrame = previous.requestAnimationFrame
      globalThis.addEventListener = previous.addEventListener
    },
  }
}

let page = null
afterEach(() => page?.restore())

it("records one frame per flush when a recorder is stopped and started again inside that frame", () => {
  page = install()
  recordShapeFrames(sel)
  assert.equal(page.flush(), 1)
  assert.equal(window.__shapes.length, 1)
  stopShapeFrames()
  recordShapeFrames(sel)
  // The retired tick is still queued beside the new one. It records nothing.
  assert.equal(page.flush(), 2)
  assert.equal(window.__shapes.length, 1)
  assert.equal(page.queued.length, 1)
  assert.equal(page.flush(), 1)
  assert.equal(window.__shapes.length, 2)
  assert.equal(page.queued.length, 1)
})
