// @vitest-environment jsdom
/**
 * A slide inside a slide moves only by its own difference — measured where
 * everything landed before any slide starts. A browser that draws a newly
 * begun slide at once (WebKit) would otherwise read the carried element back
 * at its column's old place and send it twice as far: a column title then
 * swept under the window's controls.
 */
import { act, useRef, useState } from "react"
import { createRoot } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { FlipScope, flyPane } from "./flip"
import { marks } from "./marks"

let host: HTMLDivElement
/** Each slide begun: the element and the shift it starts from. */
let begun: { id: string; shift: number }[]
/** What a begun slide adds to where each element is drawn, as WebKit draws it at once. */
const drawnShift = new Map<Element, number>()
const animations = new Map<
  string,
  { frames: Keyframe[]; timing: KeyframeAnimationOptions }
>()

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  begun = []
  drawnShift.clear()
  animations.clear()
  Element.prototype.animate = function (this: Element, keyframes, options) {
    animations.set((this as HTMLElement).dataset.flipId ?? "", {
      frames: keyframes as Keyframe[],
      timing: typeof options === "number" ? { duration: options } : (options ?? {}),
    })
    const first = (keyframes as Keyframe[])[0]
    const shift = Number(/translateX\((-?[\d.]+)px\)/.exec(String(first.transform))?.[1])
    begun.push({ id: (this as HTMLElement).dataset.flipId ?? "", shift })
    drawnShift.set(this, shift)
    return {
      cancel() {},
      finished: new Promise(() => {}),
      id: "",
    } as unknown as Animation
  }
  Element.prototype.getBoundingClientRect = function (this: Element) {
    // Where the page lays it out now: what the render said, as `data-left`.
    let left =
      Number((this as HTMLElement).dataset.left ?? 0) + (drawnShift.get(this) ?? 0)
    for (let at = this.parentElement; at; at = at.parentElement)
      left += drawnShift.get(at) ?? 0
    return { left, top: 0, width: 100, height: 20 } as DOMRect
  }
  // jsdom resolves no custom properties: the motion tokens are read as the stylesheet sets them.
  globalThis.getComputedStyle = () =>
    ({
      getPropertyValue: (name: string) =>
        name === "--desktop-slow" || name === "--desktop-flight" ? "300ms" : "",
    }) as CSSStyleDeclaration
  host = document.createElement("div")
  document.body.append(host)
})

afterEach(() => host.remove())

function Columns({ open, inert = false }: { open: boolean; inert?: boolean }) {
  const root = useRef<HTMLDivElement>(null)
  return (
    <FlipScope shape={String(open)} root={root}>
      <div ref={root} inert={inert}>
        {/* The sidebar folds: the list moves 216px left, its title inside it 23px. */}
        <div data-flip="slide" data-flip-id="list" data-left={open ? 216 : 0}>
          <div
            data-flip="slide"
            data-flip-id="column-title"
            data-left={open ? 228 : 205}
          />
        </div>
      </div>
    </FlipScope>
  )
}

it("slides a carried element by its own difference, wherever the browser draws a slide begun", async () => {
  let toggle: () => void = () => {}
  function Window() {
    const [open, setOpen] = useState(true)
    toggle = () => setOpen(false)
    return <Columns open={open} />
  }
  const root = createRoot(host)
  await act(async () => root.render(<Window />))
  await act(async () => toggle())
  expect(begun).toEqual([
    { id: "list", shift: 216 },
    // 23px of its own, not the 216 it is carried as well.
    { id: "column-title", shift: 23 - 216 },
  ])
  await act(async () => root.unmount())
})

it("leaves inert folding columns to their existing transition owner", async () => {
  const root = createRoot(host)
  try {
    await act(async () => root.render(<Columns open inert />))
    await act(async () => root.render(<Columns open={false} inert />))
    expect(begun).toEqual([])
  } finally {
    await act(async () => root.unmount())
  }
})

