// @vitest-environment jsdom
/**
 * The drag on a host of its own: a fake source that keeps the layout in a
 * variable and the least page a host must give — a grid, panes keyed by the
 * frame, a row to pick an item up from. What the module asks of a host
 * (`SplitPanesSource`, `SplitPanesDragOptions`, the `data-split-*` marks) is
 * each held here; the workspace's own wiring of it is tested beside the
 * workspace (`workspace/adapters/dom/split-panes-drag.test.tsx`).
 */
import { act, useRef, useSyncExternalStore, type RefObject } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import type { Drop, SplitPanesSource } from "../../application/ports"
import { dropOutcome } from "../../model/drop"
import { panesOf, singlePane, splitPane, type PaneLayout } from "../../model/pane-layout"
import type { PaneRoom } from "../../model/pane-sizing"
import { saying, useSplitPanesDrag, type SplitPanesDragOptions } from "./drag"
import { FlipScope } from "./flip"
import { classes, gridOf, marks } from "./marks"

let host: HTMLDivElement
/** Every animation asked for, of what, and whether it was let go. */
const animated: {
  element: Element
  keyframes: Keyframe[]
  duration: unknown
  cancelled: boolean
}[] = []

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  // jsdom neither animates nor lays out: animations end at once, boxes are set by hand.
  const finished = Promise.resolve()
  animated.length = 0
  Element.prototype.animate = function (this: Element, keyframes, timing) {
    const asked = {
      element: this,
      keyframes: keyframes as Keyframe[],
      duration: typeof timing === "object" ? timing.duration : timing,
      cancelled: false,
    }
    animated.push(asked)
    return {
      cancel() {
        asked.cancelled = true
      },
      finished,
      id: "",
    } as unknown as Animation
  }
  Element.prototype.getAnimations = () => []
  host = document.createElement("div")
  document.body.append(host)
})

afterEach(() => host.remove())

/**
 * A host's source over a layout it keeps itself, applying a drop as the drag
 * previews it. Each answer can be what a host may give: no room measured
 * (`room: undefined`), no layout (`change({ layout: null })`).
 */
function fakeSource(
  start: PaneLayout,
  { room = { width: 1100, height: 800, spare: 0 } }: { room?: PaneRoom | null } = {},
) {
  let layout: PaneLayout | null = start
  const listeners = new Set<() => void>()
  const changed = () => listeners.forEach((listener) => listener())
  const state = {
    watched: [] as unknown[],
    items: new Set(["x"]),
    targetable: true,
    measured: 0,
    drops: [] as Drop[],
    subscribed: 0,
  }
  const source: SplitPanesSource = {
    layout: () => layout,
    subscribe: (onChange) => {
      state.subscribed++
      listeners.add(onChange)
      return () => {
        state.subscribed--
        listeners.delete(onChange)
      }
    },
    watched: () => state.watched,
    holds: (item) => state.items.has(item),
    targetable: () => state.targetable,
    measure: () => {
      state.measured++
      return room ?? undefined
    },
    commitDrop: (drop) => {
      state.drops.push(drop)
      const outcome = layout
        ? dropOutcome(layout, drop.carried, drop.target, drop.zone, drop.room)
        : null
      if (outcome) layout = outcome.layout
      changed()
    },
    resize: () => {},
    equalize: () => {},
    fit: () => {},
  }
  return {
    source,
    state,
    /** A change the host makes, told to whoever listens. */
    change: (next: Partial<{ layout: PaneLayout | null; watched: unknown[] }>) => {
      if (next.layout !== undefined) layout = next.layout
      if (next.watched) state.watched = next.watched
      changed()
    },
    layout: () => layout,
  }
}

type Fake = ReturnType<typeof fakeSource>

function Host({ fake, options }: { fake: Fake; options: SplitPanesDragOptions }) {
  const root = useRef<HTMLDivElement>(null)
  useSplitPanesDrag(root, fake.source, options)
  const layout = fake.layout()
  return <Page root={root} layout={layout} />
}

/**
 * A host that draws the layout as the source changes, in a `FlipScope` as
 * the workspace's grid is — jsdom resolves no motion tokens, so it flies
 * nothing, as with less motion.
 */
