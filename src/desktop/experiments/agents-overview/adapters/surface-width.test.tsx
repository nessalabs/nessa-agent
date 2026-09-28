// @vitest-environment jsdom
/**
 * Whether the overview is wide enough for its peek beside the list: first
 * from what its place measured before it appeared, then from the observer —
 * and only a change of answer draws it again.
 */
import { act, useRef } from "react"
import { createRoot } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { useAtLeastWide } from "./surface-width"

let host: HTMLDivElement
let report: (width: number) => void = () => {}

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  class Observer {
    constructor(callback: ResizeObserverCallback) {
      report = (width) =>
        callback(
          [{ contentRect: { width } } as ResizeObserverEntry],
          this as unknown as ResizeObserver,
        )
    }
    observe() {}
    disconnect() {}
  }
  Object.assign(globalThis, { ResizeObserver: Observer })
  host = document.createElement("div")
  document.body.append(host)
})

afterEach(() => host.remove())

let renders = 0
function Surface({ hint }: { hint: number }) {
  const element = useRef<HTMLDivElement>(null)
  const wide = useAtLeastWide(element, 820, hint)
  renders++
  return <div ref={element} data-wide={wide || undefined} />
}

it("starts from its place's width, and draws again only when the answer changes", async () => {
  const root = createRoot(host)
  renders = 0
  await act(async () => root.render(<Surface hint={1200} />))
  expect(host.querySelector("[data-wide]")).not.toBeNull()
  const drawn = renders
  await act(async () => report(1300))
  expect(renders).toBe(drawn)
  await act(async () => report(700))
  expect(host.querySelector("[data-wide]")).toBeNull()
  expect(renders).toBe(drawn + 1)
  await act(async () => root.unmount())
})
