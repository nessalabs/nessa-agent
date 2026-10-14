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
      unmount(): void
    }
  }
}

const probe = (window.__clockProbe = {
  nowReads: 0,
  renders: 0,
  timerStarts: 0,
  timerTicks: 0,
  timerStops: 0,
  unmount: () => {},
})

const now = () => (probe.nowReads++ === 0 ? 999 : 1000)

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

const originalSetInterval = window.setInterval.bind(window)
window.setInterval = ((callback: TimerHandler, delay?: number, ...args: unknown[]) => {
  probe.timerStarts += 1
  return originalSetInterval(
    (...callbackArgs: unknown[]) => {
      probe.timerTicks += 1
      if (typeof callback === "function") callback(...callbackArgs)
    },
    delay,
    ...args,
  )
}) as typeof window.setInterval

const originalClearInterval = window.clearInterval.bind(window)
window.clearInterval = ((handle?: number) => {
  probe.timerStops += 1
  originalClearInterval(handle)
}) as typeof window.clearInterval

root.render(
  <React.StrictMode>
    <ClockProvider now={now}>
      <ClockReadout />
    </ClockProvider>
  </React.StrictMode>,
)
