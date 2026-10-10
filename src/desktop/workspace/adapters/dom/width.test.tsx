// @vitest-environment jsdom
/**
 * Whether the overview's layer is wide enough for its peek beside the list:
 * from the observer, and only a change of answer draws it again.
 */
import { act, useRef } from "react"
import { createRoot } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { useAtLeastWide } from "./width"

const frames = new Map<number, FrameRequestCallback>()
let frameId = 0
const paint = () => {
  const pending = [...frames.values()]
  frames.clear()
  for (const callback of pending) callback(0)
}
let host: HTMLDivElement
let report: (width: number) => void = () => {}

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    const id = ++frameId
    frames.set(id, callback)
    return id
  })
  vi.stubGlobal("cancelAnimationFrame", (id: number) => frames.delete(id))
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

afterEach(() => {
  host.remove()
  frames.clear()
  vi.unstubAllGlobals()
})

let renders = 0
function Surface() {
  const element = useRef<HTMLDivElement>(null)
  const wide = useAtLeastWide(element, 820)
  renders++
  return <div ref={element} data-wide={wide || undefined} />
}

it("follows the observer, and draws again only when the answer changes", async () => {
  const root = createRoot(host)
  renders = 0
  await act(async () => root.render(<Surface />))
  expect(host.querySelector("[data-wide]")).toBeNull()
  await act(async () => report(1200))
  expect(host.querySelector("[data-wide]")).toBeNull()
  await act(paint)
  expect(host.querySelector("[data-wide]")).not.toBeNull()
  const drawn = renders
  await act(async () => report(1300))
  await act(paint)
  expect(renders).toBe(drawn)
  await act(async () => report(700))
  await act(paint)
  expect(host.querySelector("[data-wide]")).toBeNull()
  expect(renders).toBe(drawn + 1)
  await act(async () => report(1200))
  await act(async () => report(700))
  await act(paint)
  expect(renders).toBe(drawn + 1)
  await act(async () => report(1200))
  expect(frames.size).toBe(1)
  await act(async () => root.unmount())
  expect(frames.size).toBe(0)
  await act(paint)
})
