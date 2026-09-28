// @vitest-environment jsdom
/**
 * The pointer's drag, where jsdom can follow it: the copy is the pane itself
 * and stays with the pointer, the zone is said, a drop commits what was
 * shown, and Escape lets it all go. What the drag reads of the page it reads
 * as the press begins; after that it only writes, and selects nothing.
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

/** The press's frame painted, and the task after it: when a drag's copy is made. */
const painted = () =>
  act(
    async () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => setTimeout(() => resolve(), 1)),
      ),
  )

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
  // Aimed at the middle of the other pane — halfway from the pointer to the
  // copy's centre, 106.5, 192 past the pointer — a swap, said, and held by a placeholder.
  pointer("pointermove", 720, 208)
  await frames()
  expect(ghost?.style.transform).toBe("translate(660px, 192px)")
  expect(host.querySelector('[role="status"]')?.textContent).toBe("Swap with Session c")
  expect(host.querySelector(".workspace-drag-placeholder")).not.toBeNull()
  pointer("pointerup", 720, 208)
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

it("reads the page only as the press begins: beginning, previewing, dropping and letting go only write", async () => {
  const { root } = await mounted()
  let reads = 0
  const counted =
    <T,>(read: () => T) =>
    () => {
      reads++
      return read()
    }
  for (const element of host.querySelectorAll<HTMLElement>("*")) {
    const rect = element.getBoundingClientRect.bind(element)
    element.getBoundingClientRect = counted(rect)
  }
  const layoutReads = ["offsetTop", "offsetWidth", "offsetHeight", "scrollTop"] as const
  const kept = layoutReads.map(
    (name) =>
      [name, Object.getOwnPropertyDescriptor(HTMLElement.prototype, name)] as const,
  )
  for (const name of layoutReads)
    Object.defineProperty(HTMLElement.prototype, name, {
      configurable: true,
      get: counted(() => 0),
      set: () => {
        throw new Error(`${name} written`)
      },
    })
  const style = window.getComputedStyle
  window.getComputedStyle = (...args) => {
    reads++
    return style(...args)
  }
  const header = host.querySelector('[data-drag-pane="1"]')
  if (!header) throw new Error("no header")
  try {
    pointer("pointerdown", 60, 16, header)
    // Nothing is read in the press's own frame: what a drag needs is read
    // once that frame has painted.
    expect(reads).toBe(0)
    await painted()
    const atPress = reads
    expect(atPress).toBeGreaterThan(0)
    pointer("pointermove", 90, 40)
    pointer("pointermove", 830, 400)
    await frames()
    pointer("pointermove", 300, 400)
    await frames()
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
    pointer("pointerup", 300, 400)
    await frames()
    pointer("pointerdown", 60, 16, header)
    await painted()
    const again = reads
    pointer("pointermove", 830, 400)
    await frames()
    pointer("pointerup", 830, 400)
    await frames()
    // The drop's command measures the room (`measure`), which the store's
    // dependency does in tests; the drag itself read nothing after the press.
    expect(reads).toBe(again)
    expect(atPress).toBeGreaterThan(0)
  } finally {
    window.getComputedStyle = style
    for (const [name, descriptor] of kept)
      if (descriptor) Object.defineProperty(HTMLElement.prototype, name, descriptor)
  }
  await act(async () => root.unmount())
})

it("selects nothing while pressed or carrying, and leaves nothing selected", async () => {
  const { root } = await mounted()
  const header = host.querySelector('[data-drag-pane="1"]')
  if (!header) throw new Error("no header")
  const starts = () => {
    const event = new Event("selectstart", { bubbles: true, cancelable: true })
    header.dispatchEvent(event)
    return event.defaultPrevented
  }
  expect(starts()).toBe(false)
  pointer("pointerdown", 60, 16, header)
  expect(starts()).toBe(true)
  pointer("pointermove", 830, 400)
  await frames()
  expect(starts()).toBe(true)
  const range = document.createRange()
  range.selectNodeContents(header)
  window.getSelection()?.addRange(range)
  pointer("pointerup", 830, 400)
  expect(window.getSelection()?.rangeCount).toBe(0)
  expect(starts()).toBe(false)
  await act(async () => root.unmount())
})

it("takes away what it made for a press that never becomes a drag", async () => {
  const { root } = await mounted()
  const header = host.querySelector('[data-drag-pane="1"]')
  if (!header) throw new Error("no header")
  pointer("pointerdown", 60, 16, header)
  // Made once the press's frame has painted, unseen.
  await painted()
  expect(host.querySelector(".workspace-drag-ghost")?.hasAttribute("data-waiting")).toBe(
    true,
  )
  pointer("pointerup", 61, 16)
  expect(host.querySelector(".workspace-drag-ghost")).toBeNull()
  expect(host.querySelector("[data-dragging]")).toBeNull()
  await act(async () => root.unmount())
})

