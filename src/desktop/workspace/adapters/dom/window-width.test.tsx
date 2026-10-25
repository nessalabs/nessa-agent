// @vitest-environment jsdom
/**
 * The side columns are fitted as a change is laid out, before the frame
 * paints — never in a later effect, which a resize observer's synchronous
 * render (`flushSync`) would flush out of turn, resizing the window's columns
 * inside the observer and leaving its notifications undelivered.
 */
import { act, useEffect } from "react"
import { createRoot } from "react-dom/client"
import { expect, it } from "vitest"
import { useFitOnResize } from "./window-width"

it("fits in the commit that sees the width, before any effect of that commit runs", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const order: string[] = []
  // A part of the window with an effect of its own: effects run parts first.
  function Part({ width }: { width: number }) {
    useEffect(() => {
      order.push(`effect ${width}`)
    }, [width])
    return null
  }
  function Window({ width }: { width: number }) {
    useFitOnResize(width, 1, (at) => order.push(`fit ${at}`))
    return <Part width={width} />
  }
  const host = document.createElement("div")
  const root = createRoot(host)
  await act(async () => root.render(<Window width={1200} />))
  await act(async () => root.render(<Window width={900} />))
  expect(order).toEqual(["fit 1200", "effect 1200", "fit 900", "effect 900"])
  await act(async () => root.unmount())
})

it("fits a new pane column from the next animation frame", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const order: string[] = []
  const queued: FrameRequestCallback[] = []
  const request = window.requestAnimationFrame
  const cancel = window.cancelAnimationFrame
  window.requestAnimationFrame = (callback) => {
    queued.push(callback)
    return queued.length
  }
  window.cancelAnimationFrame = () => {}
  function Window({ columns }: { columns: number }) {
    useFitOnResize(1000, columns, () => order.push(`fit ${columns}`))
    return null
  }
  const host = document.createElement("div")
  const root = createRoot(host)
  try {
    await act(async () => root.render(<Window columns={1} />))
    order.length = 0
    queued.length = 0
    await act(async () => root.render(<Window columns={2} />))
    // The commit counted the column and did not fold. The frame it queued does.
    expect(order).toEqual([])
    expect(queued).toHaveLength(1)
    queued[0](0)
    expect(order).toEqual(["fit 2"])
    await act(async () => root.unmount())
  } finally {
    window.requestAnimationFrame = request
    window.cancelAnimationFrame = cancel
  }
})

it("fits nothing while covered, and uncovered fits only a size that changed", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const fitted: number[] = []
  function Window({ width, active }: { width: number; active: boolean }) {
    useFitOnResize(width, 1, (at) => fitted.push(at), active)
    return null
  }
  const root = createRoot(document.createElement("div"))
  await act(async () => root.render(<Window width={760} active />))
  // A full view takes the window: its width changes under it, and nothing is fitted.
  await act(async () => root.render(<Window width={712} active={false} />))
  // Uncovered at the width it was fitted at: nothing to fit.
  await act(async () => root.render(<Window width={760} active />))
  expect(fitted).toEqual([760])
  await act(async () => root.render(<Window width={900} active={false} />))
  expect(fitted).toEqual([760])
  await act(async () => root.render(<Window width={900} active />))
  expect(fitted).toEqual([760, 900])
  await act(async () => root.unmount())
})