it("reads where every pane landed before any flight starts, so the page lays out once", async () => {
  const log: ("read" | "write")[] = []
  const animate = Element.prototype.animate
  Element.prototype.animate = function (this: Element, ...args) {
    log.push("write")
    return animate.apply(this, args)
  }
  const rect = Element.prototype.getBoundingClientRect
  Element.prototype.getBoundingClientRect = function (this: Element) {
    if (this.hasAttribute("data-pane")) log.push("read")
    return rect.call(this)
  }
  const offsets = ["offsetLeft", "offsetTop"] as const
  const descriptors = offsets.map((name) =>
    Object.getOwnPropertyDescriptor(HTMLElement.prototype, name),
  )
  for (const name of offsets)
    Object.defineProperty(HTMLElement.prototype, name, {
      configurable: true,
      get(this: HTMLElement) {
        if (this.parentElement?.hasAttribute("data-pane")) log.push("read")
        return 0
      },
    })
  let swap: () => void = () => {}
  function Panes() {
    const root = useRef<HTMLDivElement>(null)
    const [swapped, setSwapped] = useState(false)
    swap = () => setSwapped(true)
    return (
      <FlipScope shape={String(swapped)} root={root}>
        <div ref={root}>
          {["a", "b"].map((id, index) => (
            <article
              key={id}
              data-pane
              data-flip="pane"
              data-flip-id={id}
              data-left={(swapped ? 1 - index : index) * 500}
            >
              <header className="workspace-pane-header" data-split-keeps="top-left" />
              <div className="workspace-pane-body" />
            </article>
          ))}
        </div>
      </FlipScope>
    )
  }
  const root = createRoot(host)
  await act(async () => root.render(<Panes />))
  log.length = 0
  await act(async () => swap())
  expect(log).toContain("write")
  expect(log.join(" ")).toMatch(/^(read )+(write ?)+$/)
  Element.prototype.animate = animate
  Element.prototype.getBoundingClientRect = rect
  offsets.forEach((name, index) => {
    const descriptor = descriptors[index]
    if (descriptor) Object.defineProperty(HTMLElement.prototype, name, descriptor)
  })
  await act(async () => root.unmount())
})

it("reads counter-scale offsets only for panes that move", async () => {
  const offsetTop = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "offsetTop")
  const read: string[] = []
  Object.defineProperty(HTMLElement.prototype, "offsetTop", {
    configurable: true,
    get(this: HTMLElement) {
      const id = this.parentElement?.dataset.flipId
      if (id) read.push(id)
      return 0
    },
  })
  let change = () => {}
  function Panes() {
    const root = useRef<HTMLDivElement>(null)
    const [changed, setChanged] = useState(false)
    change = () => setChanged(true)
    return (
      <FlipScope shape={String(changed)} root={root}>
        <div ref={root}>
          <article data-flip="pane" data-flip-id="still" data-left={0}>
            <div />
          </article>
          <article data-flip="pane" data-flip-id="moving" data-left={changed ? 200 : 100}>
            <div />
          </article>
        </div>
      </FlipScope>
    )
  }
  const root = createRoot(host)
  try {
    await act(async () => root.render(<Panes />))
    read.length = 0
    await act(async () => change())
    expect(read).toEqual(["moving"])
  } finally {
    await act(async () => root.unmount())
    if (offsetTop) Object.defineProperty(HTMLElement.prototype, "offsetTop", offsetTop)
  }
})

it("does not resolve flight styles after a shape update that moves nothing", async () => {
  const style = globalThis.getComputedStyle
  let after = 0
  globalThis.getComputedStyle = (element) => {
    if ((element as HTMLElement).dataset.changed === "true") after++
    return style(element)
  }
  let change = () => {}
  function Panes() {
    const root = useRef<HTMLDivElement>(null)
    const [changed, setChanged] = useState(false)
    change = () => setChanged(true)
    return (
      <FlipScope shape={String(changed)} root={root}>
        <div ref={root} data-changed={String(changed)}>
          <article data-flip="pane" data-flip-id="still" data-left={0}>
            <div />
          </article>
        </div>
      </FlipScope>
    )
  }
  const root = createRoot(host)
  try {
    await act(async () => root.render(<Panes />))
    await act(async () => change())
    expect(after).toBe(0)
    expect(begun).toEqual([])
  } finally {
    await act(async () => root.unmount())
    globalThis.getComputedStyle = style
  }
})

