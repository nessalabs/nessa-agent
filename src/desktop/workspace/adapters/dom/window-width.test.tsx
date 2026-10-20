// @vitest-environment jsdom
/**
 * The side columns are fitted as a change is laid out, before the frame
 * paints — never in a later effect, which a resize observer's synchronous
 * render (`flushSync`) would flush out of turn, resizing the window's columns
 * inside the observer and leaving its notifications undelivered.
 */
import { act, StrictMode, useEffect } from "react"
import { createRoot } from "react-dom/client"
import { expect, it } from "vitest"
import { useDrawnAfterColumnChange, useFitOnResize } from "./window-width"

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

it("paints a sidebar fold from the next frame when a pane column appears, and before paint when the person toggles it", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const queued = new Map<number, FrameRequestCallback>()
  let next = 1
  const request = window.requestAnimationFrame
  const cancel = window.cancelAnimationFrame
  window.requestAnimationFrame = (callback) => {
    const id = next++
    queued.set(id, callback)
    return id
  }
  window.cancelAnimationFrame = (id) => {
    queued.delete(id)
  }
  function Window({ open, columns }: { open: boolean; columns: number }) {
    const drawn = useDrawnAfterColumnChange(open, columns)
    return <div data-drawn={drawn ? "open" : "closed"} />
  }
  const host = document.createElement("div")
  const root = createRoot(host)
  const drawn = () => host.querySelector("div")?.getAttribute("data-drawn")
  const run = () => {
    const callback = queued.values().next().value
    queued.clear()
    callback?.(0)
  }
  try {
    await act(async () =>
      root.render(
        <StrictMode>
          <Window open columns={1} />
        </StrictMode>,
      ),
    )
    expect(drawn()).toBe("open")
    queued.clear()
    await act(async () =>
      root.render(
        <StrictMode>
          <Window open={false} columns={1} />
        </StrictMode>,
      ),
    )
    expect(drawn()).toBe("closed")
    expect(queued.size).toBe(0)
    await act(async () =>
      root.render(
        <StrictMode>
          <Window open columns={1} />
        </StrictMode>,
      ),
    )
    queued.clear()
    await act(async () =>
      root.render(
        <StrictMode>
          <Window open={false} columns={2} />
        </StrictMode>,
      ),
    )
    expect(drawn()).toBe("open")
    expect(queued.size).toBe(1)
    await act(async () => run())
    expect(drawn()).toBe("closed")
    await act(async () => root.unmount())
  } finally {
    window.requestAnimationFrame = request
    window.cancelAnimationFrame = cancel
  }
})
