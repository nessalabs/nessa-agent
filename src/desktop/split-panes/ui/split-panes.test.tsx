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
import { resizeEdge, type PaneRoom } from "../model/pane-sizing"
import { gridOf, marks } from "../adapters/dom/marks"
import { SplitPanes, type ShownPane } from "./split-panes"

let host: HTMLDivElement
let root: Root
/** Each grid the page observes, and how to tell it its size changed. */
const observed: { target: Element; resized: () => void }[] = []

/**
 * jsdom fires animation frames on an interval. A busy loop can run one
 * before a test asks, so the frames here run only when `run` says so.
 */
function steppedFrames() {
  const queued = new Map<number, FrameRequestCallback>()
  let next = 1
  const request = window.requestAnimationFrame
  const cancel = window.cancelAnimationFrame
  window.requestAnimationFrame = (callback) => {
    const id = next++
    queued.set(id, callback)
    return id
  }
  window.cancelAnimationFrame = (id) => {
    queued.delete(id)
  }
  return {
    async run() {
      const pending = [...queued.keys()]
      await act(async () => {
        for (const id of pending) {
          const callback = queued.get(id)
          if (callback === undefined) continue
          queued.delete(id)
          callback(0)
        }
      })
    },
    restore() {
      queued.clear()
      window.requestAnimationFrame = request
      window.cancelAnimationFrame = cancel
    },
  }
}

let frames: ReturnType<typeof steppedFrames>

beforeEach(() => {
  frames = steppedFrames()
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
  frames.restore()
  host.remove()
})

/**
 * A host's source that records what it is asked. It can give what a host may:
 * no room measured (`room: null`), or no layout — told (`change(null)`) or
 * not yet (`withhold()`, as a layout that goes between two events).
 */
