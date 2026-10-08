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
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import { leaveInPlace, useReflow } from "./overview-reflow"

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

describe("an item leaving the list", () => {
  const listWith = (slow: string) => {
    const list = document.createElement("div")
    list.style.setProperty("--desktop-slow", slow)
    const item = document.createElement("li")
    item.dataset.reflow = "request:a"
    item.innerHTML = '<div data-overview-item="a" tabindex="0">A</div>'
    list.append(item)
    host.append(list)
    return { list, item }
  }

  it("leaves a copy where it stood, out of the re-flow, the keyboard and screen readers", () => {
    const { list, item } = listWith("240ms")
    item.animate = (() => ({}) as Animation) as typeof item.animate
    leaveInPlace(list, item)
    const ghost = list.querySelector<HTMLElement>("[data-departing]")
    expect(ghost).not.toBeNull()
    expect(ghost?.getAttribute("aria-hidden")).toBe("true")
    expect(ghost?.inert).toBe(true)
    expect(ghost?.style.position).toBe("absolute")
    expect(list.querySelectorAll("[data-departing] [data-overview-item]").length).toBe(0)
    expect(list.querySelectorAll("[data-departing][data-reflow]").length).toBe(0)
    expect(list.querySelectorAll("[data-departing] [tabindex]").length).toBe(0)
  })

  it("leaves nothing behind with less motion, where the token is zero", () => {
    const { list, item } = listWith("0ms")
    item.animate = (() => ({}) as Animation) as typeof item.animate
    leaveInPlace(list, item)
    expect(list.querySelector("[data-departing]")).toBeNull()
  })
})
