// @vitest-environment jsdom
/**
 * The column's title goes where the laid-out widths say (`titlePlacement`):
 * inline in the titlebar row when it fits there, below it when not — and
 * moves when the room changes, before the frame paints.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { titleBreathingRoom } from "../model/column-title"
import { ColumnHeader } from "./column-header"

/** A ResizeObserver whose widths the test sets, as the page's layout would. */
const observed = new Set<{ callback: ResizeObserverCallback; targets: Element[] }>()
const widths = new Map<string, number>()
const widthOf = (element: Element) =>
  widths.get(
    element.classList.contains("desktop-column-sizer")
      ? "title"
      : element.classList.contains("desktop-column-action")
        ? "action"
        : "row",
  ) ?? 0

function layOut(next: { row: number; title: number; action: number }) {
  for (const [key, value] of Object.entries(next)) widths.set(key, value)
  for (const { callback, targets } of observed)
    callback(
      targets.map(
        (target) =>
          ({ target, contentRect: { width: widthOf(target) } }) as ResizeObserverEntry,
      ),
      {} as ResizeObserver,
    )
}

let host: HTMLDivElement
let root: Root

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  class Observer {
    private entry = {
      callback: (() => {}) as ResizeObserverCallback,
      targets: [] as Element[],
    }
    constructor(callback: ResizeObserverCallback) {
      this.entry.callback = callback
      observed.add(this.entry)
    }
    observe(target: Element) {
      this.entry.targets.push(target)
    }
    unobserve() {}
    disconnect() {
      observed.delete(this.entry)
    }
  }
  Object.assign(globalThis, { ResizeObserver: Observer })
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(() => {
  act(() => root.unmount())
  host.remove()
  widths.clear()
})

const title = () =>
  host.querySelector<HTMLElement>(".desktop-column-title:not(.desktop-column-sizer)")

it("sits inline in the titlebar row when it fits, and below it when it does not", () => {
  act(() =>
    root.render(<ColumnHeader title="desktop-app" action={<button type="button" />} />),
  )
  act(() => layOut({ row: 300, title: 110, action: 28 }))
  expect(title()?.dataset.placement).toBe("inline")
  expect(title()?.parentElement?.classList.contains("desktop-column-bar")).toBe(true)

  // The sidebar folds: the row's content now starts after the window's controls.
  act(() => layOut({ row: 110, title: 110, action: 28 }))
  expect(title()?.dataset.placement).toBe("below")
  expect(title()?.previousElementSibling?.classList.contains("desktop-column-bar")).toBe(
    true,
  )

  // Room again, with room to breathe.
  act(() => layOut({ row: 110 + 28 + titleBreathingRoom, title: 110, action: 28 }))
  expect(title()?.dataset.placement).toBe("inline")
})

it("draws one title, whichever row it is in, and a column without one measures nothing", () => {
  act(() => root.render(<ColumnHeader title="Running" />))
  act(() => layOut({ row: 400, title: 80, action: 0 }))
  expect(host.querySelectorAll("h2")).toHaveLength(1)
  act(() => root.render(<ColumnHeader action={<button type="button" />} />))
  expect(host.querySelector(".desktop-column-title")).toBeNull()
  expect(host.querySelector(".desktop-column-bar")?.hasAttribute("data-title")).toBe(
    false,
  )
})
