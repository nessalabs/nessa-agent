// @vitest-environment jsdom
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, expect, it, vi } from "vitest"
import { ClockProvider, useNow } from "../index"

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

let host: HTMLDivElement
let root: Root

function mount(children: React.ReactNode) {
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
  act(() => root.render(children))
}

afterEach(() => {
  if (root) act(() => root.unmount())
  host?.remove()
  vi.restoreAllMocks()
  vi.useRealTimers()
})

function Reader({ name, interval }: { name: string; interval: number }) {
  const now = useNow(interval)
  return <output data-testid={name}>{now}</output>
}

it("keeps the actual hook snapshot stable when injected time crosses a label bucket across React reads", () => {
  let reads = 0
  const now = () => (reads++ === 0 ? 999 : 1000)
  const errors = vi.spyOn(console, "error").mockImplementation(() => {})

  mount(
    <ClockProvider now={now}>
      <Reader name="clock" interval={1000} />
    </ClockProvider>,
  )

  expect(reads).toBeGreaterThan(0)
  expect(errors.mock.calls.flat().join(" ")).not.toContain(
    "The result of getSnapshot should be cached to avoid an infinite loop",
  )
})

it("publishes timer ticks to same-interval readers without mixing provider clocks", () => {
  vi.useFakeTimers()
  let firstNow = 10
  let secondNow = 20
  const setInterval = vi.spyOn(window, "setInterval")

  mount(
    <>
      <ClockProvider now={() => firstNow}>
        <Reader name="first-a" interval={1000} />
        <Reader name="first-b" interval={1000} />
      </ClockProvider>
      <ClockProvider now={() => secondNow}>
        <Reader name="second" interval={1000} />
      </ClockProvider>
    </>,
  )

  expect(setInterval).toHaveBeenCalledTimes(2)
  expect(host.querySelector('[data-testid="first-a"]')?.textContent).toBe("10")
  expect(host.querySelector('[data-testid="first-b"]')?.textContent).toBe("10")
  expect(host.querySelector('[data-testid="second"]')?.textContent).toBe("20")

  firstNow = 11
  secondNow = 21
  act(() => vi.advanceTimersByTime(1000))

  expect(host.querySelector('[data-testid="first-a"]')?.textContent).toBe("11")
  expect(host.querySelector('[data-testid="first-b"]')?.textContent).toBe("11")
  expect(host.querySelector('[data-testid="second"]')?.textContent).toBe("21")
})

it("retires the timer and ignores its late callback after a remount", () => {
  vi.useFakeTimers()
  const now = () => 10
  const callbacks: Array<() => void> = []
  const originalSetInterval = window.setInterval.bind(window)
  const setInterval = vi.spyOn(window, "setInterval")
  setInterval.mockImplementation(((callback: TimerHandler, delay?: number) => {
    if (typeof callback === "function") callbacks.push(callback as () => void)
    return originalSetInterval(callback, delay)
  }) as typeof window.setInterval)
  const render = (child: React.ReactNode) =>
    act(() => root.render(<ClockProvider now={now}>{child}</ClockProvider>))
  let renders = 0
  function CountedReader() {
    const current = useNow(1000)
    renders += 1
    return <output data-testid="clock">{current}</output>
  }

  mount(
    <ClockProvider now={now}>
      <CountedReader />
    </ClockProvider>,
  )
  expect(callbacks).toHaveLength(1)
  render(null)
  render(<CountedReader />)

  const rendersBeforeLateCallback = renders
  act(() => callbacks[0]())

  expect(renders).toBe(rendersBeforeLateCallback)
  expect(setInterval).toHaveBeenCalledTimes(2)
})

it("replaces interval and provider subscriptions with the current clock", () => {
  vi.useFakeTimers()
  const firstNow = () => 100
  const secondNow = () => 200
  const setInterval = vi.spyOn(window, "setInterval")
  const clearInterval = vi.spyOn(window, "clearInterval")
  const render = (now: () => number, interval: number) =>
    act(() =>
      root.render(
        <ClockProvider now={now}>
          <Reader name="clock" interval={interval} />
        </ClockProvider>,
      ),
    )

  mount(
    <ClockProvider now={firstNow}>
      <Reader name="clock" interval={1000} />
    </ClockProvider>,
  )
  expect(host.querySelector('[data-testid="clock"]')?.textContent).toBe("100")

  render(firstNow, 2000)
  expect(setInterval).toHaveBeenCalledTimes(2)
  expect(clearInterval).toHaveBeenCalledTimes(1)
  render(secondNow, 2000)
  expect(setInterval).toHaveBeenCalledTimes(3)
  expect(clearInterval).toHaveBeenCalledTimes(2)
  expect(host.querySelector('[data-testid="clock"]')?.textContent).toBe("200")
})
