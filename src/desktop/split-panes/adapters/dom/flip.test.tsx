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

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  begun = []
  drawnShift.clear()
  Element.prototype.animate = function (this: Element, keyframes) {
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

function Columns({ open }: { open: boolean }) {
  const root = useRef<HTMLDivElement>(null)
  return (
    <FlipScope shape={String(open)} root={root}>
      <div ref={root}>
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
  const offsetTop = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "offsetTop")
  Object.defineProperty(HTMLElement.prototype, "offsetTop", {
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
  if (offsetTop) Object.defineProperty(HTMLElement.prototype, "offsetTop", offsetTop)
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

it("lands a zero-duration shape without geometry reads and cancels an earlier flight", async () => {
  const style = globalThis.getComputedStyle
  let duration = "0ms"
  globalThis.getComputedStyle = () =>
    ({ getPropertyValue: () => duration }) as unknown as CSSStyleDeclaration
  const rect = vi.spyOn(Element.prototype, "getBoundingClientRect")
  const cancel = vi.fn()
  const finish: (() => void)[] = []
  Element.prototype.animate = () =>
    ({
      cancel,
      finished: new Promise<void>((resolve) => finish.push(resolve)),
    }) as unknown as Animation
  function Pane({ phase }: { phase: number }) {
    const scope = useRef<HTMLDivElement>(null)
    return (
      <FlipScope shape={String(phase)} root={scope}>
        <div ref={scope}>
          <article data-flip="pane" data-flip-id="a" data-left={phase * 100}>
            <div>Conversation stays mounted</div>
          </article>
        </div>
      </FlipScope>
    )
  }
  const root = createRoot(host)
  try {
    await act(async () => root.render(<Pane phase={0} />))
    const body = host.querySelector("article > div")
    await act(async () => root.render(<Pane phase={1} />))
    expect(rect).not.toHaveBeenCalled()
    expect(finish).toHaveLength(0)
    duration = "300ms"
    await act(async () => root.render(<Pane phase={2} />))
    expect(finish.length).toBeGreaterThan(0)
    expect(host.firstElementChild?.hasAttribute(marks.flipping)).toBe(true)
    const flying = finish.length
    duration = "0ms"
    rect.mockClear()
    await act(async () => root.render(<Pane phase={3} />))
    expect(rect).not.toHaveBeenCalled()
    expect(finish).toHaveLength(flying)
    expect(cancel).toHaveBeenCalledTimes(flying)
    expect(host.querySelector("article > div")).toBe(body)
    expect(host.firstElementChild?.hasAttribute(marks.flipping)).toBe(false)
    expect(host.querySelector(`[${marks.flying}], [${marks.measuring}]`)).toBeNull()
    await act(async () => finish.forEach((resolve) => resolve()))
    expect(host.firstElementChild?.hasAttribute(marks.flipping)).toBe(false)
    expect(host.querySelector("article > div")).toBe(body)
  } finally {
    await act(async () => root.unmount())
    rect.mockRestore()
    globalThis.getComputedStyle = style
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
      children: [{ element: header, top: 0 }],
    },
    { left: 100, top: 0, width: 800, height: 400 } as DOMRect,
    300,
  )
  expect(frames).toHaveLength(2)
  expect(
    frames.every((animation) => animation.every((frame) => frame.opacity === undefined)),
  ).toBe(true)
})