it("copies one screen of the conversation, what is scrolled above standing in as a spacer", async () => {
  const { root } = await mounted()
  const pane = host.querySelector<HTMLElement>('[data-pane-key="1"]')
  if (!pane) throw new Error("no pane")
  // The conversation shows 300–700; its five parts are 200px tall, from 0.
  const view = pane.querySelector<HTMLElement>(".workspace-transcript")
  if (view) {
    view.getBoundingClientRect = () => new DOMRect(0, 300, 546, 400)
    Object.defineProperty(view, "scrollTop", { value: 300 })
  }
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
  // Where it was scrolled to, by transform: a scroll would lay the copy out at once.
  expect((copied as HTMLElement | null)?.style.transform).toBe("translateY(-300px)")
  expect(host.querySelector(".workspace-drag-ghost")?.hasAttribute("data-waiting")).toBe(
    false,
  )
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
  // The other pane spans 554–1100. The drag aims 106.5, 192 past the pointer:
  // from 927, 8 it aims at 1033.5, 200 — 66px from the pane's right, 200 from its top.
  // A few milliseconds apart, as a real pointer's moves are: its heading is read from their times.
  const apart = () => new Promise((resolve) => setTimeout(resolve, 4))
  pointer("pointerdown", 60, 16, header)
  for (const x of [600, 700, 800, 900, 927]) {
    pointer("pointermove", x, 8)
    await apart()
  }
  await frames()
  expect(said()).toBe("Move right of Session c")
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  pointer("pointerup", 927, 8)
  await frames()

  pointer("pointerdown", 60, 16, header)
  for (const y of [400, 300, 200, 100, 8]) {
    pointer("pointermove", 927, y)
    await apart()
  }
  await frames()
  expect(said()).toBe("Move above Session c")
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  pointer("pointerup", 927, 8)
  await act(async () => root.unmount())
})

it("aims past the grid, or into a gutter, at the pane nearest; at the carried pane's own place, at nothing", async () => {
  const { root } = await mounted()
  const header = host.querySelector('[data-drag-pane="1"]')
  if (!header) throw new Error("no header")
  const said = () => host.querySelector('[role="status"]')?.textContent
  pointer("pointerdown", 60, 16, header)
  // The copy runs off the window's right: its aim, past the grid, is the last column's side.
  pointer("pointermove", 1080, 300)
  await frames()
  expect(said()).toBe("Move right of Session c")
  // The gutter between the panes (546–554): the pane beside it, never nothing.
  pointer("pointermove", 445.5, 208)
  await frames()
  expect(said()).toMatch(/Session c$/)
  // Back over its own place: nothing offered, and let go there it goes back.
  pointer("pointermove", 60, 16)
  await frames()
  expect(said()).toBe("")
  expect(host.querySelector(".workspace-drag-placeholder")).toBeNull()
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  pointer("pointerup", 60, 16)
  await act(async () => root.unmount())
})

it("lets go of nothing when released off the grid, or when the page loses the pointer", async () => {
  const { store, root } = await mounted()
  const before = store.getState().workspace.panes
  const header = host.querySelector('[data-drag-pane="1"]')
  if (!header) throw new Error("no header")
  const said = () => host.querySelector('[role="status"]')?.textContent
  pointer("pointerdown", 60, 16, header)
  pointer("pointermove", 720, 208)
  await frames()
  expect(said()).toBe("Swap with Session c")
  // Out of the window, with no move the frame has read yet: released there, it goes back.
  pointer("pointerup", 1300, 400)
  await frames()
  expect(store.getState().workspace.panes).toBe(before)
  expect(host.querySelector("[data-dragging]")).toBeNull()

  // Moved off the grid, nothing is offered.
  pointer("pointerdown", 60, 16, header)
  pointer("pointermove", 720, 208)
  await frames()
  pointer("pointermove", 720, 900)
  await frames()
  expect(said()).toBe("")
  pointer("pointerup", 720, 900)
  await frames()
  expect(store.getState().workspace.panes).toBe(before)

  // The pointer taken from the page mid-drag: as Escape.
  pointer("pointerdown", 60, 16, header)
  pointer("pointermove", 720, 208)
  await frames()
  host
    .querySelector(".workspace-drag-shield")
    ?.dispatchEvent(new PointerEvent("lostpointercapture"))
  pointer("pointerup", 720, 208)
  await frames()
  expect(store.getState().workspace.panes).toBe(before)
  await act(async () => root.unmount())
})
