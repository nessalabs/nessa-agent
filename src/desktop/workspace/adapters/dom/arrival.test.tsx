// @vitest-environment jsdom
import { act, useRef } from "react"
import { createRoot } from "react-dom/client"
import { expect, it, vi } from "vitest"
import { useArrival } from "./arrival"

it("reads the arrival's landing layout before animation writes and cancels on removal", async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  let written = false
  const cancel = vi.fn()
  const geometry = vi
    .spyOn(Element.prototype, "getBoundingClientRect")
    .mockImplementation(() => {
      expect(written, "a layout read after animation styles were written").toBe(false)
      return new DOMRect(20, 30, 200, 100)
    })
  const computed = vi.spyOn(globalThis, "getComputedStyle").mockImplementation(() => {
    expect(written, "a style read after animation styles were written").toBe(false)
    return { getPropertyValue: () => "200ms" } as unknown as CSSStyleDeclaration
  })
  const previousAnimate = Object.getOwnPropertyDescriptor(Element.prototype, "animate")
  const animate = vi.fn(() => {
    written = true
    return { cancel } as unknown as Animation
  })
  Object.defineProperty(Element.prototype, "animate", {
    configurable: true,
    value: animate,
  })
  function Arrival() {
    const dock = useRef<HTMLDivElement>(null)
    const scroller = useRef<HTMLDivElement>(null)
    const heading = useRef<HTMLDivElement>(null)
    useArrival(
      { composer: new DOMRect(0, 0, 400, 100), text: new DOMRect(5, 5, 300, 50) },
      { dock, scroller, heading },
    )
    return (
      <>
        <div ref={dock}>
          <div className="desktop-composer">
            <textarea />
          </div>
        </div>
        <div ref={scroller}>
          <div data-role="user">
            <div className="workspace-bubble" />
          </div>
        </div>
        <div ref={heading} />
      </>
    )
  }
  try {
    await act(async () => root.render(<Arrival />))
    expect(geometry).toHaveBeenCalledTimes(2)
    expect(animate).toHaveBeenCalledTimes(4)
    await act(async () => root.unmount())
    expect(cancel).toHaveBeenCalledTimes(4)
  } finally {
    geometry.mockRestore()
    computed.mockRestore()
    if (previousAnimate)
      Object.defineProperty(Element.prototype, "animate", previousAnimate)
    else Reflect.deleteProperty(Element.prototype, "animate")
    host.remove()
  }
})
