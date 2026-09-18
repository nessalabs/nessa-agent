import { afterEach, expect, it, vi } from "vitest"

import { revealOnFirstRender } from "./reveal-on-first-render"

afterEach(() => vi.unstubAllGlobals())

/**
 * The window this runs in is hidden, and a hidden macOS window is never drawn —
 * so its webview is served no animation frames, and anything queued for one is
 * queued for good. This is that window: `requestAnimationFrame` takes the
 * callback and never calls it, the way the real one does not.
 */
function aWindowThatIsNeverDrawn() {
  const queued: FrameRequestCallback[] = []
  vi.stubGlobal("requestAnimationFrame", (frame: FrameRequestCallback) =>
    queued.push(frame),
  )
  return queued
}

it("reveals a window that is never served an animation frame", () => {
  const queued = aWindowThatIsNeverDrawn()
  const reveal = vi.fn()

  revealOnFirstRender(reveal)

  // The defect: waiting one frame past the first render meant waiting for a
  // paint that the reveal itself was the precondition for, so setup played its
  // opening sound behind a window that was never shown.
  expect(reveal).toHaveBeenCalledTimes(1)
  expect(queued).toHaveLength(0)
})

it("asks for the reveal once per render it is given", () => {
  aWindowThatIsNeverDrawn()
  const reveal = vi.fn()

  // Strict mode runs the mounting effect twice. Showing an already-shown window
  // is the end state either way, so this does not deduplicate — it must simply
  // not swallow the second one and leave the first cancelled.
  expect(revealOnFirstRender(reveal)).toBeUndefined()
  expect(revealOnFirstRender(reveal)).toBeUndefined()

  expect(reveal).toHaveBeenCalledTimes(2)
})
