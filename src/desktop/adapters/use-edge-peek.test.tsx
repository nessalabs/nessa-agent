// @vitest-environment jsdom
/**
 * The edge peek against the clock and the pointer: while a pointer button is
 * held, the pointer neither reveals nor hides the sidebar — as the press
 * found it, it stays, in every engine — and once released, where the pointer
 * last was decides (ADR 238 › _Drag and drop_).
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { useEdgePeek, type EdgePeekControls } from "./use-edge-peek"

let root: Root
let peek: EdgePeekControls

function Peek() {
  peek = useEdgePeek(true, false)
  return null
}

beforeEach(() => {
  vi.useFakeTimers()
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  root = createRoot(document.createElement("div"))
  act(() => root.render(<Peek />))
})

afterEach(() => {
  act(() => root.unmount())
  vi.useRealTimers()
})

const free = { buttons: 0 }
const held = { buttons: 1 }
const wait = (ms: number) => act(() => void vi.advanceTimersByTime(ms))
const release = () => act(() => void window.dispatchEvent(new Event("pointerup")))

it("reveals on a rest at the edge and hides once the pointer has left", async () => {
  act(() => peek.enter(free))
  await wait(200)
  expect(peek.shown).toBe(true)
  act(() => peek.leave(free))
  await wait(400)
  expect(peek.shown).toBe(false)
})

it("stays revealed while a button is held, whatever the pointer passes, and hides once released away", async () => {
  act(() => peek.enter(free))
  await wait(200)
  // A drag begins over it and carries the pointer out: it stays.
  act(() => peek.leave(held))
  await wait(1000)
  expect(peek.shown).toBe(true)
  // Released away from it: now it goes, on its own delay.
  release()
  await wait(400)
  expect(peek.shown).toBe(false)
})

it("stays revealed when released over it, and is not revealed by a held pointer passing the edge", async () => {
  act(() => peek.enter(free))
  await wait(200)
  act(() => peek.leave(held))
  act(() => peek.enter(held))
  release()
  await wait(1000)
  expect(peek.shown).toBe(true)
  act(() => peek.leave(free))
  await wait(400)
  expect(peek.shown).toBe(false)
  // Hidden, a held pointer at the edge reveals nothing.
  act(() => peek.enter(held))
  await wait(1000)
  expect(peek.shown).toBe(false)
})