it("retires interrupted flights without delayed body restoration, including a replaced root", async () => {
  const finish: (() => void)[] = []
  const cancel = vi.fn()
  Element.prototype.animate = () =>
    ({
      cancel,
      finished: new Promise<void>((resolve) => finish.push(resolve)),
    }) as unknown as Animation
  function Panes({ phase, rebound = false }: { phase: number; rebound?: boolean }) {
    const scope = useRef<HTMLDivElement>(null)
    return (
      <FlipScope shape={String(phase)} root={scope}>
        <div key={String(rebound)} ref={scope}>
          {["a", "b"].map((id, index) => (
            <article
              key={id}
              data-pane-key={id}
              data-flip="pane"
              data-flip-id={id}
              data-left={((index + phase) % 2) * 100}
            >
              <div>Conversation stays mounted</div>
            </article>
          ))}
        </div>
      </FlipScope>
    )
  }
  const root = createRoot(host)
  try {
    await act(async () => root.render(<Panes phase={0} />))
    const body = host.querySelector("article > div")
    await act(async () => root.render(<Panes phase={1} />))
    const firstEnd = finish.length
    await act(async () => root.render(<Panes phase={2} />))
    expect(host.querySelector("article > div")).toBe(body)
    await act(async () => finish.slice(0, firstEnd).forEach((resolve) => resolve()))
    expect(host.firstElementChild?.hasAttribute(marks.flipping)).toBe(true)
    await act(async () => finish.slice(firstEnd).forEach((resolve) => resolve()))
    expect(host.firstElementChild?.hasAttribute(marks.flipping)).toBe(false)
    expect(host.querySelector(`[${marks.flying}]`)).toBeNull()
    await act(async () => root.render(<Panes phase={3} />))
    const beforeRebind = finish.length
    const old = host.firstElementChild
    if (!old) throw new Error("missing scope")
    await act(async () => root.render(<Panes phase={4} rebound />))
    expect(finish).toHaveLength(beforeRebind)
    expect(old.hasAttribute(marks.flipping)).toBe(false)
    await act(async () => finish.forEach((resolve) => resolve()))
    expect(host.firstElementChild?.hasAttribute(marks.flipping)).toBe(false)
    expect(cancel).toHaveBeenCalled()
  } finally {
    await act(async () => root.unmount())
  }
})

it("resizes a pane and its header by transform without fading its content", () => {
  const pane = document.createElement("article")
  const header = document.createElement("header")
  header.setAttribute(marks.keeps, "top-left")
  pane.append(header)
  const frames: Keyframe[][] = []
  Element.prototype.animate = (_keyframes) => {
    frames.push(_keyframes as Keyframe[])
    return { cancel() {}, finished: new Promise(() => {}) } as unknown as Animation
  }
  flyPane(
    {
      pane,
      to: { left: 0, top: 0, width: 500, height: 400 } as DOMRect,
      children: [{ element: header, left: 0, top: 0, slide: 0 }],
    },
    { left: 100, top: 0, width: 800, height: 400 } as DOMRect,
    300,
  )
  expect(frames).toHaveLength(2)
  expect(
    frames.every((animation) => animation.every((frame) => frame.opacity === undefined)),
  ).toBe(true)
})