function FlippingHost({ fake, options }: { fake: Fake; options: SplitPanesDragOptions }) {
  const root = useRef<HTMLDivElement>(null)
  useSplitPanesDrag(root, fake.source, options)
  const layout = useSyncExternalStore(fake.source.subscribe, fake.layout)
  return (
    <FlipScope shape={JSON.stringify(layout?.columns)} root={root}>
      <Page root={root} layout={layout} />
    </FlipScope>
  )
}

function Page({
  root,
  layout,
}: {
  root: RefObject<HTMLDivElement | null>
  layout: PaneLayout | null
}) {
  return (
    <div ref={root}>
      <div className="cover" data-drag-item="x">
        Item x
      </div>
      <div data-split-grid>
        {(layout ? panesOf(layout) : []).map((pane) => (
          <article
            key={pane.key}
            data-pane-key={pane.key}
            aria-label={`Pane ${pane.item}`}
            // The top-left pane, as the grid's frame marks it.
            {...{ [marks.corner]: pane.key === 1 || undefined }}
          >
            <header data-split-keeps="top-left" data-drag-pane={pane.key} data-host-mark>
              {pane.item}
            </header>
            <div data-split-through>
              <div data-split-scroll>
                <div>
                  <p>{`${pane.item} words`}</p>
                </div>
              </div>
              <footer data-split-keeps="foot">composer</footer>
            </div>
          </article>
        ))}
      </div>
    </div>
  )
}

const options = (
  overrides: Partial<SplitPanesDragOptions> = {},
): SplitPanesDragOptions => ({
  copyOf: (item) => {
    const copy = document.createElement("article")
    copy.textContent = `copy of ${item}`
    return copy
  },
  covered: () => [],
  ...overrides,
})

/** Two panes, a and b, side by side in an 1100 × 800 grid, laid out by hand. */
async function mounted(
  fake: Fake,
  opts: SplitPanesDragOptions = options(),
  Drawn: typeof Host = Host,
) {
  const root = createRoot(host)
  await act(async () => root.render(<Drawn fake={fake} options={opts} />))
  layOut()
  return root
}

function layOut() {
  const grid = gridOf(host)
  if (grid) grid.getBoundingClientRect = () => new DOMRect(0, 0, 1100, 800)
  host.querySelectorAll<HTMLElement>("[data-pane-key]").forEach((pane, index) => {
    pane.getBoundingClientRect = () => new DOMRect(index * 554, 0, 546, 800)
  })
}

const two = () => splitPane(singlePane("a"), 1, "right", "b")

const painted = () =>
  act(
    async () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => setTimeout(() => resolve(), 1)),
      ),
  )
const frames = () =>
  act(
    async () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() =>
          requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
        ),
      ),
  )
const oneFrame = () =>
  act(async () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())))
const pointer = (type: string, x: number, y: number, target: EventTarget = window) =>
  target.dispatchEvent(
    new PointerEvent(type, {
      clientX: x,
      clientY: y,
      button: 0,
      buttons: type === "pointerup" ? 0 : 1,
      bubbles: true,
      cancelable: true,
    }),
  )
const press = async (x: number, y: number, target: Element) => {
  pointer("pointerdown", x, y, target)
  await painted()
}
const element = (selector: string) => {
  const found = host.querySelector<HTMLElement>(selector)
  if (!found) throw new Error(`no ${selector}`)
  return found
}
const said = () => host.querySelector('[role="status"]')?.textContent
const carrying = () => host.querySelector(`[${marks.carrying}]`) !== null
/** Pane 1 lifted and held over the middle of pane 2: a swap, shown. */
const liftOntoTwo = async () => {
  await press(60, 16, element('[data-drag-pane="1"]'))
  pointer("pointermove", 90, 40)
  pointer("pointermove", 827, 400)
  await frames()
}
const items = (fake: Fake) => {
  const layout = fake.layout()
  return layout ? panesOf(layout).map((pane) => pane.item) : []
}

