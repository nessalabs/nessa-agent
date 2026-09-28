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
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { useWorkspaceStore } from "../store/hooks"
import {
  closePane,
  focusPane,
  followWorkspace,
  loadWorkspace,
  openBeside,
  showContent,
} from "../store/commands"
import { panesOf } from "../../model/pane-layout"
import { fakeSource, settle, testStore } from "../../testing"
import { restAfter } from "../../model/drop"
import type { WorkspaceRoom } from "../../model/pane-sizing"
import { measureWorkspace } from "./measure"
import { saying, useWorkspaceDrag } from "./drag"

let host: HTMLDivElement
/** Every animation asked for, and of what. */
const animated: { element: Element; keyframes: Keyframe[]; animation: Animation }[] = []

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  // jsdom neither animates nor lays out: animations end at once, boxes are set by hand.
  const finished = Promise.resolve()
  animated.length = 0
  Element.prototype.animate = function (this: Element, keyframes) {
    const animation = { cancel() {}, finished, id: "" } as unknown as Animation
    animated.push({ element: this, keyframes: keyframes as Keyframe[], animation })
    return animation
  }
  Element.prototype.getAnimations = () => []
  host = document.createElement("div")
  document.body.append(host)
})

afterEach(() => {
  host.remove()
  // A test that gave the document a clock takes it away again.
  Reflect.deleteProperty(document, "timeline")
})

