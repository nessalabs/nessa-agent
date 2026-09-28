// @vitest-environment jsdom
/**
 * The list's re-flow reads nothing of the page as the list first appears:
 * its first places are taken by the observer, once the page has laid out on
 * its own, so opening the overview lays the page out once.
 */
import { act, useRef } from "react"
import { createRoot } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { useReflow } from "./reflow"

let host: HTMLDivElement
let reads = 0
let offsetTop: PropertyDescriptor | undefined

beforeEach(() => {
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

function List() {
  const list = useRef<HTMLDivElement>(null)
  useReflow(list)
  return (
    <div ref={list} style={{ position: "relative" }}>
      {["a", "b", "c"].map((key) => (
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
