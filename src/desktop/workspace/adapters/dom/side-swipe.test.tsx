// @vitest-environment jsdom
/**
 * The swipe is read only over the chat, and a gesture that begins over
 * something scrolling sideways is left to it (`side-swipe.ts`).
 */
import { act, useRef, type ReactNode } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import type { SwipeDirection } from "../../model/side-swipe"
import { useSideSwipe } from "./side-swipe"

let root: Root
let host: HTMLDivElement
let swiped: SwipeDirection[]

function Window({
  enabled = true,
  children,
}: {
  enabled?: boolean
  children: ReactNode
}) {
  const ref = useRef<HTMLDivElement>(null)
  useSideSwipe(ref, enabled, (direction) => swiped.push(direction))
  return <div ref={ref}>{children}</div>
}

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  swiped = []
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(() => {
  act(() => root.unmount())
  host.remove()
})

function render(enabled = true) {
  act(() =>
    root.render(
      <Window enabled={enabled}>
        <aside id="sidebar">sidebar</aside>
        <main className="workspace-chat">
          <p id="words">words</p>
          <input id="field" />
          <div inert>
            <p id="covered">covered</p>
          </div>
          <div id="table" style={{ overflowX: "auto" }}>
            <span id="cell">cell</span>
          </div>
        </main>
      </Window>,
    ),
  )
  const table = host.querySelector<HTMLElement>("#table")
  if (!table) throw new Error("no table")
  Object.defineProperty(table, "scrollWidth", { value: 900 })
  Object.defineProperty(table, "clientWidth", { value: 300 })
  const field = host.querySelector<HTMLElement>("#field")
  if (!field) throw new Error("no field")
  Object.defineProperty(field, "scrollWidth", { value: 400 })
  Object.defineProperty(field, "clientWidth", { value: 200 })
}

/** One gesture: `count` wheel events at once, each `deltaX` wide. */
function wheel(
  id: string,
  deltaX: number,
  count = 8,
  init: WheelEventInit = {},
  extra: Record<string, unknown> = {},
) {
  const target = host.querySelector(`#${id}`)
  if (!target) throw new Error(`no #${id}`)
  for (let i = 0; i < count; i += 1)
    target.dispatchEvent(
      Object.assign(new WheelEvent("wheel", { deltaX, bubbles: true, ...init }), extra),
    )
}

it("reads one swipe per gesture over the chat", () => {
  render()
  wheel("words", 15)
  expect(swiped).toEqual(["left"])
})

it("leaves a gesture alone outside the chat", () => {
  render()
  wheel("sidebar", 15)
  expect(swiped).toEqual([])
})

it("leaves a gesture to what scrolls sideways under it", () => {
  render()
  wheel("cell", -15)
  expect(swiped).toEqual([])
})

it("leaves a gesture to what scrolls sideways inside a shadow root", () => {
  render()
  // A highlighted code block draws its lines in a shadow root: the wheel's
  // target outside it is only the root's host, which never scrolls.
  const block = document.createElement("div")
  host.querySelector("#words")?.after(block)
  const scroller = document.createElement("pre")
  scroller.style.overflowX = "auto"
  Object.defineProperty(scroller, "scrollWidth", { value: 900 })
  Object.defineProperty(scroller, "clientWidth", { value: 300 })
  const line = document.createElement("span")
  scroller.append(line)
  block.attachShadow({ mode: "open" }).append(scroller)
  for (let i = 0; i < 8; i += 1)
    line.dispatchEvent(
      new WheelEvent("wheel", { deltaX: 15, bubbles: true, composed: true }),
    )
  expect(swiped).toEqual([])
})

it("reads nothing while turned off", () => {
  render(false)
  wheel("words", 15)
  expect(swiped).toEqual([])
})

it("turns the sign round where the system does not reverse scrolling", () => {
  render()
  wheel("words", 15, 8, {}, { webkitDirectionInvertedFromDevice: false })
  expect(swiped).toEqual(["right"])
})

it("counts a wheel that moves in lines as lines of pixels", () => {
  render()
  // 4 lines of 16 px pass the distance that 4 pixels would not.
  wheel("words", 1, 4, { deltaMode: WheelEvent.DOM_DELTA_LINE })
  expect(swiped).toEqual(["left"])
})

it("leaves a pinch and a shifted wheel alone", () => {
  render()
  wheel("words", 15, 8, { ctrlKey: true })
  expect(swiped).toEqual([])
  act(() => root.unmount())
  root = createRoot(host)
  render()
  wheel("words", 15, 8, { shiftKey: true })
  expect(swiped).toEqual([])
})

it("leaves a gesture alone over an inert part or an input holding more than it shows", () => {
  render()
  wheel("covered", 15)
  expect(swiped).toEqual([])
  act(() => root.unmount())
  root = createRoot(host)
  render()
  wheel("field", 15)
  expect(swiped).toEqual([])
})