it("puts a start WebKit moved on ready back to the instant the pair was given", async () => {
  const held: { startTime: number | null; playState: string }[] = []
  const release: Array<() => void> = []
  Element.prototype.animate = function (this: Element, keyframes, timing) {
    const asked = {
      element: this,
      keyframes: keyframes as Keyframe[],
      duration: typeof timing === "object" ? timing.duration : timing,
      cancelled: false,
    }
    animated.push(asked)
    const animation = {
      cancel() {
        asked.cancelled = true
      },
      finished: Promise.resolve(),
      id: "",
      playState: "running",
      startTime: null as number | null,
      ready: null as Promise<unknown> | null,
    }
    animation.ready = new Promise<void>((resolve) => {
      release.push(() => {
        animation.startTime = 99999
        resolve()
      })
    })
    held.push(animation)
    return animation as unknown as Animation
  }
  Object.defineProperty(document, "timeline", {
    configurable: true,
    value: { currentTime: 1234 },
  })
  try {
    const fake = fakeSource(two())
    const root = await mounted(fake)
    await liftOntoTwo()
    // Only the animations a pair was started together: the rest are not given a start.
    const paired = held.filter((animation) => animation.startTime === 1234)
    expect(paired.length).toBeGreaterThan(1)
    for (const fulfill of release) fulfill()
    expect(paired.every((animation) => animation.startTime === 99999)).toBe(true)
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    expect(paired.every((animation) => animation.startTime === 1234)).toBe(true)
    await act(async () => root.unmount())
  } finally {
    Reflect.deleteProperty(document, "timeline")
  }
})

