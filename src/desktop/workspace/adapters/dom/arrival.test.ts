// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import {
  arrivalWaitingAttribute,
  captureArrival,
  playArrival,
  type Arrival,
} from "./arrival"

let host: HTMLDivElement
let deliver: IntersectionObserverCallback
let disconnect: ReturnType<typeof vi.fn>
let finish: (() => void)[]
let flights: { cancel: ReturnType<typeof vi.fn>; finished: Promise<void> }[]
let animate: ReturnType<typeof vi.fn>

const box = (left: number, top: number, width: number, height: number) =>
  ({ left, top, width, height }) as DOMRectReadOnly
const from: Arrival = {
  composer: box(100, 200, 400, 100),
  text: box(116, 208, 360, 40),
  duration: 400,
  glide: "ease-out",
  headingDuration: 200,
  headingDelay: 20,
  ease: "ease",
}

beforeEach(() => {
  host = document.createElement("div")
  host.innerHTML =
    '<form class="desktop-composer"><textarea></textarea></form><div data-role="user"><div class="workspace-bubble">Sent words</div></div><div class="workspace-heading">Title</div>'
  document.body.append(host)
  disconnect = vi.fn()
  finish = []
  flights = []
  class Observer {
    constructor(callback: IntersectionObserverCallback) {
      deliver = callback
    }
    observe() {}
    disconnect = disconnect
  }
  vi.stubGlobal("IntersectionObserver", Observer)
  animate = vi.fn(() => {
    let rejectFlight: (reason: unknown) => void = () => {}
    const flight = {
      cancel: vi.fn(() => rejectFlight(new DOMException("Cancelled", "AbortError"))),
      finished: new Promise<void>((resolve, reject) => {
        finish.push(resolve)
        rejectFlight = reject
      }),
    }
    flights.push(flight)
    return flight as unknown as Animation
  })
  vi.stubGlobal("getComputedStyle", () => ({
    getPropertyValue: (name: string) =>
      name === "--desktop-arrival"
        ? "400ms"
        : name === "--desktop-slow"
          ? "200ms"
          : name === "--desktop-stagger"
            ? "20ms"
            : "ease",
  }))
  for (const target of host.querySelectorAll<HTMLElement>(
    ".desktop-composer, .workspace-bubble, .workspace-heading",
  ))
    target.animate = animate
})

afterEach(() => {
  host.remove()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})

function entries() {
  return [
    ...host.querySelectorAll(".desktop-composer, .workspace-bubble, .workspace-heading"),
  ].map(
    (target, index) =>
      ({
        target,
        boundingClientRect: box(50, 50 + index * 100, 200, 50),
      }) as IntersectionObserverEntry,
  )
}
const report = (values = entries()) => deliver(values, {} as IntersectionObserver)

it("waits for all browser-owned boxes, then glides from the field without reading destination layout", async () => {
  const done = vi.fn()
  const read = vi
    .spyOn(Element.prototype, "getBoundingClientRect")
    .mockImplementation(() => {
      throw new Error("destination layout was read")
    })
  const stop = playArrival(host, from, done)
  expect(host.hasAttribute(arrivalWaitingAttribute)).toBe(true)
  report(entries().slice(0, 1))
  expect(animate).not.toHaveBeenCalled()
  report(entries().slice(1))
  expect(host.hasAttribute(arrivalWaitingAttribute)).toBe(false)
  expect(disconnect).toHaveBeenCalledOnce()
  expect(read).not.toHaveBeenCalled()
  expect(animate.mock.calls[0][0]).toEqual([
    { transform: "translate(150px, 175px) scale(2)" },
    { transform: "none" },
  ])
  expect(animate.mock.calls[1][0]).toEqual([
    { transform: "translate(50px, 50px)" },
    { transform: "none" },
  ])
  report()
  expect(animate).toHaveBeenCalledTimes(3)
  finish[0]()
  await Promise.resolve()
  expect(done).not.toHaveBeenCalled()
  finish.slice(1).forEach((resolve) => resolve())
  await Promise.resolve()
  await Promise.resolve()
  expect(done).toHaveBeenCalledOnce()
  stop()
  expect(flights.every((flight) => flight.cancel.mock.calls.length === 1)).toBe(true)
})

it("rejects a late observation after cleanup and cannot clear a newer arrival's waiting mark", () => {
  const done = vi.fn()
  const stop = playArrival(host, from, done)
  const late = deliver
  stop()
  expect(host.hasAttribute(arrivalWaitingAttribute)).toBe(false)
  const stopNext = playArrival(host, from, done)
  stop()
  late(entries(), {} as IntersectionObserver)
  expect(host.hasAttribute(arrivalWaitingAttribute)).toBe(true)
  expect(animate).not.toHaveBeenCalled()
  expect(done).not.toHaveBeenCalled()
  stopNext()
})

it("cancels launched flights without reporting completion after the view leaves", async () => {
  const done = vi.fn()
  const stop = playArrival(host, from, done)
  report()
  stop()
  finish.forEach((resolve) => resolve())
  await Promise.resolve()
  await Promise.resolve()
  expect(done).not.toHaveBeenCalled()
  expect(flights.every((flight) => flight.cancel.mock.calls.length === 1)).toBe(true)
})

it("captures the origin before send and reads no geometry with reduced motion", () => {
  const composer = host.querySelector<HTMLElement>(".desktop-composer")!
  const text = host.querySelector("textarea")!
  vi.spyOn(composer, "getBoundingClientRect").mockReturnValue(from.composer as DOMRect)
  vi.spyOn(text, "getBoundingClientRect").mockReturnValue(from.text as DOMRect)
  expect(captureArrival(host)).toMatchObject({
    composer: from.composer,
    text: from.text,
    duration: 400,
  })
  vi.stubGlobal("getComputedStyle", () => ({ getPropertyValue: () => "0ms" }))
  const reads = vi.spyOn(Element.prototype, "getBoundingClientRect")
  expect(captureArrival(host)).toBeNull()
  expect(reads).not.toHaveBeenCalled()
})

it("does not capture an unlaid-out home or leave a missing destination waiting", () => {
  expect(captureArrival(host)).toBeNull()
  host.querySelector(".workspace-bubble")?.remove()
  const done = vi.fn()
  const stop = playArrival(host, from, done)
  expect(done).toHaveBeenCalledOnce()
  expect(host.hasAttribute(arrivalWaitingAttribute)).toBe(false)
  expect(animate).not.toHaveBeenCalled()
  stop()
})
