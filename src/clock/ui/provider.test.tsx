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

it("keeps a retired timer from changing the remounted snapshot", async () => {
  vi.useFakeTimers()
  let currentTime = 10
  let afterSnapshotRead: (() => void) | undefined
  const now = () => {
    const callback = afterSnapshotRead
    afterSnapshotRead = undefined
    callback?.()
    return currentTime
  }
  const callbacks: Array<() => void> = []
  const originalSetInterval = window.setInterval.bind(window)
  const setInterval = vi.spyOn(window, "setInterval")
  setInterval.mockImplementation(((callback: TimerHandler, delay?: number) => {
    if (typeof callback === "function") callbacks.push(callback as () => void)
    return originalSetInterval(callback, delay)
  }) as typeof window.setInterval)
  const renders = { reader: 0, probe: 0 }
  let setReaderVisible: (visible: boolean) => void = () => {}
  let setProbeVisible: (visible: boolean) => void = () => {}
  const CountedReader = React.memo(function CountedReader({
    name,
  }: {
    name: "reader" | "probe"
  }) {
    const current = useNow(1000)
    renders[name] += 1
    return <output data-testid={name}>{current}</output>
  })
  function Parent() {
    const [readerVisible, setReaderVisibleState] = React.useState(true)
    const [probeVisible, setProbeVisibleState] = React.useState(false)
    setReaderVisible = setReaderVisibleState
    setProbeVisible = setProbeVisibleState
    return (
      <ClockProvider now={now}>
        {readerVisible ? <CountedReader name="reader" /> : null}
        {probeVisible ? <CountedReader name="probe" /> : null}
      </ClockProvider>
    )
  }

  mount(<Parent />)
  expect(callbacks).toHaveLength(1)
  act(() => setReaderVisible(false))
  act(() => setReaderVisible(true))
  expect(callbacks).toHaveLength(2)

  currentTime = 1010
  const readerRendersBeforeParentUpdate = renders.reader
  let retiredCallbackDelivered = false
  afterSnapshotRead = () => {
    retiredCallbackDelivered = true
    callbacks[0]()
  }
  await act(async () => {
    React.startTransition(() => setProbeVisible(true))
    await Promise.resolve()
  })

  expect(retiredCallbackDelivered).toBe(true)
  expect(renders.reader).toBe(readerRendersBeforeParentUpdate)
  expect(renders.probe).toBe(1)
  expect(host.querySelector('[data-testid="reader"]')?.textContent).toBe("10")

  const readerRendersBeforeCurrentTick = renders.reader
  const probeRendersBeforeCurrentTick = renders.probe
  act(() => callbacks[1]())

  expect(renders.reader).toBe(readerRendersBeforeCurrentTick + 1)
  expect(renders.probe).toBe(probeRendersBeforeCurrentTick + 1)
  expect(host.querySelector('[data-testid="reader"]')?.textContent).toBe("1010")
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
