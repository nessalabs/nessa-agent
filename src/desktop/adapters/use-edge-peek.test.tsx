// @vitest-environment jsdom
/**
 * The edge peek against the clock and the pointer: a press freezes it — a
 * reveal or hide on its way is cancelled, and while the button is held the
 * pointer neither reveals nor hides the sidebar — and once released, where
 * the pointer last was decides; a release the page missed is taken at the
 * next enter or leave with no button held, or the window's blur
 * (`model/edge-peek.ts`, ADR 238 › _Side columns_).
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
const press = () => act(() => void window.dispatchEvent(new Event("pointerdown")))
const blur = () => act(() => void window.dispatchEvent(new Event("blur")))

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

it("stays exactly as it was for a press made while its hide was on its way", async () => {
  act(() => peek.enter(free))
  await wait(200)
  act(() => peek.leave(free))
  // Pressed within the hide's 350ms — a pane's header, just right of the peek.
  await wait(200)
  press()
  await wait(1000)
  expect(peek.shown).toBe(true)
  // Released away from it: it goes, on its own delay from the release.
  release()
  await wait(300)
  expect(peek.shown).toBe(true)
  await wait(100)
  expect(peek.shown).toBe(false)
})

it("reveals nothing for a press made while resting at the edge", async () => {
  act(() => peek.enter(free))
  await wait(100)
  press()
  await wait(1000)
  expect(peek.shown).toBe(false)
  // Released still at the edge: now it reveals.
  release()
  await wait(200)
  expect(peek.shown).toBe(true)
})

it("takes an enter with no button held as the release it missed", async () => {
  act(() => peek.enter(free))
  await wait(200)
  press()
  act(() => peek.leave(held))
  // The button is let go outside the window: no pointerup reaches the page.
  act(() => peek.enter(free))
  // A click inside it later is its own press and release: it stays.
  press()
  release()
  await wait(1000)
  expect(peek.shown).toBe(true)
  act(() => peek.leave(free))
  await wait(400)
  expect(peek.shown).toBe(false)
})

it("takes the window's blur as the release", async () => {
  act(() => peek.enter(free))
  await wait(200)
  press()
  act(() => peek.leave(held))
  await wait(1000)
  expect(peek.shown).toBe(true)
  blur()
  await wait(400)
  expect(peek.shown).toBe(false)
})
