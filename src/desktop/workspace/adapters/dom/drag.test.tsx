// @vitest-environment jsdom
/**
 * The pointer's drag, where jsdom can follow it: the copy is the pane itself
 * and stays with the pointer, the zone is said, a drop commits what was
 * shown, and Escape lets it all go.
 */
import { act, useRef } from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { useWorkspaceStore } from "../store/hooks"
import { loadWorkspace, openBeside } from "../store/commands"
import { panesOf } from "../../model/pane-layout"
import { settle, testStore } from "../../testing"
import { useWorkspaceDrag } from "./drag"

let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  // jsdom neither animates nor lays out: animations end at once, boxes are set by hand.
  const finished = Promise.resolve()
  Element.prototype.animate = function () {
    return { cancel() {}, finished, id: "" } as unknown as Animation
  }
  Element.prototype.getAnimations = () => []
  host = document.createElement("div")
  document.body.append(host)
})

afterEach(() => host.remove())

function Grid() {
  const root = useRef<HTMLDivElement>(null)
  const store = useWorkspaceStore()
  useWorkspaceDrag(store, root)
  const panes = store.getState().workspace.panes
  return (
    <div ref={root} data-workspace data-sidebar="closed">
      <div className="workspace-panes">
        {(panes ? panesOf(panes) : []).map((pane) => (
          <article
            key={pane.key}
            data-pane-key={pane.key}
            aria-label={`Session ${pane.sessionId}`}
          >
            <header className="workspace-pane-header" data-drag-pane={pane.key}>
              {pane.sessionId}
            </header>
            <div className="workspace-pane-body">
              <div className="workspace-transcript">
                <div className="workspace-transcript-inner">
                  {[0, 1, 2, 3, 4].map((part) => (
                    <p key={part} data-part={part}>
                      {`${pane.sessionId} part ${part}`}
                    </p>
                  ))}
                </div>
              </div>
            </div>
          </article>
        ))}
      </div>
    </div>
  )
}

/** Three frames: the drag's first shows the copy, its second what the zone would do. */
const frames = () =>
  act(
    async () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() =>
          requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
        ),
      ),
  )
const pointer = (type: string, x: number, y: number, target: EventTarget = window) =>
  target.dispatchEvent(
    new PointerEvent(type, { clientX: x, clientY: y, button: 0, bubbles: true }),
  )

async function mounted() {
  const store = testStore()
  await store.dispatch(loadWorkspace())
  await settle()
  store.dispatch(openBeside({ sessionId: "c", side: "right" }))
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <Provider store={store}>
        <Grid />
      </Provider>,
    ),
  )
  const grid = host.querySelector<HTMLElement>(".workspace-panes")
  if (!grid) throw new Error("no grid")
  Object.defineProperty(grid, "offsetWidth", { value: 1100 })
  Object.defineProperty(grid, "offsetHeight", { value: 800 })
  grid.getBoundingClientRect = () => new DOMRect(0, 0, 1100, 800)
  host.querySelectorAll<HTMLElement>("[data-pane-key]").forEach((pane, index) => {
    pane.getBoundingClientRect = () => new DOMRect(index * 554, 0, 546, 800)
  })
  return { store, root }
}

it("carries a copy of the pane with the pointer, and commits the zone it shows on the drop", async () => {
  const { store, root } = await mounted()
  const header = host.querySelector('[data-drag-pane="1"]')
  if (!header) throw new Error("no header")
  pointer("pointerdown", 60, 16, header)
  pointer("pointermove", 90, 40)
  const ghost = host.querySelector<HTMLElement>(".workspace-drag-ghost")
  expect(ghost?.textContent).toContain("a")
  // Grabbed 60, 16 from the pane's corner: its corner stays that far from the pointer.
  expect(ghost?.style.transform).toBe("translate(30px, 24px)")
  // Over the middle of the other pane: a swap, said, and held by a placeholder.
  pointer("pointermove", 830, 400)
  await frames()
  expect(ghost?.style.transform).toBe("translate(770px, 384px)")
  expect(host.querySelector('[role="status"]')?.textContent).toBe("Swap with Session c")
  expect(host.querySelector(".workspace-drag-placeholder")).not.toBeNull()
  pointer("pointerup", 830, 400)
  await frames()
  const after = store.getState().workspace.panes
  expect((after ? panesOf(after) : []).map((pane) => pane.sessionId)).toEqual(["c", "a"])
  await act(async () => root.unmount())
})

