// @vitest-environment jsdom
/**
 * The list's re-flow reads nothing of the page as the list first appears:
 * its first places are taken by the observer, once the page has laid out on
 * its own, so opening the overview lays the page out once. Nor does a render
 * that changes nothing the list holds — a row chosen, an answer on its way —
 * so answering never makes the page lay out early; a change of what it holds
 * is read once.
 */
import { act, useRef } from "react"
import { createRoot } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { useReflow } from "./overview-reflow"

let host: HTMLDivElement
let reads = 0
let offsetTop: PropertyDescriptor | undefined

let observed: (() => void) | null = null

beforeEach(() => {
  observed = null
  class Observer {
    constructor(callback: () => void) {
      observed = callback
    }
    observe() {}
    disconnect() {}
  }
  Object.assign(globalThis, { ResizeObserver: Observer })
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  reads = 0
  offsetTop = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "offsetTop")
  Object.defineProperty(HTMLElement.prototype, "offsetTop", {
    configurable: true,
    get: () => {
      reads++
      return 0
    },
  })
  host = document.createElement("div")
  document.body.append(host)
})

afterEach(() => {
  if (offsetTop) Object.defineProperty(HTMLElement.prototype, "offsetTop", offsetTop)
  host.remove()
})

function List({
  keys = ["a", "b", "c"],
  chosen = "",
}: {
  keys?: string[]
  chosen?: string
}) {
  const list = useRef<HTMLDivElement>(null)
  useReflow(list, keys.join(","))
  return (
    <div ref={list} style={{ position: "relative" }} data-chosen={chosen}>
      {keys.map((key) => (
        <p key={key} data-reflow={key}>
          {key}
        </p>
      ))}
    </div>
  )
}

it("reads no place of the list as it first appears", async () => {
  const root = createRoot(host)
  await act(async () => root.render(<List />))
  expect(reads).toBe(0)
  await act(async () => root.unmount())
})

it("reads nothing for a render that changes nothing it holds, and reads once when it does", async () => {
  const root = createRoot(host)
  await act(async () => root.render(<List />))
  // The page laid out on its own: the observer takes the first places.
  await act(async () => observed?.())
  reads = 0
  await act(async () => root.render(<List chosen="b" />))
  await act(async () => root.render(<List chosen="c" />))
  expect(reads).toBe(0)
  await act(async () => root.render(<List keys={["a", "c"]} chosen="c" />))
  expect(reads).toBeGreaterThan(0)
  await act(async () => root.unmount())
})