function fakeSource(
  start: PaneLayout | null,
  { room = { width: 1100, height: 800, spare: 0 } }: { room?: PaneRoom | null } = {},
) {
  let layout = start
  let withheld = false
  const listeners = new Set<() => void>()
  const asked = { resized: [] as EdgeMove[], equalized: 0, fitted: 0 }
  const source: SplitPanesSource = {
    layout: () => (withheld ? null : layout),
    subscribe: (onChange) => {
      listeners.add(onChange)
      return () => listeners.delete(onChange)
    },
    watched: () => [],
    holds: () => true,
    targetable: () => true,
    measure: () => room ?? undefined,
    commitDrop: () => {},
    resize: (move) => asked.resized.push(move),
    equalize: () => asked.equalized++,
    fit: () => asked.fitted++,
  }
  return {
    source,
    asked,
    withhold: () => {
      withheld = true
    },
    change: async (next: PaneLayout | null) => {
      layout = next
      await act(async () => listeners.forEach((listener) => listener()))
    },
    // Notifies without wrapping `act`, so a frame callback can run it while
    // a draw is still queued.
    replace: (next: PaneLayout | null) => {
      layout = next
      for (const listener of listeners) listener()
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
  const found = gridOf(host)
  if (!found) throw new Error("no grid")
  return found
}

/** The frame a column change paints on: three frames after it commits. */
const nextPaint = async () => {
  await frames.run()
  await frames.run()
  await frames.run()
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
  expect(first.hasAttribute(marks.corner)).toBe(true)
  expect(first.style.getPropertyValue("--cw")).toBe("0.5")
  expect(first.hasAttribute("data-multi-host")).toBe(true)
  expect(grid().hasAttribute(marks.multi)).toBe(true)
  expect(host.querySelector(".host-empty")).toBeNull()
})

it("shows the host's empty state while there are no panes", async () => {
  await mounted(fakeSource(null))
  expect(grid().querySelector(".host-empty")?.textContent).toBe("Nothing open")
  expect(grid().hasAttribute(marks.multi)).toBe(false)
})

it("paints a new arrangement on the frame after it commits", async () => {
  host.setAttribute("data-workspace", "")
  const layout = two()
  const fake = fakeSource(layout)
  await mounted(fake)
  await fake.change(splitPane(layout, 2, "bottom", "c"))
  // The commit's own turn keeps the boxes already painted and writes
  // nothing. The next frame holds a folded sidebar in the row, and so does
  // the frame after that: a drop's copy lets go, then a column title glides.
  expect(grid().querySelectorAll(".host-pane")).toHaveLength(2)
  expect(host.hasAttribute("data-spare-held")).toBe(false)
  await frames.run()
  expect(host.hasAttribute("data-spare-held")).toBe(true)
  await frames.run()
  expect(grid().querySelectorAll(".host-pane")).toHaveLength(2)
  expect(host.hasAttribute("data-spare-held")).toBe(true)
  await frames.run()
  expect(grid().querySelectorAll(".host-pane")).toHaveLength(3)
  expect(host.hasAttribute("data-spare-held")).toBe(false)
  // That paint skips scene text. The frame after shows it again.
  expect(grid().hasAttribute(marks.painting)).toBe(true)
  await nextPaint()
  expect(grid().hasAttribute(marks.painting)).toBe(false)
})

it("replaces a draw the source left before that draw commits", async () => {
  host.setAttribute("data-workspace", "")
  const layout = two()
  const fake = fakeSource(layout)
  await mounted(fake)
  const stacked = splitPane(layout, 2, "bottom", "c")
  await fake.change(stacked)
  await frames.run()
  await frames.run()
  expect(grid().querySelectorAll(".host-pane")).toHaveLength(2)
  expect(host.hasAttribute("data-spare-held")).toBe(true)
  // Queued behind the callback that hands React the new boxes, so both run
  // before that update commits. The source is back on the painted arrangement.
  requestAnimationFrame(() => fake.replace(layout))
  await frames.run()
  expect(grid().querySelectorAll(".host-pane")).toHaveLength(2)
  expect(host.hasAttribute("data-spare-held")).toBe(false)
  // The grid is still following the source. A later split waits, then paints.
  await fake.change(stacked)
  expect(grid().querySelectorAll(".host-pane")).toHaveLength(2)
  await nextPaint()
  expect(grid().querySelectorAll(".host-pane")).toHaveLength(3)
  expect(host.hasAttribute("data-spare-held")).toBe(false)
})

it("does not paint a draw a later callback in that frame replaced", async () => {
  host.setAttribute("data-workspace", "")
  const layout = two()
  const fake = fakeSource(layout)
  await mounted(fake)
  await fake.change(splitPane(layout, 2, "bottom", "c"))
  await frames.run()
  await frames.run()
  const beside = splitPane(layout, 1, "left", "d")
  requestAnimationFrame(() => fake.replace(beside))
  await frames.run()
  // The boxes that were about to commit stay off the screen. The replacement
  // arrangement waits its own three frames, and the spare hold stays up.
  expect(grid().querySelectorAll(".host-pane")).toHaveLength(2)
  expect(host.hasAttribute("data-spare-held")).toBe(true)
  await frames.run()
  await frames.run()
  expect(grid().querySelectorAll(".host-pane")).toHaveLength(2)
  await frames.run()
  expect(grid().querySelectorAll(".host-pane")).toHaveLength(3)
  expect(host.hasAttribute("data-spare-held")).toBe(false)
})

it("paints a replaced source's layout though that source never notifies", async () => {
  await mounted(fakeSource(two()))
  expect(grid().querySelectorAll(".host-pane")).toHaveLength(2)
  const next = fakeSource(splitPane(two(), 2, "bottom", "c"))
  await act(async () => {
    root.render(
      <StrictMode>
        <SplitPanes
          source={next.source}
          renderPane={renderPane}
          empty={<p className="host-empty">Nothing open</p>}
        />
      </StrictMode>,
    )
  })
  // The commit keeps the boxes already painted. The new source's columns
  // were never announced; the grid reads them when it subscribes.
  expect(grid().querySelectorAll(".host-pane")).toHaveLength(2)
  await nextPaint()
  expect(grid().querySelectorAll(".host-pane")).toHaveLength(3)
})

it("renders again when the arrangement changes, and not when only focus moves", async () => {
  const layout = two()
  const fake = fakeSource(layout)
  await mounted(fake)
  renders.length = 0
  await fake.change(focusPane(layout, 2))
  expect(renders).toEqual([])
  await fake.change(splitPane(layout, 2, "bottom", "c"))
  // The commit re-renders the boxes already painted. The new pane waits three frames.
  expect(renders.every((key) => key < 3)).toBe(true)
  expect(host.querySelectorAll(".host-pane")).toHaveLength(2)
  await nextPaint()
  expect(renders.some((key) => key !== 1 && key !== 2)).toBe(true)
  expect(host.querySelectorAll(".host-pane")).toHaveLength(3)
})

it("paints a resize of the same panes on the frame it happens", async () => {
  const layout = two()
  const fake = fakeSource(layout)
  await mounted(fake)
  await frames.run()
  const first = () => grid().querySelector<HTMLElement>(".host-pane")
  expect(first()?.style.getPropertyValue("--cw")).toBe("0.5")
  expect(grid().hasAttribute(marks.painting)).toBe(false)
  const wider = resizeEdge(layout, { axis: "x", column: 0 }, 0.7, 1000)
  await fake.change(wider)
  expect(first()?.style.getPropertyValue("--cw")).toBe("0.7")
  expect(grid().querySelectorAll(".host-pane")).toHaveLength(2)
  expect(host.hasAttribute("data-spare-held")).toBe(false)
  const narrower = resizeEdge(wider, { axis: "x", column: 0 }, 0.35, 1000)
  await fake.change(narrower)
  expect(first()?.style.getPropertyValue("--cw")).toBe("0.35")
  expect(grid().hasAttribute(marks.painting)).toBe(false)
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

it("asks nothing of an edge moved with no room measured, or with the layout gone", async () => {
  const unmeasured = fakeSource(two(), { room: null })
  await mounted(unmeasured)
  const edgeOf = () => {
    const found = grid().querySelector<HTMLElement>('[role="separator"]')
    if (!found) throw new Error("no edge")
    return found
  }
  const nudge = () =>
    act(async () =>
      edgeOf().dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }),
      ),
    )
  await nudge()
  expect(unmeasured.asked.resized).toEqual([])
  // Measured, but the layout gone before the host has said so.
  const gone = fakeSource(two())
  await mounted(gone)
  gone.withhold()
  await nudge()
  expect(gone.asked.resized).toEqual([])
  // Said: the grid shows the host's empty state on the next frame, and no edge.
  await gone.change(null)
  expect(grid().querySelector('[role="separator"]')).not.toBeNull()
  await nextPaint()
  expect(grid().querySelector('[role="separator"]')).toBeNull()
  expect(grid().querySelector(".host-empty")).not.toBeNull()
})
