// @vitest-environment jsdom
import { act, useRef } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { controlledAnimationFrames } from "../../testing"
import { useParkedAfterSlide } from "./parked"

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
