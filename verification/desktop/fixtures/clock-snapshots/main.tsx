import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { ClockProvider, useNow } from "../../../../src/clock"

declare global {
  interface Window {
    __clockProbe: {
      nowReads: number
      renders: number
      timerStarts: number
      timerTicks: number
      timerStops: number
      intervals: number[]
      advance(): void
      publishTick(): void
      unmount(): void
    }
  }
}

const probe: Window["__clockProbe"] = (window.__clockProbe = {
  nowReads: 0,
  renders: 0,
  timerStarts: 0,
  timerTicks: 0,
  timerStops: 0,
  intervals: [],
  advance: () => {},
  publishTick: () => {},
  unmount: () => {},
})

let currentTime = 999
const now = () => {
  probe.nowReads += 1
  return currentTime
}

function ClockReadout() {
  const current = useNow(1000)
  const renders = ++probe.renders
  return (
    <output id="clock" data-now={current} data-renders={renders}>
      {current}
    </output>
  )
}

const container = document.getElementById("root")
if (!container) throw new Error("missing #root")
const root: Root = createRoot(container)
probe.unmount = () => root.unmount()
probe.advance = () => {
  currentTime = 1000
}

const originalSetInterval = window.setInterval.bind(window)
const activeTicks = new Map<number, (...args: unknown[]) => void>()
window.setInterval = ((callback: TimerHandler, delay?: number, ...args: unknown[]) => {
  if (delay !== 1000) return originalSetInterval(callback, delay, ...args)
  probe.timerStarts += 1
  probe.intervals.push(delay)
  const tick = (...callbackArgs: unknown[]) => {
    probe.timerTicks += 1
    if (typeof callback === "function") callback(...callbackArgs)
  }
  // Keep the real callback and interval handle, but hold the first browser tick
  // until the script has recorded the pre-publication React render.
  const handle = originalSetInterval(tick, 60_000, ...args)
  activeTicks.set(handle, tick)
  return handle
}) as typeof window.setInterval
probe.publishTick = () => {
  const tick = [...activeTicks.values()].at(-1)
  if (!tick) throw new Error("the clock interval has no active callback")
  tick()
}

const originalClearInterval = window.clearInterval.bind(window)
window.clearInterval = ((handle?: number) => {
  if (handle !== undefined && activeTicks.has(handle)) {
    probe.timerStops += 1
    activeTicks.delete(handle)
  }
  originalClearInterval(handle)
}) as typeof window.clearInterval

root.render(
  <React.StrictMode>
    <ClockProvider now={now}>
      <ClockReadout />
    </ClockProvider>
  </React.StrictMode>,
)