it("lets everything go on Escape, the layout untouched", async () => {
  const { store, root } = await mounted()
  const before = store.getState().workspace.panes
  const header = host.querySelector('[data-drag-pane="1"]')
  if (!header) throw new Error("no header")
  pointer("pointerdown", 60, 16, header)
  pointer("pointermove", 830, 400)
  await frames()
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  await frames()
  expect(host.querySelector(".workspace-drag-placeholder")).toBeNull()
  expect(host.querySelector("[data-dragging]")).toBeNull()
  pointer("pointerup", 830, 400)
  expect(store.getState().workspace.panes).toBe(before)
  await act(async () => root.unmount())
})

it("copies one screen of the conversation, what is scrolled above standing in as a spacer", async () => {
  const { root } = await mounted()
  const pane = host.querySelector<HTMLElement>('[data-pane-key="1"]')
  if (!pane) throw new Error("no pane")
  // The conversation shows 300–700; its five parts are 200px tall, from 0.
  const view = pane.querySelector<HTMLElement>(".workspace-transcript")
  if (view) view.getBoundingClientRect = () => new DOMRect(0, 300, 546, 400)
  pane.querySelectorAll<HTMLElement>("[data-part]").forEach((part, index) => {
    part.getBoundingClientRect = () => new DOMRect(0, index * 200, 546, 200)
  })
  pointer("pointerdown", 60, 16, pane.querySelector("header") ?? pane)
  pointer("pointermove", 90, 40)
  const copied = host.querySelector(".workspace-drag-ghost .workspace-transcript-inner")
  const parts = [...(copied?.querySelectorAll("[data-part]") ?? [])]
  expect(parts.map((part) => part.textContent)).toEqual([
    "a part 1",
    "a part 2",
    "a part 3",
  ])
  // What is scrolled out above keeps its height, so the copy shows the same screen.
  expect((copied?.firstElementChild as HTMLElement | null)?.style.height).toBe("200px")
  await act(async () => root.unmount())
})

it("reads where every pane is drawn before it moves any, when the zone changes", async () => {
  const { root } = await mounted()
  const log: ("read" | "write")[] = []
  const animate = Element.prototype.animate
  Element.prototype.animate = function (...args) {
    log.push("write")
    return animate.apply(this, args)
  }
  host.querySelectorAll<HTMLElement>("[data-pane-key]").forEach((pane) => {
    const rect = pane.getBoundingClientRect
    pane.getBoundingClientRect = () => {
      log.push("read")
      return rect()
    }
    for (const child of pane.children)
      Object.defineProperty(child, "offsetTop", {
        get: () => {
          log.push("read")
          return 0
        },
      })
  })
  const header = host.querySelector('[data-drag-pane="1"]')
  if (!header) throw new Error("no header")
  pointer("pointerdown", 60, 16, header)
  pointer("pointermove", 830, 400)
  await frames()
  expect(log).toContain("write")
  // Every read of the page, then every write: the page is laid out once.
  expect(log.join(" ")).toMatch(/^(read )+(write ?)+$/)
  Element.prototype.animate = animate
  await act(async () => root.unmount())
})

it("draws the copy in the frame the drag begins, and what the zone would do in the next", async () => {
  const { root } = await mounted()
  const queued: FrameRequestCallback[] = []
  const request = window.requestAnimationFrame
  window.requestAnimationFrame = (callback) => queued.push(callback)
  const frame = () => act(async () => queued.splice(0).forEach((run) => run(0)))
  const header = host.querySelector('[data-drag-pane="1"]')
  if (!header) throw new Error("no header")
  pointer("pointerdown", 60, 16, header)
  pointer("pointermove", 830, 400)
  expect(host.querySelector(".workspace-drag-ghost")).not.toBeNull()
  await frame()
  expect(host.querySelector(".workspace-drag-placeholder")).toBeNull()
  await frame()
  expect(host.querySelector(".workspace-drag-placeholder")).not.toBeNull()
  window.requestAnimationFrame = request
  pointer("pointerup", 830, 400)
  await act(async () => root.unmount())
})

it("takes the zone the pointer heads for: sideways near the top is the side, upward the top", async () => {
  const { root } = await mounted()
  const header = host.querySelector('[data-drag-pane="1"]')
  if (!header) throw new Error("no header")
  const said = () => host.querySelector('[role="status"]')?.textContent
  // The other pane spans 554–1100; 1034, 40 is 66px from its right and 40 from its top.
  pointer("pointerdown", 60, 16, header)
  for (const x of [700, 800, 900, 1000, 1034]) pointer("pointermove", x, 40)
  await frames()
  expect(said()).toBe("Move right of Session c")
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  pointer("pointerup", 1034, 40)
  await frames()

  pointer("pointerdown", 60, 16, header)
  for (const y of [400, 300, 200, 100, 40]) pointer("pointermove", 1034, y)
  await frames()
  expect(said()).toBe("Move above Session c")
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  pointer("pointerup", 1034, 40)
  await act(async () => root.unmount())
})