it("treats a cancelled preview's ready rejection as letting go", async () => {
  const unhandled: unknown[] = []
  const reported: unknown[] = []
  const previous = globalThis.reportError
  globalThis.reportError = (error: unknown) => {
    reported.push(error)
  }
  const onUnhandled = (reason: unknown) => {
    unhandled.push(reason)
  }
  process.on("unhandledRejection", onUnhandled)
  const animations: { startTime: number | null; cancelReady: () => void }[] = []
  Element.prototype.animate = function (this: Element, keyframes, timing) {
    const asked = {
      element: this,
      keyframes: keyframes as Keyframe[],
      duration: typeof timing === "object" ? timing.duration : timing,
      cancelled: false,
    }
    animated.push(asked)
    const animation = {
      cancel() {
        asked.cancelled = true
      },
      finished: Promise.resolve(),
      id: "",
      playState: "pending",
      startTime: null as number | null,
      ready: null as Promise<unknown> | null,
      cancelReady: () => {},
    }
    animation.ready = new Promise<void>((_resolve, reject) => {
      animation.cancelReady = () => {
        animation.playState = "idle"
        reject(new DOMException("cancelled", "AbortError"))
      }
    })
    animations.push(animation)
    return animation as unknown as Animation
  }
  Object.defineProperty(document, "timeline", {
    configurable: true,
    value: { currentTime: 1234 },
  })
  try {
    const root = await mounted(fakeSource(two()))
    await liftOntoTwo()
    const paired = animations.filter((animation) => animation.startTime === 1234)
    expect(paired.length).toBeGreaterThan(0)
    await act(async () => {
      for (const animation of paired) animation.cancelReady()
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    expect(unhandled).toEqual([])
    expect(reported).toEqual([])
    await act(async () => root.unmount())
  } finally {
    process.removeListener("unhandledRejection", onUnhandled)
    if (previous === undefined) Reflect.deleteProperty(globalThis, "reportError")
    else globalThis.reportError = previous
    Reflect.deleteProperty(document, "timeline")
  }
})

it("keeps a ready failure that is not cancellation", async () => {
  const reported: unknown[] = []
  const previous = globalThis.reportError
  globalThis.reportError = (error: unknown) => {
    reported.push(error)
  }
  const animations: { startTime: number | null; failReady: () => void }[] = []
  Element.prototype.animate = function (this: Element, keyframes, timing) {
    const asked = {
      element: this,
      keyframes: keyframes as Keyframe[],
      duration: typeof timing === "object" ? timing.duration : timing,
      cancelled: false,
    }
    animated.push(asked)
    const animation = {
      cancel() {
        asked.cancelled = true
      },
      finished: Promise.resolve(),
      id: "",
      playState: "pending",
      startTime: null as number | null,
      ready: null as Promise<unknown> | null,
      failReady: () => {},
    }
    animation.ready = new Promise<void>((_resolve, reject) => {
      animation.failReady = () => reject(new Error("clock failed"))
    })
    animations.push(animation)
    return animation as unknown as Animation
  }
  Object.defineProperty(document, "timeline", {
    configurable: true,
    value: { currentTime: 1234 },
  })
  try {
    const root = await mounted(fakeSource(two()))
    await liftOntoTwo()
    const paired = animations.filter((animation) => animation.startTime === 1234)
    expect(paired.length).toBeGreaterThan(0)
    await act(async () => {
      paired[0]?.failReady()
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    expect(reported.map((error) => (error as Error).message)).toContain("clock failed")
    await act(async () => root.unmount())
  } finally {
    if (previous === undefined) Reflect.deleteProperty(globalThis, "reportError")
    else globalThis.reportError = previous
    Reflect.deleteProperty(document, "timeline")
  }
})

it("previews the outcome of the layout the source holds, and commits it through the source in the room the press read", async () => {
  const fake = fakeSource(two())
  const root = await mounted(fake)
  await liftOntoTwo()
  expect(said()).toBe("Swap with Pane b")
  pointer("pointerup", 827, 400)
  // The pointerup turn flies the copy and does not commit: the commit lays
  // the new arrangement out, and that layout is the next frame's.
  expect(fake.state.drops).toEqual([])
  await frames()
  expect(fake.state.drops).toEqual([
    {
      carried: { kind: "pane", pane: 1 },
      target: 2,
      zone: "center",
      room: { width: 1100, height: 800, spare: 0 },
    },
  ])
  // Measured once, as the press began.
  expect(fake.state.measured).toBe(1)
  expect(items(fake)).toEqual(["b", "a"])
  // Bodies were held out of the commit's layout, then brought back.
  expect(host.querySelector("[data-drag-settling]")).toBeNull()
  expect(carrying()).toBe(false)
  await act(async () => root.unmount())
})

it("holds pane bodies out of the frame a cancel lets the preview go, and brings one back each frame after", async () => {
  const fake = fakeSource(two())
  const root = await mounted(fake)
  await liftOntoTwo()
  expect(document.documentElement.hasAttribute(marks.reflow)).toBe(true)
  expect(document.documentElement.hasAttribute(marks.pressing)).toBe(true)
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  pointer("pointerup", 827, 400)
  // The preview's mark drops as the copy flies home, and the bodies are
  // waiting on that turn — one comes back on the frame after. The glass
  // stays off until the frame after the last of them.
  await act(async () => {})
  expect(document.documentElement.hasAttribute(marks.reflow)).toBe(false)
  // The style frame's layers stay for this turn and drop on the next.
  expect(document.documentElement.hasAttribute(marks.promoted)).toBe(true)
  expect(host.querySelectorAll("[data-drag-settling]").length).toBeGreaterThan(1)
  expect(document.documentElement.hasAttribute(marks.pressing)).toBe(true)
  await oneFrame()
  expect(document.documentElement.hasAttribute(marks.promoted)).toBe(false)
  expect(host.querySelectorAll("[data-drag-settling]").length).toBe(1)
  expect(document.documentElement.hasAttribute(marks.pressing)).toBe(true)
  await oneFrame()
  expect(host.querySelector("[data-drag-settling]")).toBeNull()
  expect(document.documentElement.hasAttribute(marks.pressing)).toBe(true)
  await oneFrame()
  expect(document.documentElement.hasAttribute(marks.pressing)).toBe(false)
  expect(fake.state.drops).toEqual([])
  await act(async () => root.unmount())
})

it("applies a preview's styles the frame before it promotes the panes and moves them", async () => {
  const fake = fakeSource(two())
  const root = await mounted(fake)
  await press(60, 16, element('[data-drag-pane="1"]'))
  pointer("pointermove", 90, 40)
  pointer("pointermove", 827, 400)
  const paneMoved = () =>
    animated.some(({ element: of }) => of.hasAttribute("data-pane-key"))
  let styled = false
  for (let step = 0; step < 8 && !styled; step++) {
    await oneFrame()
    styled = document.documentElement.hasAttribute(marks.reflow)
  }
  expect(styled).toBe(true)
  expect(document.documentElement.hasAttribute(marks.promoted)).toBe(false)
  expect(paneMoved()).toBe(false)
  await oneFrame()
  expect(document.documentElement.hasAttribute(marks.promoted)).toBe(true)
  expect(paneMoved()).toBe(true)
  await act(async () => root.unmount())
})

it("holds pane bodies out of the commit frame and brings one back each frame after", async () => {
  const fake = fakeSource(two())
  const root = await mounted(fake)
  await liftOntoTwo()
  pointer("pointerup", 827, 400)
  await act(
    async () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())),
  )
  expect(host.querySelectorAll("[data-drag-settling]").length).toBeGreaterThan(0)
  await frames()
  expect(host.querySelector("[data-drag-settling]")).toBeNull()
  expect(fake.state.drops).toHaveLength(1)
  // The copy is gone on that landing. Fading it would blend it with the blur.
  const handoff = animated.filter(
    (asked) =>
      asked.element.classList.contains(classes.ghost) &&
      asked.keyframes.some((frame) => frame.opacity === 0),
  )
  expect(handoff.at(-1)?.duration).toBe(0)
  await act(async () => root.unmount())
})

it("does not commit a drop whose preview no longer holds when its frame runs", async () => {
  const fake = fakeSource(two())
  const root = await mounted(fake)
  await liftOntoTwo()
  pointer("pointerup", 827, 400)
  expect(fake.state.drops).toEqual([])
  const layout = fake.layout()
  if (!layout) throw new Error("no layout")
  // A pane opens in the frame the commit waits: that is not the swap shown.
  fake.change({ layout: splitPane(layout, 2, "bottom", "c") })
  await frames()
  expect(fake.state.drops).toEqual([])
  expect(items(fake)).toEqual(["a", "b", "c"])
  expect(fake.state.subscribed).toBe(1)
  await act(async () => root.unmount())
})

it("does not commit a drop after a resize in the frame it waits, and still commits when a watched value is unchanged", async () => {
  const kept = { open: true }
  const resized = fakeSource(two())
  const resizedRoot = await mounted(resized)
  await liftOntoTwo()
  pointer("pointerup", 827, 400)
  window.dispatchEvent(new Event("resize"))
  await frames()
  expect(resized.state.drops).toEqual([])
  expect(items(resized)).toEqual(["a", "b"])
  await act(async () => resizedRoot.unmount())

  const same = fakeSource(two())
  same.state.watched = [kept]
  const sameRoot = await mounted(same)
  await liftOntoTwo()
  pointer("pointerup", 827, 400)
  same.change({ watched: [kept] })
  await frames()
  expect(same.state.drops).toHaveLength(1)
  expect(items(same)).toEqual(["b", "a"])
  await act(async () => sameRoot.unmount())
})

it("does not commit a flick released before the target preview was shown", async () => {
  const fake = fakeSource(two())
  const root = await mounted(fake)
  const header = element('[data-drag-pane="1"]')
  const button = document.createElement("button")
  header.append(button)
  const onButton = new PointerEvent("pointerdown", {
    clientX: 60,
    clientY: 16,
    button: 0,
    buttons: 1,
    bubbles: true,
    cancelable: true,
  })
  button.dispatchEvent(onButton)
  expect(onButton.defaultPrevented).toBe(false)
  const down = new PointerEvent("pointerdown", {
    clientX: 60,
    clientY: 16,
    button: 0,
    buttons: 1,
    bubbles: true,
    cancelable: true,
  })
  header.dispatchEvent(down)
  // Cancelled, so the button's press was not taken: a press already held is ignored.
  expect(down.isTrusted).toBe(false)
  expect(down.defaultPrevented).toBe(true)
  await painted()
  // Across and up in this turn. The preview is a frame, and that frame has not run.
  pointer("pointermove", 90, 40)
  pointer("pointermove", 827, 400)
  pointer("pointerup", 827, 400)
  expect(fake.state.drops).toEqual([])
  await frames()
  expect(fake.state.drops).toEqual([])
  expect(items(fake)).toEqual(["a", "b"])
  expect(said() ?? "").toBe("")
  // The same release, once that preview has been shown, commits on the next frame.
  await liftOntoTwo()
  expect(said()).toBe("Swap with Pane b")
  pointer("pointerup", 827, 400)
  expect(fake.state.drops).toEqual([])
  await frames()
  expect(fake.state.drops).toHaveLength(1)
  await act(async () => root.unmount())
})

it("does not commit a drop whose frame was cancelled by unmount", async () => {
  const fake = fakeSource(two())
  const root = await mounted(fake)
  await liftOntoTwo()
  pointer("pointerup", 827, 400)
  await act(async () => root.unmount())
  await frames()
  expect(fake.state.drops).toEqual([])
  expect(fake.state.subscribed).toBe(0)
})

it("with less motion, previews a swap at once — the other pane drawn where the drop puts it — and lets it go as the drop lands", async () => {
  // As the person's window is set: Settings › Appearance › Motion, Reduced (#286).
  document.documentElement.dataset.motion = "reduced"
  try {
    const fake = fakeSource(two())
    const root = await mounted(fake, options(), FlippingHost)
    await liftOntoTwo()
    expect(said()).toBe("Swap with Pane b")
    // The placeholder alone would mark pane b's own rect, under the copy:
    // pane b is drawn in pane a's place, and pane a in b's, with no glide.
    const drawnAt = (key: number) =>
      animated
        .filter(({ element: of }) => of === element(`[data-pane-key="${key}"]`))
        .at(-1)
    expect(String(drawnAt(2)?.keyframes.at(-1)?.transform)).toMatch(
      /^translate\(-554px, 0px\) scale\(1, 1\)$/,
    )
    expect(String(drawnAt(1)?.keyframes.at(-1)?.transform)).toMatch(
      /^translate\(554px, 0px\) scale\(1, 1\)$/,
    )
    expect(drawnAt(2)?.duration).toBe(0)
    // The transcript's clip is a style, set once. Keyframes that animate it
    // lay the transcript out on every frame of the glide.
    expect(
      animated.every(({ keyframes }) =>
        keyframes.every((frame) => frame?.clipPath == null),
      ),
    ).toBe(true)
    // Dropped: the panes are laid out where the preview drew them, and the
    // preview is let go as they are — left on, it would draw them moved again.
    const preview = animated.filter(({ element: of }) => of.closest("[data-pane-key]"))
    pointer("pointerup", 827, 400)
    expect(items(fake)).toEqual(["a", "b"])
    await frames()
    expect(items(fake)).toEqual(["b", "a"])
    expect(preview.filter(({ cancelled }) => !cancelled)).toEqual([])
    await act(async () => root.unmount())
  } finally {
    delete document.documentElement.dataset.motion
  }
})

it("ends at once when a value the source watches changes, and not when it is the same", async () => {
  const kept = { open: true }
  const fake = fakeSource(two())
  fake.state.watched = [kept]
  const root = await mounted(fake)
  await liftOntoTwo()
  // Told of a change, but every watched value the same: carrying on.
  fake.change({ watched: [kept] })
  await frames()
  expect(carrying()).toBe(true)
  // Another value: gone at once, nothing dropped.
  fake.change({ watched: [{ open: true }] })
  await frames()
  expect(carrying()).toBe(false)
  expect(host.querySelector(`[${marks.waiting}], [${marks.lifted}]`)).toBeNull()
  pointer("pointerup", 827, 400)
  await frames()
  expect(fake.state.drops).toEqual([])
  await act(async () => root.unmount())
})

it("ends at once when the panes' arrangement changes under it, but not when only focus moves", async () => {
  const fake = fakeSource(two())
  const root = await mounted(fake)
  await liftOntoTwo()
  const layout = fake.layout()
  if (!layout) throw new Error("no layout")
  fake.change({ layout: { ...layout, focused: 2 } })
  await frames()
  expect(carrying()).toBe(true)
  fake.change({ layout: splitPane(layout, 2, "bottom", "c") })
  await frames()
  expect(carrying()).toBe(false)
  pointer("pointerup", 827, 400)
  await frames()
  expect(fake.state.drops).toEqual([])
  await act(async () => root.unmount())
})

it("makes nothing for a press when the source measures no room, and lifts nothing after", async () => {
  const fake = fakeSource(two(), { room: null })
  const root = await mounted(fake)
  await press(60, 16, element(`[${marks.dragPane}="1"]`))
  // Measured once, as the press's frame painted: nothing to draw a drag in, so the press ends.
  expect(fake.state.measured).toBe(1)
  expect(host.querySelector(`.${classes.ghost}`)).toBeNull()
  // The press is over, not left pending: a selection may start again.
  const selecting = new Event("selectstart", { bubbles: true, cancelable: true })
  element(`[${marks.dragPane}="1"]`).dispatchEvent(selecting)
  expect(selecting.defaultPrevented).toBe(false)
  pointer("pointermove", 90, 40)
  pointer("pointermove", 827, 400)
  await frames()
  expect(carrying()).toBe(false)
  pointer("pointerup", 827, 400)
  await frames()
  expect(fake.state.drops).toEqual([])
  await act(async () => root.unmount())
})

it("ends a press, and a drag, the moment the source has no layout", async () => {
  const fake = fakeSource(two())
  const root = await mounted(fake)
  // Pressed, before its copy is made: the layout goes, the press ends, nothing is made.
  pointer("pointerdown", 60, 16, element(`[${marks.dragPane}="1"]`))
  fake.change({ layout: null })
  await painted()
  expect(host.querySelector(`.${classes.ghost}`)).toBeNull()
  pointer("pointermove", 827, 400)
  await frames()
  expect(carrying()).toBe(false)
  pointer("pointerup", 827, 400)
  await frames()
  // Carrying: the layout goes, the drag ends at once, nothing is dropped.
  fake.change({ layout: two() })
  await liftOntoTwo()
  expect(carrying()).toBe(true)
  fake.change({ layout: null })
  await frames()
  expect(carrying()).toBe(false)
  pointer("pointerup", 827, 400)
  await frames()
  expect(fake.state.drops).toEqual([])
  await act(async () => root.unmount())
})

it("ends at once when the carried item is no longer held", async () => {
  const fake = fakeSource(two())
  const root = await mounted(fake)
  await press(10, 10, element("[data-drag-item]"))
  pointer("pointermove", 40, 40)
  pointer("pointermove", 827, 400)
  await frames()
  expect(carrying()).toBe(true)
  fake.state.items.delete("x")
  fake.change({})
  await frames()
  expect(carrying()).toBe(false)
  await act(async () => root.unmount())
})

it("never aims at what the host covers, whatever is under it", async () => {
  const fake = fakeSource(two())
  const root = await mounted(
    fake,
    options({ covered: (scope) => [...scope.querySelectorAll(".cover")] }),
  )
  // The row the item is picked up from lies over pane b's middle.
  element(".cover").getBoundingClientRect = () => new DOMRect(700, 300, 300, 200)
  await press(710, 310, element("[data-drag-item]"))
  pointer("pointermove", 740, 340)
  pointer("pointermove", 827, 400)
  await frames()
  expect(said()).toBe("")
  // Past it, pane b is a target as ever.
  pointer("pointermove", 827, 600)
  await frames()
  expect(said()).toMatch(/Pane b$/)
  pointer("pointermove", 827, 400)
  await frames()
  expect(said()).toBe("")
  pointer("pointerup", 827, 400)
  await frames()
  expect(fake.state.drops).toEqual([])
  await act(async () => root.unmount())
})

it("offers no zone while the source says no pane can be aimed at", async () => {
  const fake = fakeSource(two())
  fake.state.targetable = false
  const root = await mounted(fake)
  await liftOntoTwo()
  expect(said()).toBe("")
  expect(host.querySelector(`.${classes.placeholder}`)).toBeNull()
  pointer("pointerup", 827, 400)
  await frames()
  expect(fake.state.drops).toEqual([])
  await act(async () => root.unmount())
})

it("marks the root while a drop would take the host's spare room, and clears it when let go", async () => {
  // One pane in a grid too narrow for two: a split fits only with the spare room.
  const fake = fakeSource(singlePane("a"), {
    room: { width: 500, height: 800, spare: 400 },
  })
  const root = await mounted(fake)
  const scope = element("[data-split-grid]").parentElement
  await press(10, 10, element("[data-drag-item]"))
  for (const x of [200, 300, 400, 480]) {
    pointer("pointermove", x, 400)
    await new Promise((resolve) => setTimeout(resolve, 4))
  }
  await frames()
  expect(said()).toBe("Split right of Pane a")
  expect(scope?.hasAttribute(marks.takesSpare)).toBe(true)
  // Over the pane's middle, a replace takes nothing.
  pointer("pointermove", 250, 400)
  await new Promise((resolve) => setTimeout(resolve, 200))
  await frames()
  expect(said()).toBe("Open in place of Pane a")
  expect(scope?.hasAttribute(marks.takesSpare)).toBe(false)
  for (const x of [300, 400, 480]) {
    pointer("pointermove", x, 400)
    await new Promise((resolve) => setTimeout(resolve, 4))
  }
  await frames()
  expect(scope?.hasAttribute(marks.takesSpare)).toBe(true)
  // Let go home: cleared.
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  pointer("pointerup", 480, 400)
  await frames()
  expect(scope?.hasAttribute(marks.takesSpare)).toBe(false)
  // Ended at once by a change: cleared too.
  await press(10, 10, element("[data-drag-item]"))
  for (const x of [200, 300, 400, 480]) {
    pointer("pointermove", x, 400)
    await new Promise((resolve) => setTimeout(resolve, 4))
  }
  await frames()
  expect(scope?.hasAttribute(marks.takesSpare)).toBe(true)
  fake.change({ watched: [{}] })
  await frames()
  expect(scope?.hasAttribute(marks.takesSpare)).toBe(false)
  pointer("pointerup", 480, 400)
  await act(async () => root.unmount())
})

it("carries the host's copy of an item, and a picture of a pane without the host's stripped marks", async () => {
  const fake = fakeSource(two())
  const root = await mounted(fake, options({ stripped: ["data-host-mark"] }))
  await press(10, 10, element("[data-drag-item]"))
  pointer("pointermove", 40, 40)
  expect(element(`.${classes.ghost}`).textContent).toBe("copy of x")
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  pointer("pointerup", 40, 40)
  await frames()
  await liftOntoTwo()
  const copy = element(`.${classes.ghost} header`)
  // A picture of the pane as it looks — in the corner, its header starts
  // where the pane's does — but nothing to carry, aim at, or mid-drag.
  expect(element(`.${classes.ghost} article`).hasAttribute(marks.corner)).toBe(true)
  // Only `dragPane` is on the pane as it is pictured; the others are set
  // after the copy is made, and are listed so that stays true.
  for (const name of [marks.dragPane, marks.dragItem, marks.carrying, marks.lifted])
    expect(element(`.${classes.ghost}`).querySelector(`[${name}]`), name).toBeNull()
  expect(copy.hasAttribute("data-host-mark")).toBe(false)
  expect(copy.hasAttribute("data-drag-pane")).toBe(false)
  // The pane itself keeps them.
  expect(element('[data-pane-key="1"] header').hasAttribute("data-host-mark")).toBe(true)
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  pointer("pointerup", 827, 400)
  await act(async () => root.unmount())
})

it("holds each part of a pane to what it keeps to as a preview reshapes it", async () => {
  const fake = fakeSource(two())
  const root = await mounted(fake)
  await press(60, 16, element('[data-drag-pane="1"]'))
  // Up pane b's middle: above it. Pane b goes below, wider and shorter.
  for (const y of [400, 300, 200, 100, 30]) {
    pointer("pointermove", 827, y)
    await new Promise((resolve) => setTimeout(resolve, 4))
  }
  await frames()
  expect(said()).toBe("Move above Pane b")
  const origin = (selector: string) =>
    element(`[data-pane-key="2"] ${selector}`).style.transformOrigin
  // The header its top left; the scroller, unmarked, the top — centred across as the pane grows;
  // the part marked foot its foot, looked for inside the wrapper marked through.
  expect(origin("[data-split-keeps='top-left']")).toBe("0px 0px")
  expect(origin("[data-split-scroll]")).toBe("273px 0px")
  expect(origin("[data-split-keeps='foot']")).toBe("273px 800px")
  // The wrapper itself is looked inside, never drawn as a part.
  expect(origin("[data-split-through]")).toBe("")
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  pointer("pointerup", 827, 30)
  await act(async () => root.unmount())
})

it("copies one screen of what the host marks as scrolling", async () => {
  const fake = fakeSource(two())
  const root = await mounted(fake)
  const scroller = element('[data-pane-key="1"] [data-split-scroll]')
  Object.defineProperty(scroller, "scrollTop", { value: 120 })
  await liftOntoTwo()
  // The scroller's content in the copy, found by the fixture's own shape: the copy keeps no marks.
  const copied = element(`.${classes.ghost} article > div > div > div`)
  expect(copied.style.transform).toBe("translateY(-120px)")
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  pointer("pointerup", 827, 400)
  await act(async () => root.unmount())
})

it("listens to the source while mounted, and stops when unmounted", async () => {
  const fake = fakeSource(two())
  const root: Root = await mounted(fake)
  expect(fake.state.subscribed).toBe(1)
  await act(async () => root.unmount())
  expect(fake.state.subscribed).toBe(0)
})

it("says each zone as a person would: above and below, left of and right of", () => {
  const outcome = (does: "move" | "split") =>
    ({ does, lands: 1, layout: null, takesSpare: false }) as unknown as Parameters<
      typeof saying
    >[0]
  expect(saying(outcome("split"), "top", "Notes")).toBe("Split above Notes")
  expect(saying(outcome("split"), "bottom", "Notes")).toBe("Split below Notes")
  expect(saying(outcome("split"), "left", "Notes")).toBe("Split left of Notes")
  expect(saying(outcome("move"), "top", "Notes")).toBe("Move above Notes")
  expect(saying(outcome("move"), "right", "Notes")).toBe("Move right of Notes")
})