function Grid() {
  const root = useRef<HTMLDivElement>(null)
  const store = useWorkspaceStore()
  useWorkspaceDrag(store, root)
  const panes = store.getState().workspace.panes
  return (
    <div ref={root} data-workspace data-sidebar="closed">
      <div data-drag-session="d">Session d</div>
      <nav className="workspace-sidebar" />
      <div className="workspace-panes">
        {(panes ? panesOf(panes) : []).map((pane) => (
          <article
            key={pane.key}
            data-pane-key={pane.key}
            aria-label={`Session ${pane.sessionId}`}
          >
            <header className="workspace-pane-header" data-drag-pane={pane.key}>
              {pane.sessionId}
              <button type="button" aria-label="Close Pane" />
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
/** The primary button's pointer: held from its press to its release. */
const pointer = (
  type: string,
  x: number,
  y: number,
  target: EventTarget = window,
  init: PointerEventInit = {},
) =>
  target.dispatchEvent(
    new PointerEvent(type, {
      clientX: x,
      clientY: y,
      button: 0,
      buttons: type === "pointerup" ? 0 : 1,
      bubbles: true,
      cancelable: true,
      ...init,
    }),
  )

/**
 * Two panes, a and c, in an 1100 × 800 grid, laid out by hand: the room is
 * the page's own measure of it (`measureWorkspace`) unless a test says
 * otherwise.
 */
async function mounted(
  measure: () => WorkspaceRoom | undefined = () => measureWorkspace(host),
  source = fakeSource(),
) {
  const store = testStore(source, measure)
  await store.dispatch(loadWorkspace())
  await settle()
  const root = createRoot(host)
  const render = () =>
    act(async () =>
      root.render(
        <Provider store={store}>
          <Grid />
        </Provider>,
      ),
    )
  await render()
  const grid = host.querySelector<HTMLElement>(".workspace-panes")
  if (!grid) throw new Error("no grid")
  Object.defineProperty(grid, "offsetWidth", { value: 1100 })
  Object.defineProperty(grid, "offsetHeight", { value: 800 })
  grid.getBoundingClientRect = () => new DOMRect(0, 0, 1100, 800)
  // Opened beside in the room the page has, then drawn.
  store.dispatch(openBeside({ sessionId: "c", side: "right" }))
  await render()
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
  // The carrier is the pointer; the copy is drawn about its centre (273, 400),
  // held where it was grabbed, 60, 16 from its corner, and glides until its
  // centre is under the pointer.
  expect(carrier()?.style.transform).toBe("translate(90px, 40px)")
  expect(ghost?.style.transform).toBe("translate(-273px, -400px)")
  const glider = ghost?.parentElement
  expect(glider?.style.transform).toBe("translate(213px, 384px)")
  const glide = animated.find((entry) => entry.element === glider)
  expect(glide?.keyframes.map((frame) => frame.transform)).toEqual([
    "translate(213px, 384px)",
    "translate(0px, 0px)",
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
  // The page's own measure of the room, counted and not stubbed away.
  let measured = 0
  const { root, store } = await mounted(() => {
    measured++
    return measureWorkspace(host)
  })
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
    measured = 0
    await press(60, 16, header(1))
    const again = reads
    // The room is measured once, as the press begins…
    expect(measured).toBe(1)
    pointer("pointermove", 830, 400)
    pointer("pointermove", 827, 400)
    await frames()
    pointer("pointerup", 827, 400)
    await frames()
    // …and the preview and the drop's command are given it: nothing after the press reads.
    expect(reads).toBe(again)
    expect(measured).toBe(1)
    const after = store.getState().workspace.panes
    expect((after ? panesOf(after) : []).map((pane) => pane.sessionId)).toEqual([
      "c",
      "a",
    ])
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

it("lifts only once its copy is made: a move before then reads nothing and lifts nothing", async () => {
  const { store, root } = await mounted()
  const before = store.getState().workspace.panes
  const header1 = header(1)
  let reads = 0
  for (const element of host.querySelectorAll<HTMLElement>("*")) {
    const rect = element.getBoundingClientRect.bind(element)
    element.getBoundingClientRect = () => {
      reads++
      return rect()
    }
  }
  // Pressed and moved within the press's own frame.
  pointer("pointerdown", 60, 16, header1)
  pointer("pointermove", 90, 40)
  pointer("pointermove", 827, 400)
  expect(reads).toBe(0)
  expect(carrier()).toBeNull()
  // Made once the frame has painted, unseen: the next move lifts it where the pointer is.
  await painted()
  expect(reads).toBeGreaterThan(0)
  expect(host.querySelector(".workspace-drag-ghost")?.hasAttribute("data-waiting")).toBe(
    true,
  )
  pointer("pointermove", 826, 400)
  expect(carrier()?.style.transform).toBe("translate(826px, 400px)")
  pointer("pointermove", 825, 400)
  await frames()
  expect(said()).toBe("Swap with Session c")
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  pointer("pointerup", 826, 400)
  await frames()
  expect(store.getState().workspace.panes).toBe(before)
  await act(async () => root.unmount())
})

it("commits only a drop that was previewed: a flick, in any engine's timing, changes nothing", async () => {
  const { store, root } = await mounted()
  const before = store.getState().workspace.panes
  // Down, across and up in one task: a click, nothing made, nothing dropped.
  pointer("pointerdown", 60, 16, header(1))
  pointer("pointermove", 827, 400)
  pointer("pointerup", 827, 400)
  await painted()
  await frames()
  nothingLeft()
  expect(host.querySelector(".workspace-drag-ghost")).toBeNull()
  expect(store.getState().workspace.panes).toBe(before)
  // Lifted, and let go over a zone before its preview was shown: home.
  await press(60, 16, header(1))
  pointer("pointermove", 90, 40)
  pointer("pointermove", 827, 400)
  pointer("pointerup", 827, 400)
  await frames()
  nothingLeft()
  expect(store.getState().workspace.panes).toBe(before)
  await act(async () => root.unmount())
})

it("commits the zone on the page when let go in another before the frame drew it", async () => {
  const { store, root } = await mounted()
  await press(60, 16, header(1))
  pointer("pointermove", 90, 40)
  pointer("pointermove", 827, 400)
  await frames()
  expect(said()).toBe("Swap with Session c")
  pointer("pointermove", 1090, 400)
  pointer("pointerup", 1090, 400)
  await frames()
  const after = store.getState().workspace.panes
  expect((after ? panesOf(after) : []).map((pane) => pane.sessionId)).toEqual(["c", "a"])
  nothingLeft()
  await act(async () => root.unmount())
})

it("carries with the primary button alone: another button ends the press or the drag, and none other presses one", async () => {
  const { store, root } = await mounted()
  const before = store.getState().workspace.panes
  // A press of another button starts nothing — made nothing, holds nothing:
  // its release taken by the menu it opens, the next primary press still carries.
  pointer("pointerdown", 60, 16, header(1), { button: 2, buttons: 2 })
  await painted()
  expect(host.querySelector(".workspace-drag-ghost")).toBeNull()
  await press(60, 16, header(1))
  pointer("pointermove", 90, 40)
  expect(carrier()).not.toBeNull()
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  pointer("pointerup", 90, 40)
  await frames()
  // Carrying, a chord — the right button joining the left — ends it: home, nothing dropped.
  // (A chorded button reaches the page as a move, never a press: the Pointer
  // Events spec's chorded button interactions.)
  for (const chord of [
    () => pointer("pointermove", 828, 400, window, { button: 2, buttons: 3 }),
  ]) {
    await press(60, 16, header(1))
    pointer("pointermove", 90, 40)
    pointer("pointermove", 827, 400)
    await frames()
    expect(said()).toBe("Swap with Session c")
    chord()
    await frames()
    nothingLeft()
    pointer("pointermove", 830, 400, window, { buttons: 1 })
    pointer("pointerup", 830, 400)
    await frames()
    expect(carrier()).toBeNull()
    expect(store.getState().workspace.panes).toBe(before)
  }
  // Pressed, a move with no button held lets the press go.
  await press(60, 16, header(1))
  pointer("pointermove", 90, 40, window, { buttons: 0 })
  pointer("pointermove", 827, 400)
  await frames()
  expect(carrier()).toBeNull()
  pointer("pointerup", 827, 400)
  await act(async () => root.unmount())
})

it("leaves a press on a control inside a header to the control", async () => {
  const { root } = await mounted()
  const close = header(1).querySelector("button")
  if (!close) throw new Error("no control")
  pointer("pointerdown", 500, 16, close)
  await painted()
  pointer("pointermove", 827, 400)
  await frames()
  expect(carrier()).toBeNull()
  expect(host.querySelector(".workspace-drag-ghost")).toBeNull()
  pointer("pointerup", 827, 400)
  await act(async () => root.unmount())
})

it("swallows the click its own release makes, and nothing for another pointer's", async () => {
  const { root } = await mounted()
  const clicks: EventTarget[] = []
  const heard = (event: Event) => clicks.push(event.target as EventTarget)
  host.addEventListener("click", heard)
  await press(60, 16, header(1))
  pointer("pointermove", 90, 40)
  pointer("pointermove", 827, 400)
  await frames()
  // Another pointer lets go: its click is its own.
  pointer("pointerup", 300, 300, window, { pointerId: 7 })
  header(2).dispatchEvent(new MouseEvent("click", { bubbles: true }))
  expect(clicks).toHaveLength(1)
  expect(carrier()).not.toBeNull()
  // The drag's own release: the click after it is not a click on what it started from.
  pointer("pointerup", 827, 400)
  header(2).dispatchEvent(new MouseEvent("click", { bubbles: true }))
  expect(clicks).toHaveLength(1)
  await frames()
  host.removeEventListener("click", heard)
  await act(async () => root.unmount())
})

it("never aims over a side column: the sidebar revealed over the panes is no zone", async () => {
  const { store, root } = await mounted()
  const before = store.getState().workspace.panes
  const scope = host.querySelector<HTMLElement>("[data-workspace]")
  const sidebar = host.querySelector<HTMLElement>(".workspace-sidebar")
  if (!scope || !sidebar) throw new Error("no sidebar")
  // Revealed from the edge, over pane 1's left and the gutter's side of pane c? No: 8–264.
  scope.dataset.peek = ""
  sidebar.getBoundingClientRect = () => new DOMRect(8, 8, 256, 784)
  host.querySelectorAll<HTMLElement>("[data-pane-key]").forEach((pane, index) => {
    pane.getBoundingClientRect = () => new DOMRect(index * 554, 0, 546, 800)
  })
  const row = host.querySelector("[data-drag-session]")
  if (!row) throw new Error("no row")
  await press(10, 10, row)
  pointer("pointermove", 40, 40)
  pointer("pointermove", 100, 400)
  await act(async () => new Promise((resolve) => setTimeout(resolve, restAfter + 30)))
  await frames()
  expect(said()).toBe("")
  expect(host.querySelector(".workspace-drag-placeholder")).toBeNull()
  // Past its edge, pane a is a target as ever.
  pointer("pointermove", 300, 400)
  await frames()
  expect(said()).toMatch(/Session a$/)
  pointer("pointermove", 100, 400)
  await frames()
  expect(said()).toBe("")
  pointer("pointerup", 100, 400)
  await frames()
  expect(store.getState().workspace.panes).toBe(before)
  nothingLeft()
  await act(async () => root.unmount())
})

it("takes a peek still sliding in as covering where it slides to, not the part of the way it has come", async () => {
  const { store, root } = await mounted()
  const before = store.getState().workspace.panes
  const scope = host.querySelector<HTMLElement>("[data-workspace]")
  const sidebar = host.querySelector<HTMLElement>(".workspace-sidebar")
  if (!scope || !sidebar) throw new Error("no sidebar")
  // Revealed just before the press: drawn 200px short of its place, 8–264,
  // by the transform it slides on (a browser computes it as a matrix).
  scope.dataset.peek = ""
  sidebar.getBoundingClientRect = () => new DOMRect(-192, 8, 256, 784)
  const computed = window.getComputedStyle
  const style = vi.spyOn(window, "getComputedStyle").mockImplementation((element) => {
    const real = computed(element)
    if (element !== sidebar) return real
    return new Proxy(real, {
      get: (target, key) =>
        key === "transform" ? "matrix(1, 0, 0, 1, -200, 0)" : Reflect.get(target, key),
    })
  })
  const row = host.querySelector("[data-drag-session]")
  if (!row) throw new Error("no row")
  await press(10, 10, row)
  style.mockRestore()
  pointer("pointermove", 40, 40)
  // Past where it is drawn now (64), inside where it settles (264): no zone.
  pointer("pointermove", 100, 400)
  await act(async () => new Promise((resolve) => setTimeout(resolve, restAfter + 30)))
  await frames()
  expect(said()).toBe("")
  pointer("pointerup", 100, 400)
  await frames()
  expect(store.getState().workspace.panes).toBe(before)
  nothingLeft()
  await act(async () => root.unmount())
})

it("ends at once when the carried session is no longer listed", async () => {
  const source = fakeSource()
  const { store, root } = await mounted(undefined, source)
  const stop = store.dispatch(followWorkspace())
  const before = store.getState().workspace.panes
  const row = host.querySelector("[data-drag-session]")
  if (!row) throw new Error("no row")
  await press(10, 10, row)
  pointer("pointermove", 40, 40)
  pointer("pointermove", 827, 400)
  await frames()
  expect(carrier()).not.toBeNull()
  // The session is removed by the source; the panes and the view are untouched.
  await act(async () =>
    source.emit({ kind: "session-removed", sessionId: "d", revision: 9 }),
  )
  expect(store.getState().workspace.panes).toBe(before)
  await frames()
  nothingLeft()
  pointer("pointerup", 827, 400)
  await frames()
  expect(store.getState().workspace.panes).toBe(before)
  stop()
  await act(async () => root.unmount())
})

it("never takes Settings' Escape: under an inert window the keys are not the drag's", async () => {
  const { root } = await mounted()
  await press(60, 16, header(1))
  pointer("pointermove", 90, 40)
  pointer("pointermove", 827, 400)
  await frames()
  // Settings opens over the window, and its Escape comes before anything else runs.
  host.setAttribute("inert", "")
  const escape = new KeyboardEvent("keydown", { key: "Escape", cancelable: true })
  window.dispatchEvent(escape)
  expect(escape.defaultPrevented).toBe(false)
  await frames()
  nothingLeft()
  host.removeAttribute("inert")
  pointer("pointerup", 827, 400)
  await act(async () => root.unmount())
})

it("says each zone as a person would: above and below, left of and right of", () => {
  const outcome = (does: "move" | "split") =>
    ({ does, lands: 1, layout: null, foldSidebar: false }) as unknown as Parameters<
      typeof saying
    >[0]
  expect(saying(outcome("split"), "top", "Notes")).toBe("Split above Notes")
  expect(saying(outcome("split"), "bottom", "Notes")).toBe("Split below Notes")
  expect(saying(outcome("split"), "left", "Notes")).toBe("Split left of Notes")
  expect(saying(outcome("move"), "top", "Notes")).toBe("Move above Notes")
  expect(saying(outcome("move"), "right", "Notes")).toBe("Move right of Notes")
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

it("draws the copy at the slot it would land in, and the panes at the rects the drop gives them — none stretched — and at their own with no zone", async () => {
  const { store, root } = await mounted()
  const ghost = () => host.querySelector<HTMLElement>(".workspace-drag-ghost")
  const placeholder = () => host.querySelector<HTMLElement>(".workspace-drag-placeholder")
  const pane = (key: number) =>
    host.querySelector<HTMLElement>(`[data-pane-key="${key}"]`) ?? new HTMLElement()
  /** The last transform of the last animation asked of `element`. */
  const lastDrawn = (element: Element | null) =>
    animated
      .filter((entry) => entry.element === element)
      .at(-1)
      ?.keyframes.at(-1)?.transform
  /**
   * At every step of the last change of shape asked of `box`, its scale times
   * the scale its content (`content`) is drawn at back: 1, 1 where nothing is stretched.
   */
  const stretch = (box: Element | null, content: Element | null) => {
    const scales = (element: Element | null) =>
      (animated.filter((entry) => entry.element === element).at(-1)?.keyframes ?? []).map(
        (frame) =>
          (/scale\(([^,]+), ([^)]+)\)/.exec(String(frame.transform)) ?? [])
            .slice(1)
            .map(Number),
      )
    const outer = scales(box)
    const inner = scales(content)
    expect(outer.length).toBeGreaterThan(2)
    expect(inner.length).toBe(outer.length)
    return outer.map(([x, y], step) => [
      Number((x * inner[step][0]).toFixed(6)),
      Number((y * inner[step][1]).toFixed(6)),
    ])
  }
  const unstretched = (box: Element | null, content: Element | null) =>
    expect(new Set(stretch(box, content).flat())).toEqual(new Set([1]))
  // The document's clock, so the page can start a box and its content at one time.
  Object.defineProperty(document, "timeline", {
    configurable: true,
    value: { currentTime: 1234 },
  })
  /** When the last animation asked of `element` starts. */
  const startOf = (element: Element | null) =>
    animated.filter((entry) => entry.element === element).at(-1)?.animation.startTime
  await press(60, 16, header(1))
  // Up pane c's middle: above it. The tall pane a would lie on top, the width of the grid.
  for (const y of [400, 300, 200, 100, 30]) {
    pointer("pointermove", 827, y)
    await apart()
  }
  await frames()
  expect(said()).toBe("Move above Session c")
  expect(placeholder()?.style.width).toBe("1100px")
  expect(placeholder()?.style.height).toBe("396px")
  // The copy is laid out at the slot's size, its centre on the pointer, at rest unscaled.
  expect(ghost()?.style.width).toBe("1100px")
  expect(ghost()?.style.height).toBe("396px")
  expect(lastDrawn(ghost())).toBe("translate(-550px, -198px) scale(1, 1)")
  expect(lastDrawn(ghost()?.firstElementChild ?? null)).toBe("scale(1, 1)")
  // Pane c goes below it (0, 404, 1100 × 396): drawn there by transform
  // from where it is laid out (554, 0, 546 × 800), its content scaled back —
  // cut to that shape, never laid out again.
  expect(pane(2).style.width).toBe("")
  expect(lastDrawn(pane(2))).toBe(
    `translate(-277px, 202px) scale(${1100 / 546}, ${396 / 800})`,
  )
  // Pane a's slot would be the window's corner: its header steps past the controls there.
  expect(pane(1).getAttribute("data-drag-corner")).toBe("yes")
  expect(pane(2).getAttribute("data-drag-corner")).toBeNull()
  // Its content is scaled back as a pane that shape lays it out: the header
  // held to the top left, the conversation to the top and centred across
  // (273 is half the width it has) — cut to the shape where it is smaller.
  const [paneHeader, paneBody] = Array.from(pane(2).children) as HTMLElement[]
  expect(paneHeader.style.transformOrigin).toBe("0px 0px")
  expect(paneBody.style.transformOrigin).toBe("273px 0px")
  // Each box and its content start at one time: never a frame apart.
  for (const [box, content] of [
    [pane(2), pane(2).firstElementChild],
    [ghost(), ghost()?.firstElementChild ?? null],
  ] as const) {
    expect(startOf(box)).toBe(1234)
    expect(startOf(content)).toBe(1234)
  }
  // On the way, every step: the box scaled, its content scaled back exactly.
  unstretched(ghost(), ghost()?.firstElementChild ?? null)
  unstretched(pane(2), pane(2).firstElementChild)
  // Off the grid: nothing offered, so the copy is its own size and the panes their own.
  pointer("pointermove", 1200, 300)
  await frames()
  expect(said()).toBe("")
  expect(ghost()?.style.width).toBe("546px")
  expect(ghost()?.style.height).toBe("800px")
  expect(lastDrawn(ghost())).toBe("translate(-273px, -400px) scale(1, 1)")
  expect(lastDrawn(pane(2))).toBe("translate(0px, 0px) scale(1, 1)")
  // Out from under the controls once its motion ends — here, at once — its header steps back.
  expect(pane(1).getAttribute("data-drag-corner")).toBeNull()
  // Back above it and dropped: it lands at the slot previewed; no pane keeps a size of the drag's.
  for (const y of [400, 300, 200, 100, 30]) {
    pointer("pointermove", 827, y)
    await apart()
  }
  await frames()
  expect(said()).toBe("Move above Session c")
  pointer("pointerup", 827, 30)
  await frames()
  const after = store.getState().workspace.panes
  expect((after ? panesOf(after) : []).map((each) => each.sessionId)).toEqual(["a", "c"])
  for (const each of host.querySelectorAll<HTMLElement>("[data-pane-key]")) {
    expect([each.style.width, each.style.height]).toEqual(["", ""])
    expect(each.hasAttribute("data-drag-corner")).toBe(false)
  }
  nothingLeft()
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
  // Lost, then the pointer keeps moving and is released over a zone.
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
    // Seen as it happens — Settings included — with no move of the pointer.
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
