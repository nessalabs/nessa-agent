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
import { afterEach, beforeEach, expect, it } from "vitest"
import { FlipScope } from "./flip"

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
