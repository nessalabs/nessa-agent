/**
 * The same ticking clock the desktop approval card uses (`useNow`): one
 * shared timer per interval, and the time itself injected so a test can hold
 * it still. It lives in this layer because the conversation UI does not
 * import a desktop adapter.
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
  const follow = useCallback(
    (listener: () => void) => subscribe(interval, listener),
    [interval],
  )
  useSyncExternalStore(follow, () => Math.floor(now() / interval))
  return now()
}
