// @vitest-environment jsdom
import { expect, it } from "vitest"
import { measureWorkspace } from "./measure"

/** jsdom lays nothing out: the boxes are set by hand, as the browser would. */
function box(element: HTMLElement, width: number, height: number) {
  Object.defineProperty(element, "offsetWidth", { value: width })
  Object.defineProperty(element, "offsetHeight", { value: height })
}

it("measures the grid as laid out, whatever transform a flight draws it with", () => {
  const page = document.createElement("div")
  page.innerHTML = `<div data-workspace data-sidebar="open">
      <aside class="workspace-sidebar"></aside>
      <div class="workspace-panes" style="transform: scale(0.5)"></div>
    </div>`
  const grid = page.querySelector<HTMLElement>(".workspace-panes")!
  box(grid, 900, 700)
  // What a flight would report mid-way; the room must not read it.
  grid.getBoundingClientRect = () => new DOMRect(0, 0, 450, 350)
  box(page.querySelector<HTMLElement>(".workspace-sidebar")!, 240, 700)
  expect(measureWorkspace(page)).toEqual({ width: 900, height: 700, spare: 248 })
})

it("counts no spare room for a sidebar already folded, and no room without a grid", () => {
  const page = document.createElement("div")
  page.innerHTML = `<div data-workspace data-sidebar="closed">
      <aside class="workspace-sidebar"></aside>
      <div class="workspace-panes"></div>
    </div>`
  box(page.querySelector<HTMLElement>(".workspace-panes")!, 900, 700)
  expect(measureWorkspace(page)?.spare).toBe(0)
  expect(measureWorkspace(document.createElement("div"))).toBeUndefined()
})
