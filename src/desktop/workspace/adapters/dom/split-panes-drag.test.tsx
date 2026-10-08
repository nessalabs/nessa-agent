// @vitest-environment jsdom
/**
 * The pointer's drag in the workspace, where jsdom can follow it: the split
 * panes' drag (`useSplitPanesDrag`) wired to the workspace's store
 * (`workspaceSplitPanes`) and page (`workspaceDragOptions`), as the window
 * wires it — the page's side of `split-panes/model/drag.ts`. The copy is the
 * pane itself, held under the pointer and gliding to its centre; the zone
 * under the pointer is said, a drop commits what was shown, and Escape, a
 * lost pointer or any change lets it all go. What the drag reads of the page
 * it reads as the press begins; after that it only writes, and selects
 * nothing. What the drag asks of any host is held on a fake one beside the
 * drag (`split-panes/adapters/dom/drag.test.tsx`).
 */
import { act, useMemo, useRef } from "react"
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
  toggleSidebar,
} from "../store/commands"
import { panesOf } from "../../../split-panes/model/pane-layout"
import { fakeSource, settle, shownBy, shownIn, testStore } from "../../testing"
import { paneItemKey, sessionItem } from "../../model/pane-item"
import { restAfter } from "../../../split-panes/model/drop"
import type { PaneRoom } from "../../../split-panes/model/pane-sizing"
import { measureWorkspace } from "./measure"
import { workspaceSplitPanes } from "../store/split-panes-source"
import { useSplitPanesDrag } from "../../../split-panes"
import { classes, gridOf, marks } from "../../../split-panes"
import { dragCard, dragCardSize, workspaceDragOptions } from "./split-panes-drag"

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
  const source = useMemo(() => workspaceSplitPanes(store), [store])
  const options = useMemo(workspaceDragOptions, [])
  useSplitPanesDrag(root, source, options)
  const panes = store.getState().workspace.panes
  return (
    <div ref={root} data-workspace data-sidebar="closed">
      <div data-drag-item={paneItemKey(sessionItem("d"))}>Session d</div>
      <nav className="workspace-sidebar" />
      <section className="workspace-list" />
      <div className="split-panes-grid" data-split-grid>
        {(panes ? panesOf(panes) : []).map((pane) => (
          <article
            key={pane.key}
            data-pane-key={pane.key}
            aria-label={`Session ${shownBy(pane)}`}
          >
            <header
              className="workspace-pane-header"
              data-split-keeps="top-left"
              data-drag-pane={pane.key}
            >
              {shownBy(pane)}
              <button type="button" aria-label="Close Pane" />
            </header>
            <div className="workspace-pane-body" data-split-through>
              <div className="workspace-transcript" data-split-scroll>
                <div className="workspace-transcript-inner">
                  {[0, 1, 2, 3, 4].map((part) => (
                    <p key={part} data-part={part}>
                      {`${shownBy(pane)} part ${part}`}
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
  measure: () => PaneRoom | undefined = () => measureWorkspace(host),
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
  const grid = gridOf(host)
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

const carrier = () => host.querySelector<HTMLElement>(`.${classes.carrier}`)
const said = () => host.querySelector('[role="status"]')?.textContent
const header = (key: number) => {
  const found = host.querySelector(`[data-drag-pane="${key}"]`)
  if (!found) throw new Error(`no header ${key}`)
  return found
}
/** Nothing of a drag is left on the page. */
const nothingLeft = () => {
  expect(carrier()).toBeNull()
  expect(host.querySelector(`.${classes.placeholder}`)).toBeNull()
  expect(host.querySelector(`.${classes.shield}`)).toBeNull()
  expect(host.querySelector(`[${marks.carrying}]`)).toBeNull()
  expect(host.querySelector(`[${marks.lifted}]`)).toBeNull()
}
const apart = () => new Promise((resolve) => setTimeout(resolve, 4))

it("carries the copy under the pointer, gliding to its centre, and commits the zone it shows", async () => {
  const { store, root } = await mounted()
  await press(60, 16, header(1))
  pointer("pointermove", 90, 40)
  const ghost = host.querySelector<HTMLElement>(`.${classes.ghost}`)
  expect(ghost?.textContent).toContain("a")
  // The carrier is the pointer; the copy is drawn about its centre (273, 400),
  // held where it was grabbed, 60, 16 from its corner, and glides until its
  // centre is under the pointer.
  expect(carrier()?.style.transform).toBe("translate(90px, 40px)")
  expect(ghost?.style.transform).toBe("translate(-140px, -22px)")
  const glider = ghost?.parentElement
  expect(glider?.style.transform).toBe("translate(0px, 0px)")
  const glide = animated.find((entry) => entry.element === glider)
  expect(glide?.keyframes.map((frame) => frame.transform)).toEqual([
    "translate(0px, 0px)",
    "translate(0px, 0px)",
  ])
  // The middle of the other pane, where the pointer is: a swap, said, and held by a placeholder.
  pointer("pointermove", 827, 400)
  await frames()
  expect(carrier()?.style.transform).toBe("translate(827px, 400px)")
  expect(said()).toBe("Swap with Session c")
  expect(host.querySelector(`.${classes.placeholder}`)).not.toBeNull()
  pointer("pointerup", 827, 400)
  await frames()
  const after = store.getState().workspace.panes
  expect(shownIn(after)).toEqual(["c", "a"])
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
    expect(shownIn(after)).toEqual(["c", "a"])
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
  expect(host.querySelector(`.${classes.ghost}`)?.hasAttribute(marks.waiting)).toBe(true)
  pointer("pointerup", 61, 16)
  nothingLeft()
  expect(host.querySelector(`.${classes.ghost}`)).toBeNull()
  await act(async () => root.unmount())
})

it("pictures the pane's header and leaves its conversation out of the copy", async () => {
  const { root } = await mounted()
  const pane = host.querySelector<HTMLElement>('[data-pane-key="1"]')
  if (!pane) throw new Error("no pane")
  // The window's drag region and the focused pane's mark are no part of a copy.
  // The header picture's night scene is tens of thousands of characters; shaping
  // it is the lift's long layout.
  header(1).setAttribute("data-tauri-drag-region", "")
  pane.setAttribute("data-pane-focused", "")
  const sliver = document.createElement("div")
  sliver.dataset.sliver = ""
  sliver.textContent = "night"
  pane.prepend(sliver)
  await press(60, 16, header(1))
  pointer("pointermove", 90, 40)
  const ghost = host.querySelector(`.${classes.ghost}`)
  expect(ghost?.querySelector("[data-tauri-drag-region], [data-pane-focused]")).toBeNull()
  expect(ghost?.querySelector("[data-sliver]")).toBeNull()
  expect(ghost?.querySelector(".workspace-drag-title")?.textContent).toBe(
    pane.getAttribute("aria-label"),
  )
  // The conversation is not painted, and building it is the press's long frame.
  expect(ghost?.querySelector(".workspace-pane-body, .workspace-transcript")).toBeNull()
  expect(host.querySelector(`.${classes.ghost}`)?.hasAttribute(marks.waiting)).toBe(false)
  await act(async () => root.unmount())
})

it("carries only a row's title and icon, never its preview or a composer", () => {
  const pressed = document.createElement("div")
  pressed.innerHTML =
    '<span class="workspace-agent-tile">A</span><span class="workspace-session-title">Session d</span><p>Long conversation preview</p><textarea>draft</textarea>'
  const copy = dragCard(
    { kind: "item", item: "session:d" },
    {
      pressed,
      pane: null,
      picture: (element) => element.cloneNode(true) as HTMLElement,
      focusedPane: null,
    },
  )
  expect(copy.textContent).toBe("ASession d")
  expect(
    copy.querySelector("textarea, .workspace-transcript, .workspace-pane-body"),
  ).toBeNull()
  expect(workspaceDragOptions().copySize).toEqual(dragCardSize)
  expect(workspaceDragOptions().previewPanes).toBe(false)
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
  expect(host.querySelector(`.${classes.ghost}`)?.hasAttribute(marks.waiting)).toBe(false)
  await frame()
  expect(host.querySelector(`.${classes.placeholder}`)).toBeNull()
  await frame()
  expect(host.querySelector(`.${classes.placeholder}`)).not.toBeNull()
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
  expect(host.querySelector(`.${classes.ghost}`)?.hasAttribute(marks.waiting)).toBe(true)
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
  expect(host.querySelector(`.${classes.ghost}`)).toBeNull()
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
  expect(shownIn(after)).toEqual(["c", "a"])
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
  expect(host.querySelector(`.${classes.ghost}`)).toBeNull()
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
  expect(host.querySelector(`.${classes.ghost}`)).toBeNull()
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
  const row = host.querySelector("[data-drag-item]")
  if (!row) throw new Error("no row")
  await press(10, 10, row)
  pointer("pointermove", 40, 40)
  pointer("pointermove", 100, 400)
  await act(async () => new Promise((resolve) => setTimeout(resolve, restAfter + 30)))
  await frames()
  expect(said()).toBe("")
  expect(host.querySelector(`.${classes.placeholder}`)).toBeNull()
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
  const row = host.querySelector("[data-drag-item]")
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

it("never aims over the session list docked beside the panes", async () => {
  const { store, root } = await mounted()
  const before = store.getState().workspace.panes
  const scope = host.querySelector<HTMLElement>("[data-workspace]")
  const list = host.querySelector<HTMLElement>(".workspace-list")
  if (!scope || !list) throw new Error("no list")
  // Docked over pane c's left, 554–854.
  scope.dataset.list = "open"
  list.getBoundingClientRect = () => new DOMRect(554, 0, 300, 800)
  const row = host.querySelector("[data-drag-item]")
  if (!row) throw new Error("no row")
  await press(10, 10, row)
  pointer("pointermove", 40, 40)
  pointer("pointermove", 700, 400)
  await act(async () => new Promise((resolve) => setTimeout(resolve, restAfter + 30)))
  await frames()
  expect(said()).toBe("")
  // Past it, pane c is a target as ever.
  pointer("pointermove", 950, 400)
  await frames()
  expect(said()).toMatch(/Session c$/)
  pointer("pointermove", 700, 400)
  await frames()
  expect(said()).toBe("")
  pointer("pointerup", 700, 400)
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
  const row = host.querySelector("[data-drag-item]")
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

it("holds the zone the pointer moved into as it comes to rest", async () => {
  const { root } = await mounted()
  await press(60, 16, header(1))
  // Heading right fast, 220px from pane c's right edge: its right…
  for (const x of [600, 700, 800, 880]) {
    pointer("pointermove", x, 400)
    await apart()
  }
  await frames()
  expect(said()).toBe("Move right of Session c")
  // …and at rest there, still its right: the panes do not swing back as the hand stops.
  await act(async () => new Promise((resolve) => setTimeout(resolve, restAfter + 30)))
  await frames()
  expect(said()).toBe("Move right of Session c")
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
  expect(host.querySelector(`.${classes.placeholder}`)).toBeNull()
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  pointer("pointerup", 273, 400)
  await act(async () => root.unmount())
})

it("keeps a compact card and live panes unchanged while highlighting the hovered zone", async () => {
  const { store, root } = await mounted()
  const before = store.getState().workspace.panes
  await press(60, 16, header(1))
  for (const y of [400, 300, 200, 100, 30]) {
    pointer("pointermove", 827, y)
    await apart()
  }
  await frames()
  const ghost = host.querySelector<HTMLElement>(`.${classes.ghost}`)
  const highlight = host.querySelector<HTMLElement>(`.${classes.placeholder}`)
  expect(said()).toBe("Move above Session c")
  expect([ghost?.style.width, ghost?.style.height]).toEqual(["280px", "44px"])
  expect([highlight?.style.width, highlight?.style.height]).toEqual(["546px", "400px"])
  expect(host.querySelector(`[${marks.reflow}]`)).toBeNull()
  expect(host.querySelector(`[${marks.pressing}]`)).toBeNull()
  for (const pane of host.querySelectorAll<HTMLElement>("[data-pane-key]")) {
    expect([pane.style.transform, pane.style.width, pane.style.height]).toEqual([
      "",
      "",
      "",
    ])
    expect(pane.hasAttribute(marks.dragCorner)).toBe(false)
  }
  expect(store.getState().workspace.panes).toBe(before)
  pointer("pointermove", 1200, 300)
  await frames()
  expect(host.querySelector(`.${classes.placeholder}`)).toBeNull()
  expect([ghost?.style.width, ghost?.style.height]).toEqual(["280px", "44px"])
  for (const y of [400, 300, 200, 100, 30]) {
    pointer("pointermove", 827, y)
    await apart()
  }
  await frames()
  pointer("pointerup", 827, 30)
  await frames()
  expect(shownIn(store.getState().workspace.panes)).toEqual(["a", "c"])
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
    .querySelector(`.${classes.shield}`)
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
    ["an agent's fold of the sidebar", () => void store.dispatch(toggleSidebar())],
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
      return shownIn(store.getState().workspace.panes)
    }
    const was = shownIn(before.panes)
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
  expect(shownIn(after)).toEqual(["c", "a"])
  await act(async () => root.unmount())
})

it("offers no zone while the overview covers the panes: a session carried there drops nowhere", async () => {
  const { store, root } = await mounted()
  store.dispatch(showContent({ content: "agents" }))
  const before = store.getState().workspace.panes
  const row = host.querySelector("[data-drag-item]")
  if (!row) throw new Error("no row")
  await press(10, 10, row)
  pointer("pointermove", 830, 400)
  pointer("pointermove", 827, 400)
  await frames()
  await act(async () => new Promise((resolve) => setTimeout(resolve, restAfter + 30)))
  await frames()
  expect(said()).toBe("")
  expect(host.querySelector(`.${classes.placeholder}`)).toBeNull()
  pointer("pointerup", 827, 400)
  await frames()
  expect(store.getState().workspace.panes).toBe(before)
  nothingLeft()
  await act(async () => root.unmount())
})

it("removes a returning copy immediately when the room changes after pointer loss", async () => {
  const { root } = await mounted()
  await press(60, 16, header(1))
  pointer("pointermove", 830, 400)
  pointer("pointermove", 827, 400)
  await frames()
  let finish!: () => void
  const finished = new Promise<void>((resolve) => {
    finish = resolve
  })
  Element.prototype.animate = function () {
    return { cancel() {}, finished, id: "" } as unknown as Animation
  }
  window.dispatchEvent(new Event("blur"))
  expect(carrier()).not.toBeNull()
  window.dispatchEvent(new Event("resize"))
  expect(carrier()).toBeNull()
  finish()
  await frames()
  await act(async () => root.unmount())
})

it("an obsolete return settling leaves the next drag's copy and preview owned", async () => {
  const { root } = await mounted()
  await press(60, 16, header(1))
  pointer("pointermove", 830, 400)
  pointer("pointermove", 827, 400)
  await frames()
  const animate = Element.prototype.animate
  let finish!: () => void
  const finished = new Promise<void>((resolve) => {
    finish = resolve
  })
  Element.prototype.animate = () =>
    ({ cancel() {}, finished, id: "" }) as unknown as Animation
  window.dispatchEvent(new Event("blur"))
  window.dispatchEvent(new Event("resize"))
  expect(carrier()).toBeNull()
  const cancellations: ReturnType<typeof vi.fn>[] = []
  Element.prototype.animate = function (keyframes, options) {
    const animation = animate.call(this, keyframes, options)
    const cancel = vi.fn()
    cancellations.push(cancel)
    return { ...animation, cancel } as unknown as Animation
  }
  await press(60, 16, header(1))
  pointer("pointermove", 830, 400)
  pointer("pointermove", 827, 400)
  await frames()
  const held = carrier()
  expect(held).not.toBeNull()
  const counts = cancellations.map((cancel) => cancel.mock.calls.length)
  finish()
  await frames()
  expect(carrier()).toBe(held)
  expect(cancellations.map((cancel) => cancel.mock.calls.length)).toEqual(counts)
  pointer("pointerup", 827, 400)
  await frames()
  await act(async () => root.unmount())
})

for (const ending of ["return", "drop"] as const)
  it(`unmount releases the retained ${ending} flight before later settlement effects`, async () => {
    const { root } = await mounted()
    await press(60, 16, header(1))
    pointer("pointermove", 830, 400)
    pointer("pointermove", 827, 400)
    await frames()
    let finish!: () => void
    const finished = new Promise<void>((resolve) => {
      finish = resolve
    })
    const cancel = vi.fn()
    const animate = vi.fn(() => ({ cancel, finished, id: "" }) as unknown as Animation)
    Element.prototype.animate = animate
    if (ending === "return") window.dispatchEvent(new Event("blur"))
    else pointer("pointerup", 827, 400)
    expect(carrier()).not.toBeNull()
    await act(async () => root.unmount())
    expect(cancel).toHaveBeenCalled()
    const calls = animate.mock.calls.length
    finish()
    await frames()
    expect(animate).toHaveBeenCalledTimes(calls)
  })
