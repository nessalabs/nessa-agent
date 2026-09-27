/**
 * The time, for labels that age: "4m", "started 22m ago", "39s". The clock
 * itself is injected from composition (`ClockProvider`), so a test can hold
 * time still; this module only decides when a label is read again. Every
 * reader asking for the same interval shares one timer.
 */
import {
  createContext,
  useCallback,
  useContext,
  useSyncExternalStore,
  type ReactNode,
} from "react"

const ClockContext = createContext<(() => number) | null>(null)

export function ClockProvider({
  now,
  children,
}: {
  now: () => number
  children: ReactNode
}) {
  return <ClockContext.Provider value={now}>{children}</ClockContext.Provider>
}

const tickers = new Map<number, { listeners: Set<() => void>; timer: number }>()

function subscribe(interval: number, listener: () => void): () => void {
  let ticker = tickers.get(interval)
  if (!ticker) {
    const listeners = new Set<() => void>()
    const timer = window.setInterval(() => listeners.forEach((each) => each()), interval)
    ticker = { listeners, timer }
    tickers.set(interval, ticker)
  }
  ticker.listeners.add(listener)
  return () => {
    const current = tickers.get(interval)
    if (!current) return
    current.listeners.delete(listener)
    if (current.listeners.size > 0) return
    window.clearInterval(current.timer)
    tickers.delete(interval)
  }
}

/** The time now, read again every `interval` milliseconds while mounted. */
export function useNow(interval: number): number {
  const now = useContext(ClockContext)
  if (!now)
    throw new Error("useNow needs a ClockProvider: the clock comes from composition")
  // One subscription per interval for the component's life, not one per render.
  const follow = useCallback(
    (listener: () => void) => subscribe(interval, listener),
    [interval],
  )
  useSyncExternalStore(follow, () => Math.floor(now() / interval))
  return now()
}