it.each([
  { inert: false, opening: true },
  { inert: true, opening: true },
  { inert: false, opening: false },
  { inert: true, opening: false },
])(
  "keeps a carried content origin between its endpoints ($inert inert, $opening opening)",
  async ({ inert, opening }) => {
    const computed = globalThis.getComputedStyle
    globalThis.getComputedStyle = () =>
      ({
        getPropertyValue: (name: string) =>
          name === "--desktop-flight"
            ? "220ms"
            : name === "--desktop-slow"
              ? "240ms"
              : "",
      }) as CSSStyleDeclaration
    let toggle = () => {}
    Element.prototype.getBoundingClientRect = function (this: Element) {
      const element = this as HTMLElement
      return {
        left: Number(element.dataset.left ?? 0),
        top: 0,
        width: Number(element.dataset.width ?? 100),
        height: 400,
      } as DOMRect
    }
    function Window() {
      const scope = useRef<HTMLDivElement>(null)
      const [open, setOpen] = useState(!opening)
      toggle = () => setOpen(opening)
      return (
        <FlipScope shape={String(open)} root={scope}>
          <div ref={scope} inert={inert}>
            <article
              data-flip="pane"
              data-flip-id="pane"
              data-left={open ? 320 : 8}
              data-width={open ? 1120 : 1432}
            >
              <div data-flip="slide" data-flip-id="sliver" data-left={open ? 332 : 205} />
              <header>
                <div data-flip="slide" data-flip-id="name" data-left={open ? 332 : 205} />
              </header>
            </article>
          </div>
        </FlipScope>
      )
    }
    const root = createRoot(host)
    try {
      await act(async () => root.render(<Window />))
      await act(async () => toggle())
      // Parent centre:724→880 (-156). Name:205→332 (-127), so its own shift is29.
      expect(begun.find(({ id }) => id === "name")?.shift).toBe(opening ? 29 : -29)
      const parent = animations.get("pane")
      const name = animations.get("name")
      const sliver = animations.get("sliver")
      expect(begun.filter(({ id }) => id === "sliver")).toHaveLength(1)
      expect(sliver?.timing).toEqual(parent?.timing)
      expect(name?.timing).toMatchObject({
        duration: parent?.timing.duration,
        easing: "linear",
      })
      expect(name?.frames.length).toBe(parent?.frames.length)
      for (const [index, frame] of parent?.frames.entries() ?? []) {
        const carried = Number(
          /translate\(([-\d.]+)px/.exec(String(frame.transform))?.[1],
        )
        const own = Number(
          /translateX\(([-\d.]+)px/.exec(String(name?.frames[index]?.transform))?.[1],
        )
        const actualLeft = (opening ? 332 : 205) + carried + own
        expect(actualLeft).toBeGreaterThanOrEqual(205)
        expect(actualLeft).toBeLessThanOrEqual(332)
        const scale = Number(/scale\(([-\d.]+),/.exec(String(frame.transform))?.[1])
        const sliverShift = Number(
          /translateX\(([-\d.]+)px\)/.exec(
            String(sliver?.frames[index]?.transform),
          )?.[1] ?? 0,
        )
        expect((opening ? 332 : 205) + carried + sliverShift * scale).toBeCloseTo(
          actualLeft,
        )
      }
    } finally {
      globalThis.getComputedStyle = computed
      await act(async () => root.unmount())
    }
  },
)

it("counter-scales an inset child's measured slide about the pane centre without fading", () => {
  const pane = document.createElement("article")
  const sliver = document.createElement("div")
  pane.append(sliver)
  const frames: Keyframe[][] = []
  Element.prototype.animate = (keyframes) => {
    frames.push(keyframes as Keyframe[])
    return { cancel() {}, finished: new Promise(() => {}) } as unknown as Animation
  }
  flyPane(
    {
      pane,
      to: { left: 320, top: 0, width: 1120, height: 400 } as DOMRect,
      children: [{ element: sliver, left: 150, top: 20, slide: 41 }],
    },
    { left: 8, top: 0, width: 1432, height: 400 } as DOMRect,
    220,
  )
  expect(sliver.style.transformOrigin).toBe("410px 180px")
  const shift = Number(
    /translateX\(([-\d.]+)px\)/.exec(String(frames[1]?.[0]?.transform))?.[1],
  )
  expect(shift * (1432 / 1120)).toBeCloseTo(41)
  expect(String(frames[1]?.at(-1)?.transform)).toContain("translateX(0px)")
  expect(frames.flat().every((frame) => frame.opacity === undefined)).toBe(true)
})
