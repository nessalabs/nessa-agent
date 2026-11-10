// @vitest-environment jsdom
import { act, useRef } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { controlledAnimationFrames } from "../../testing"
import { slidingGuards, useFocusLeavesOnFold, useParkedAfterSlide } from "./parked"

let host: HTMLDivElement
let root: Root
let frames: ReturnType<typeof controlledAnimationFrames>

beforeEach(() => {
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
  frames = controlledAnimationFrames()
})

afterEach(() => {
  act(() => root.unmount())
  frames.restore()
  host.remove()
})

/** A slide the test ends or cancels, standing in for the column's CSS transition. */
function slide() {
  let finish = () => {}
  let cancel = () => {}
  const finished = new Promise<void>((resolve, reject) => {
    finish = resolve
    cancel = () => reject(new DOMException("cancelled", "AbortError"))
  })
  finished.catch(() => {})
  const animation = { transitionProperty: "transform", playState: "running", finished }
  return { animation, finish, cancel }
}

function Column({ away, slides }: { away: boolean; slides: () => unknown[] }) {
  const ref = useRef<HTMLElement>(null)
  const parked = useParkedAfterSlide(ref, away)
  return (
    <aside
      ref={(node) => {
        ref.current = node
        if (node) Object.assign(node, { getAnimations: slides })
      }}
      data-parked={parked || undefined}
    />
  )
}

const parked = () => host.querySelector("aside")?.hasAttribute("data-parked")

it("parks a column only once its slide away has ended, and unparks it at once", async () => {
  const away = slide()
  let running: unknown[] = []
  await act(async () => root.render(<Column away={false} slides={() => running} />))
  expect(parked()).toBe(false)
  running = [away.animation]
  await act(async () => root.render(<Column away slides={() => running} />))
  // The fold's own commit leaves it reachable and painted.
  expect(parked()).toBe(false)
  await act(async () => frames.runFrame())
  // Still sliding: its content paints all the way out.
  expect(parked()).toBe(false)
  await act(async () => away.finish())
  expect(parked()).toBe(true)
  running = []
  await act(async () => root.render(<Column away={false} slides={() => running} />))
  expect(parked()).toBe(false)
})

it("never parks a column that comes back before its slide ends", async () => {
  const away = slide()
  await act(async () => root.render(<Column away={false} slides={() => []} />))
  await act(async () => root.render(<Column away slides={() => [away.animation]} />))
  await act(async () => frames.runFrame())
  await act(async () => root.render(<Column away={false} slides={() => []} />))
  await act(async () => away.cancel())
  // A late finish of that slide does not park the column that came back.
  await act(async () => away.finish())
  expect(parked()).toBe(false)
})

it("parks on the next frame when nothing slides, and at once when it starts away", async () => {
  await act(async () => root.render(<Column away={false} slides={() => []} />))
  await act(async () => root.render(<Column away slides={() => []} />))
  expect(parked()).toBe(false)
  await act(async () => frames.runFrame())
  expect(parked()).toBe(true)
  act(() => root.unmount())
  root = createRoot(host)
  await act(async () => root.render(<Column away slides={() => []} />))
  expect(parked()).toBe(true)
})

it("parks after the slide that replaced a cancelled one while the column stayed away", async () => {
  const first = slide()
  const second = slide()
  let running: unknown[] = [first.animation]
  await act(async () => root.render(<Column away={false} slides={() => []} />))
  await act(async () => root.render(<Column away slides={() => running} />))
  await act(async () => frames.runFrame())
  running = [second.animation]
  await act(async () => first.cancel())
  expect(parked()).toBe(false)
  await act(async () => frames.runFrame())
  await act(async () => second.finish())
  expect(parked()).toBe(true)
})

function Focusable({ away }: { away: boolean }) {
  const ref = useRef<HTMLElement>(null)
  useFocusLeavesOnFold(ref, away)
  return (
    <aside ref={ref}>
      <button type="button">row</button>
    </aside>
  )
}

it("takes focus out of a column on the commit that folds it", async () => {
  await act(async () => root.render(<Focusable away={false} />))
  host.querySelector("button")?.focus()
  expect(document.activeElement?.tagName).toBe("BUTTON")
  await act(async () => root.render(<Focusable away />))
  expect(document.activeElement).toBe(document.body)
})

it("waits while a peek or a pane change still holds the column on screen", async () => {
  await act(async () => root.render(<Column away={false} slides={() => []} />))
  host.setAttribute("data-peek", "")
  await act(async () => root.render(<Column away slides={() => []} />))
  await act(async () => frames.runFrame())
  await act(async () => frames.runFrame())
  expect(parked()).toBe(false)
  host.removeAttribute("data-peek")
  await act(async () => frames.runFrame())
  expect(parked()).toBe(true)
})

it("keeps clicks and focus out of a column while it slides", async () => {
  let clicked = 0
  function Sliding() {
    return (
      <aside {...slidingGuards(true)}>
        <button type="button" onClick={() => (clicked += 1)}>
          row
        </button>
      </aside>
    )
  }
  await act(async () => root.render(<Sliding />))
  const button = host.querySelector("button")
  button?.click()
  expect(clicked).toBe(0)
  const menu = new MouseEvent("contextmenu", { bubbles: true, cancelable: true })
  button?.dispatchEvent(menu)
  expect(menu.defaultPrevented).toBe(true)
  button?.focus()
  expect(document.activeElement).toBe(document.body)
})
