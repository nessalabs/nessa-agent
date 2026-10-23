// @vitest-environment jsdom
/**
 * The opening's beats and the finished window's flight, run the way React
 * runs them: under Strict Mode, as `main.tsx` has it, with timers the test
 * moves by hand and a host that answers where the menu bar icon is when told.
 *
 * The shapes and timings are tested as plain functions in `opening.test.ts`.
 * What needs the DOM is the order of things: a finish is still on screen until
 * its flight lands, a slow host never holds the flight on the ground, and
 * reduced motion has no opening and no flight at all.
 */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const host = vi.hoisted(() => ({
  setupTrayPoint: vi.fn<() => Promise<{ x: number; y: number } | undefined>>(),
}))
vi.mock("../../host", () => host)

import { FLIGHT_MS, OPENING_BEATS } from "../model/opening"
import { useOpening } from "./use-opening"
import { useSetupFlight } from "./use-setup-flight"

/** A promise the test settles by hand. */
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((res) => {
    resolve = res
  })
  return { promise, resolve }
}

/** Web Animations as far as the flight uses them, finished by the test. */
const animations: {
  keyframes: Keyframe[]
  land: () => void
  cancel: ReturnType<typeof vi.fn>
}[] = []
function animate(this: Element, keyframes: Keyframe[]) {
  const finished = deferred<void>()
  const cancel = vi.fn()
  animations.push({ keyframes, land: () => finished.resolve(), cancel })
  return { finished: finished.promise, cancel } as unknown as Animation
}

let reduced = false
let container: HTMLDivElement
let root: Root
let seen: { flying: boolean; beat: string }

function Window({ finished }: { finished: boolean }) {
  const shell = React.useRef<HTMLDivElement>(null)
  const { flying } = useSetupFlight(finished, shell)
  const beat = useOpening()
  seen = { flying, beat }
  return React.createElement("div", { ref: shell })
}

async function render(finished: boolean) {
  await React.act(async () => {
    root.render(
      React.createElement(
        React.StrictMode,
        null,
        React.createElement(Window, { finished }),
      ),
    )
  })
}

/** Move the clock and let React and every pending promise run. */
async function advance(ms: number) {
  await React.act(async () => {
    await vi.advanceTimersByTimeAsync(ms)
  })
}

beforeEach(() => {
  vi.useFakeTimers()
  reduced = false
  animations.length = 0
  host.setupTrayPoint.mockReset()
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: reduced && query.includes("reduce"),
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
  }))
  HTMLElement.prototype.animate = animate as unknown as HTMLElement["animate"]
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
  vi.unstubAllGlobals()
  vi.useRealTimers()
  delete (HTMLElement.prototype as { animate?: unknown }).animate
})

describe("the opening", () => {
  it("walks glow, fill, form, open on its clock", async () => {
    await render(false)
    const walked = [seen.beat]
    for (let index = 1; index < OPENING_BEATS.length; index += 1) {
      await advance(OPENING_BEATS[index].at - OPENING_BEATS[index - 1].at)
      walked.push(seen.beat)
    }
    expect(walked).toEqual(["glow", "fill", "form", "open"])
  })

  it("is simply open under reduced motion", async () => {
    reduced = true
    await render(false)
    expect(seen.beat).toBe("open")
  })
})

describe("the flight into the menu bar", () => {
  it("keeps a finish on screen until the flight lands, and asks the host once", async () => {
    host.setupTrayPoint.mockResolvedValue({ x: 1400, y: 11 })
    await render(false)
    await render(true)
    expect(seen.flying).toBe(true)
    await advance(0)
    expect(host.setupTrayPoint).toHaveBeenCalledTimes(1)
    expect(animations).toHaveLength(1)
    await advance(FLIGHT_MS - 1)
    expect(seen.flying).toBe(true)
    await React.act(async () => animations[0].land())
    expect(seen.flying).toBe(false)
    // Landed where it landed: the window closes over it.
    expect(animations[0].cancel).not.toHaveBeenCalled()
  })

  it("flies to the corner when the host is slow to say where its icon is", async () => {
    const slow = deferred<{ x: number; y: number } | undefined>()
    host.setupTrayPoint.mockReturnValue(slow.promise)
    await render(false)
    await render(true)
    await advance(399)
    expect(animations).toHaveLength(0)
    await advance(1)
    expect(animations).toHaveLength(1)
    // A late answer does not start a second flight.
    await React.act(async () => slow.resolve({ x: 10, y: 10 }))
    expect(animations).toHaveLength(1)
  })

  it("lands on its deadline when the animation never reports, counted from takeoff", async () => {
    const slow = deferred<{ x: number; y: number } | undefined>()
    host.setupTrayPoint.mockReturnValue(slow.promise)
    await render(false)
    await render(true)
    // The ask's wait is not part of the flight's budget.
    await advance(400 + FLIGHT_MS)
    expect(seen.flying).toBe(true)
    await advance(600)
    expect(seen.flying).toBe(false)
  })

  it("does not fly under reduced motion: the finish hands over at once", async () => {
    reduced = true
    host.setupTrayPoint.mockResolvedValue({ x: 1400, y: 11 })
    await render(false)
    await render(true)
    expect(seen.flying).toBe(false)
    await advance(FLIGHT_MS * 3)
    expect(host.setupTrayPoint).not.toHaveBeenCalled()
    expect(animations).toHaveLength(0)
  })

  it("does not fly a setup that never finished", async () => {
    await render(false)
    await advance(FLIGHT_MS * 3)
    expect(seen.flying).toBe(false)
    expect(host.setupTrayPoint).not.toHaveBeenCalled()
  })

  it("flies again after setup comes back and finishes again", async () => {
    host.setupTrayPoint.mockResolvedValue(undefined)
    await render(false)
    await render(true)
    await advance(0)
    await React.act(async () => animations[0].land())
    expect(seen.flying).toBe(false)
    await render(false)
    await render(true)
    expect(seen.flying).toBe(true)
  })
})
