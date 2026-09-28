// @vitest-environment jsdom
/**
 * The pointer's drag, where jsdom can follow it: the page's side of
 * `model/drag.ts`. The copy is the pane itself, held under the pointer and
 * gliding to its centre; the zone under the pointer is said, a drop commits
 * what was shown, and Escape, a lost pointer or any change lets it all go.
 * What the drag reads of the page it reads as the press begins; after that it
 * only writes, and selects nothing.
 */
import { act, useRef } from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { useWorkspaceStore } from "../store/hooks"
import {
  closePane,
  focusPane,
  loadWorkspace,
  openBeside,
  showContent,
} from "../store/commands"
import { panesOf } from "../../model/pane-layout"
import { settle, testStore } from "../../testing"
import { restAfter } from "../../model/drop"
import { useWorkspaceDrag } from "./drag"

let host: HTMLDivElement
/** Every animation asked for, and of what. */
const animated: { element: Element; keyframes: Keyframe[] }[] = []

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  // jsdom neither animates nor lays out: animations end at once, boxes are set by hand.
  const finished = Promise.resolve()
  animated.length = 0
  Element.prototype.animate = function (this: Element, keyframes) {
    animated.push({ element: this, keyframes: keyframes as Keyframe[] })
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
      <div data-drag-session="d">Session d</div>
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
/** A person's press: down, and its frame painted before the pointer moves on. */
const press = async (x: number, y: number, target: EventTarget) => {
  pointer("pointerdown", x, y, target)
  await painted()
}
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

const carrier = () => host.querySelector<HTMLElement>(".workspace-drag-carrier")
const said = () => host.querySelector('[role="status"]')?.textContent
const header = (key: number) => {
  const found = host.querySelector(`[data-drag-pane="${key}"]`)
  if (!found) throw new Error(`no header ${key}`)
  return found
}
/** Nothing of a drag is left on the page. */
const nothingLeft = () => {
  expect(carrier()).toBeNull()
  expect(host.querySelector(".workspace-drag-placeholder")).toBeNull()
  expect(host.querySelector(".workspace-drag-shield")).toBeNull()
  expect(host.querySelector("[data-dragging]")).toBeNull()
  expect(host.querySelector("[data-lifted]")).toBeNull()
}
const apart = () => new Promise((resolve) => setTimeout(resolve, 4))

it("carries the copy under the pointer, gliding to its centre, and commits the zone it shows", async () => {
  const { store, root } = await mounted()
  await press(60, 16, header(1))
  pointer("pointermove", 90, 40)
  const ghost = host.querySelector<HTMLElement>(".workspace-drag-ghost")
  expect(ghost?.textContent).toContain("a")
  // The carrier is the pointer; the copy is held where it was grabbed, 60, 16
  // from its corner, and glides until its centre (273, 400) is under the pointer.
  expect(carrier()?.style.transform).toBe("translate(90px, 40px)")
  expect(ghost?.style.transform).toBe("translate(-60px, -16px)")
  const glide = animated.find((entry) => entry.element === ghost)
  expect(glide?.keyframes.map((frame) => frame.transform)).toEqual([
    "translate(-60px, -16px)",
    "translate(-273px, -400px)",
  ])
  // The middle of the other pane, where the pointer is: a swap, said, and held by a placeholder.
  pointer("pointermove", 827, 400)
  await frames()
  expect(carrier()?.style.transform).toBe("translate(827px, 400px)")
  expect(said()).toBe("Swap with Session c")
  expect(host.querySelector(".workspace-drag-placeholder")).not.toBeNull()
  pointer("pointerup", 827, 400)
  await frames()
  const after = store.getState().workspace.panes
  expect((after ? panesOf(after) : []).map((pane) => pane.sessionId)).toEqual(["c", "a"])
  nothingLeft()
  await act(async () => root.unmount())
})

it("lets everything go on Escape, the layout untouched", async () => {
  const { store, root } = await mounted()
  const before = store.getState().workspace.panes
  await press(60, 16, header(1))
  pointer("pointermove", 830, 400)
  pointer("pointermove", 827, 400)
  await frames()
  const escape = new KeyboardEvent("keydown", { key: "Escape", cancelable: true })
  window.dispatchEvent(escape)
  // Escape goes no further than the drag.
  expect(escape.defaultPrevented).toBe(true)
  await frames()
  nothingLeft()
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
  try {
    pointer("pointerdown", 60, 16, header(1))
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
    await press(60, 16, header(1))
    const again = reads
    pointer("pointermove", 830, 400)
    pointer("pointermove", 827, 400)
    await frames()
    pointer("pointerup", 827, 400)
    await frames()
    // The drop's command measures the room (`measure`), which the store's
    // dependency does in tests; the drag itself read nothing after the press.
    expect(reads).toBe(again)
  } finally {
    window.getComputedStyle = style
    for (const [name, descriptor] of kept)
      if (descriptor) Object.defineProperty(HTMLElement.prototype, name, descriptor)
  }
  await act(async () => root.unmount())
})

it("selects nothing while pressed or carrying, and leaves nothing selected", async () => {
  const { root } = await mounted()
  const starts = () => {
    const event = new Event("selectstart", { bubbles: true, cancelable: true })
    header(1).dispatchEvent(event)
    return event.defaultPrevented
  }
  expect(starts()).toBe(false)
  await press(60, 16, header(1))
  expect(starts()).toBe(true)
  pointer("pointermove", 830, 400)
  await frames()
  expect(starts()).toBe(true)
  const range = document.createRange()
  range.selectNodeContents(header(1))
  window.getSelection()?.addRange(range)
  pointer("pointerup", 830, 400)
  expect(window.getSelection()?.rangeCount).toBe(0)
  await frames()
  expect(starts()).toBe(false)
  await act(async () => root.unmount())
})

it("takes away what it made for a press that never becomes a drag", async () => {
  const { root } = await mounted()
  await press(60, 16, header(1))
  // Made once the press's frame has painted, unseen.
  await painted()
  expect(host.querySelector(".workspace-drag-ghost")?.hasAttribute("data-waiting")).toBe(
    true,
  )
  pointer("pointerup", 61, 16)
  nothingLeft()
  expect(host.querySelector(".workspace-drag-ghost")).toBeNull()
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
  await press(60, 16, header(1))
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
  await press(60, 16, header(1))
  const queued: FrameRequestCallback[] = []
  const request = window.requestAnimationFrame
  window.requestAnimationFrame = (callback) => queued.push(callback)
  const frame = () => act(async () => queued.splice(0).forEach((run) => run(0)))
  pointer("pointermove", 830, 400)
  pointer("pointermove", 827, 400)
  expect(host.querySelector(".workspace-drag-ghost")?.hasAttribute("data-waiting")).toBe(
    false,
  )
  await frame()
  expect(host.querySelector(".workspace-drag-placeholder")).toBeNull()
  await frame()
  expect(host.querySelector(".workspace-drag-placeholder")).not.toBeNull()
  window.requestAnimationFrame = request
  pointer("pointerup", 830, 400)
  await act(async () => root.unmount())
})

it("shows a drag begun before the press's frame painted once its copy is made, reading nothing in the event", async () => {
  const { store, root } = await mounted()
  const header1 = header(1)
  let reads = 0
  for (const element of host.querySelectorAll<HTMLElement>("*")) {
    const rect = element.getBoundingClientRect.bind(element)
    element.getBoundingClientRect = () => {
      reads++
      return rect()
    }
  }
  // A flick: pressed and moved within the press's own frame.
  pointer("pointerdown", 60, 16, header1)
  pointer("pointermove", 90, 40)
  pointer("pointermove", 827, 400)
  expect(reads).toBe(0)
  expect(carrier()).toBeNull()
  // Made once the frame has painted, and shown then, where the pointer is.
  await painted()
  expect(carrier()?.style.transform).toBe("translate(827px, 400px)")
  expect(reads).toBeGreaterThan(0)
  pointer("pointermove", 826, 400)
  await frames()
  expect(said()).toBe("Swap with Session c")
  pointer("pointerup", 826, 400)
  await frames()
  const after = store.getState().workspace.panes
  expect((after ? panesOf(after) : []).map((pane) => pane.sessionId)).toEqual(["c", "a"])
  // Let go before it was ever shown: nothing is left, nothing dropped.
  pointer("pointerdown", 60, 16, header1)
  pointer("pointermove", 400, 400)
  pointer("pointerup", 400, 400)
  await painted()
  await frames()
  nothingLeft()
  expect(host.querySelector(".workspace-drag-ghost")).toBeNull()
  await act(async () => root.unmount())
})

it("takes the zone the pointer heads for: sideways near the top is the side, upward the top", async () => {
  const { root } = await mounted()
  // The other pane spans 554–1100; 30px down, heading right, 40px from its right edge.
  await press(60, 16, header(1))
  for (const x of [700, 800, 900, 1000, 1060]) {
    pointer("pointermove", x, 30)
    await apart()
  }
  await frames()
  expect(said()).toBe("Move right of Session c")
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  pointer("pointerup", 1060, 30)
  await frames()

  await press(60, 16, header(1))
  for (const y of [400, 300, 200, 100, 30]) {
    pointer("pointermove", 827, y)
    await apart()
  }
  await frames()
  expect(said()).toBe("Move above Session c")
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  pointer("pointerup", 827, 30)
  await act(async () => root.unmount())
})

it("settles the zone once the pointer rests: the heading ages out", async () => {
  const { root } = await mounted()
  await press(60, 16, header(1))
  // Heading right fast, 220px from pane c's right edge: its right…
  for (const x of [600, 700, 800, 880]) {
    pointer("pointermove", x, 400)
    await apart()
  }
  await frames()
  expect(said()).toBe("Move right of Session c")
  // …and at rest there, its middle.
  await act(async () => new Promise((resolve) => setTimeout(resolve, restAfter + 30)))
  await frames()
  expect(said()).toBe("Swap with Session c")
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  pointer("pointerup", 880, 400)
  await act(async () => root.unmount())
})

it("aims at the pointer: off the grid at nothing, a gutter at the pane nearest, its own place at nothing", async () => {
  const { root } = await mounted()
  await press(60, 16, header(1))
  pointer("pointermove", 90, 40)
  pointer("pointermove", 1090, 300)
  await frames()
  expect(said()).toMatch(/Session c$/)
  // Past the grid's right: nothing.
  pointer("pointermove", 1200, 300)
  await frames()
  expect(said()).toBe("")
  // The gutter between the panes (546–554), nearer c: c.
  pointer("pointermove", 552, 208)
  await frames()
  expect(said()).toMatch(/Session c$/)
  // Back over its own place: nothing offered.
  pointer("pointermove", 273, 400)
  await frames()
  expect(said()).toBe("")
  expect(host.querySelector(".workspace-drag-placeholder")).toBeNull()
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  pointer("pointerup", 273, 400)
  await act(async () => root.unmount())
})

it("lets go of nothing when released off the grid", async () => {
  const { store, root } = await mounted()
  const before = store.getState().workspace.panes
  await press(60, 16, header(1))
  pointer("pointermove", 830, 400)
  pointer("pointermove", 827, 400)
  await frames()
  expect(said()).toBe("Swap with Session c")
  // Out of the window, with no move the frame has read yet: released there, it goes back.
  pointer("pointerup", 1300, 400)
  await frames()
  expect(store.getState().workspace.panes).toBe(before)
  nothingLeft()
  await act(async () => root.unmount())
})

it("ends press and drag when the page loses the pointer: a later move and release do nothing", async () => {
  const { store, root } = await mounted()
  const before = store.getState().workspace.panes
  await press(60, 16, header(1))
  pointer("pointermove", 830, 400)
  pointer("pointermove", 827, 400)
  await frames()
  host
    .querySelector(".workspace-drag-shield")
    ?.dispatchEvent(new PointerEvent("lostpointercapture"))
  await frames()
  nothingLeft()
  // The reviewer's order: the pointer keeps moving, then is released over a zone.
  pointer("pointermove", 800, 410)
  await frames()
  expect(carrier()).toBeNull()
  pointer("pointerup", 827, 400)
  await frames()
  expect(store.getState().workspace.panes).toBe(before)
  // The same for a press lost before it becomes a drag, and for a blur.
  for (const lose of [
    () => window.dispatchEvent(new PointerEvent("pointercancel")),
    () => window.dispatchEvent(new Event("blur")),
  ]) {
    await press(60, 16, header(1))
    lose()
    pointer("pointermove", 830, 400)
    pointer("pointerup", 827, 400)
    await frames()
    nothingLeft()
    expect(store.getState().workspace.panes).toBe(before)
  }
  await act(async () => root.unmount())
})

it("ends at once on a change: a command key, a resize, the store, Settings", async () => {
  const { store, root } = await mounted()
  const changes: [string, () => void][] = [
    [
      "⌘W",
      () =>
        window.dispatchEvent(new KeyboardEvent("keydown", { key: "w", metaKey: true })),
    ],
    ["a resize", () => window.dispatchEvent(new Event("resize"))],
    ["an agent's close", () => void store.dispatch(closePane({ pane: 2 }))],
    ["the overview", () => void store.dispatch(showContent({ content: "agents" }))],
    ["Settings", () => host.setAttribute("inert", "")],
  ]
  for (const [name, change] of changes) {
    const before = store.getState().workspace
    await press(60, 16, header(1))
    pointer("pointermove", 830, 400)
    pointer("pointermove", 827, 400)
    await frames()
    // A lone modifier, as a chord begins, changes nothing.
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Meta", metaKey: true }))
    expect(carrier(), name).not.toBeNull()
    change()
    // Settings is seen at the next move of the pointer.
    pointer("pointermove", 826, 401)
    await frames()
    // Gone at once, not flown home: nothing lifted, nothing previewed, no copy.
    expect(carrier(), name).toBeNull()
    nothingLeft()
    pointer("pointerup", 827, 400)
    await frames()
    const sessions = () => {
      const panes = store.getState().workspace.panes
      return panes ? panesOf(panes).map((pane) => pane.sessionId) : []
    }
    const was = before.panes ? panesOf(before.panes).map((pane) => pane.sessionId) : []
    // What the change did, it did; the drop did nothing on top of it: the
    // panes as they were, or — closed by the change — the one left.
    expect(sessions(), name).toEqual(sessions().length === was.length ? was : ["a"])
    host.removeAttribute("inert")
    store.dispatch(showContent({ content: "panes" }))
    if (!sessions().includes("c"))
      store.dispatch(openBeside({ sessionId: "c", side: "right" }))
  }
  await act(async () => root.unmount())
})

it("keeps carrying when only focus moves: pressing a pane focuses it, and that is no change", async () => {
  const { store, root } = await mounted()
  store.dispatch(focusPane({ pane: 2 }))
  await press(60, 16, header(1))
  // What a press on a pane's header does first.
  store.dispatch(focusPane({ pane: 1 }))
  pointer("pointermove", 830, 400)
  pointer("pointermove", 827, 400)
  await frames()
  expect(carrier()).not.toBeNull()
  expect(said()).toBe("Swap with Session c")
  pointer("pointerup", 827, 400)
  await frames()
  const after = store.getState().workspace.panes
  expect((after ? panesOf(after) : []).map((pane) => pane.sessionId)).toEqual(["c", "a"])
  await act(async () => root.unmount())
})

it("offers no zone while the overview covers the panes: a session carried there drops nowhere", async () => {
  const { store, root } = await mounted()
  store.dispatch(showContent({ content: "agents" }))
  const before = store.getState().workspace.panes
  const row = host.querySelector("[data-drag-session]")
  if (!row) throw new Error("no row")
  await press(10, 10, row)
  pointer("pointermove", 830, 400)
  pointer("pointermove", 827, 400)
  await frames()
  await act(async () => new Promise((resolve) => setTimeout(resolve, restAfter + 30)))
  await frames()
  expect(said()).toBe("")
  expect(host.querySelector(".workspace-drag-placeholder")).toBeNull()
  pointer("pointerup", 827, 400)
  await frames()
  expect(store.getState().workspace.panes).toBe(before)
  nothingLeft()
  await act(async () => root.unmount())
})
