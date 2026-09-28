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
          </article>
        ))}
      </div>
    </div>
  )
}

const frames = () => act(async () => new Promise((resolve) => setTimeout(resolve, 40)))
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
