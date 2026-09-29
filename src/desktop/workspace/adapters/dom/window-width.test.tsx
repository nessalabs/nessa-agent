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
