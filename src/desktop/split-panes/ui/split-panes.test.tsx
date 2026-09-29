// @vitest-environment jsdom
/**
 * The grid on a host of its own: it renders what the source holds, hands
 * each pane its frame to spread on the host's own root — no element around
 * it — renders again only when the arrangement changes, and carries out a
 * resize, an equalize and a fit through the source, never on a layout of its
 * own.
 */
import { act, StrictMode } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import type { EdgeMove, SplitPanesSource } from "../application/ports"
import { focusPane, singlePane, splitPane, type PaneLayout } from "../model/pane-layout"
import { SplitPanes, type ShownPane } from "./split-panes"

let host: HTMLDivElement
let root: Root
/** Each grid the page observes, and how to tell it its size changed. */
const observed: { target: Element; resized: () => void }[] = []

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  observed.length = 0
  globalThis.ResizeObserver = class {
    constructor(private readonly callback: ResizeObserverCallback) {}
    observe(target: Element) {
      observed.push({ target, resized: () => this.callback([], this) })
    }
    unobserve() {}
    disconnect() {
      observed.length = 0
    }
  }
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

function fakeSource(start: PaneLayout | null) {
  let layout = start
  const listeners = new Set<() => void>()
  const asked = { resized: [] as EdgeMove[], equalized: 0, fitted: 0 }
  const source: SplitPanesSource = {
    layout: () => layout,
    subscribe: (onChange) => {
      listeners.add(onChange)
      return () => listeners.delete(onChange)
    },
    watched: () => [],
    holds: () => true,
    targetable: () => true,
    measure: () => ({ width: 1100, height: 800, spare: 0 }),
    commitDrop: () => {},
    resize: (move) => asked.resized.push(move),
    equalize: () => asked.equalized++,
    fit: () => asked.fitted++,
  }
  return {
    source,
    asked,
    change: async (next: PaneLayout) => {
      layout = next
      await act(async () => listeners.forEach((listener) => listener()))
    },
  }
}

const renders: number[] = []
const renderPane = ({ placement, frame, multi }: ShownPane) => {
  renders.push(placement.key)
  return (
    <article className="host-pane" {...frame} data-multi-host={multi || undefined}>
      <header>{placement.key}</header>
    </article>
  )
}

const two = () => splitPane(singlePane("a"), 1, "right", "b")

async function mounted(fake: ReturnType<typeof fakeSource>) {
  renders.length = 0
  await act(async () =>
    root.render(
      <StrictMode>
        <SplitPanes
          source={fake.source}
          renderPane={renderPane}
          empty={<p className="host-empty">Nothing open</p>}
        />
      </StrictMode>,
    ),
  )
}

const grid = () => {
  const found = host.querySelector<HTMLElement>("[data-split-grid]")
  if (!found) throw new Error("no grid")
  return found
}

it("puts the frame on the host's own pane root, with nothing around it", async () => {
  await mounted(fakeSource(two()))
  const panes = [...grid().children].filter((child) => child.matches(".host-pane"))
  expect(panes).toHaveLength(2)
  const [first] = panes as HTMLElement[]
  // The pane root is the grid's child: FLIP and the preview move it, and scale its children back.
  expect(first.parentElement).toBe(grid())
  expect(first.dataset.paneKey).toBe("1")
  expect(first.dataset.flip).toBe("pane")
  expect(first.dataset.flipId).toBe("1")
  expect(first.hasAttribute("data-corner")).toBe(true)
  expect(first.style.getPropertyValue("--cw")).toBe("0.5")
  expect(first.hasAttribute("data-multi-host")).toBe(true)
  expect(grid().hasAttribute("data-multi")).toBe(true)
  expect(host.querySelector(".host-empty")).toBeNull()
})

it("shows the host's empty state while there are no panes", async () => {
  await mounted(fakeSource(null))
  expect(grid().querySelector(".host-empty")?.textContent).toBe("Nothing open")
  expect(grid().hasAttribute("data-multi")).toBe(false)
})

it("renders again when the arrangement changes, and not when only focus moves", async () => {
  const layout = two()
  const fake = fakeSource(layout)
  await mounted(fake)
  renders.length = 0
  await fake.change(focusPane(layout, 2))
  expect(renders).toEqual([])
  await fake.change(splitPane(layout, 2, "bottom", "c"))
  expect(renders.length).toBeGreaterThan(0)
  expect(host.querySelectorAll(".host-pane")).toHaveLength(3)
})

it("resizes, evens and fits through the source", async () => {
  const fake = fakeSource(two())
  await mounted(fake)
  const edge = grid().querySelector<HTMLElement>('[role="separator"]')
  if (!edge) throw new Error("no edge")
  expect(edge.getAttribute("aria-label")).toBe("Resize Columns")
  // The keyboard moves it: 16px of the two sides' 1092px, from the half each has.
  await act(async () =>
    edge.dispatchEvent(
      new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }),
    ),
  )
  expect(fake.asked.resized).toEqual([
    { edge: { axis: "x", column: 0 }, fraction: (546 + 16) / 1092, pair: 1092 },
  ])
  await act(async () => edge.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })))
  expect(fake.asked.equalized).toBe(1)
  const fitted = fake.asked.fitted
  await act(async () => observed.find((each) => each.target === grid())?.resized())
  expect(fake.asked.fitted).toBe(fitted + 1)
})
