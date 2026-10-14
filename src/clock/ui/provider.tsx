import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useSyncExternalStore,
  type ReactNode,
} from "react"

type Listener = () => void

type Clock = {
  now: () => number
  storeFor(interval: number): ClockStore
}

type ClockStore = {
  getSnapshot(): number
  subscribe(listener: Listener): () => void
}

const ClockContext = createContext<Clock | null>(null)

function createClockStore(interval: number): ClockStore {
  let revision = 0
  let activeListeners: Set<Listener> | undefined
  let timer: number | undefined

  return {
    getSnapshot: () => revision,
    subscribe(listener) {
      if (!activeListeners) {
        const listeners = new Set<Listener>()
        activeListeners = listeners
        timer = window.setInterval(() => {
          // The "retires the timer and ignores its late callback after a remount"
          // test in provider.test.tsx checks that a retired callback does not
          // cause a remounted reader to render.
          if (activeListeners !== listeners) return
          revision += 1
          listeners.forEach((each) => each())
        }, interval)
      }

      const listeners = activeListeners
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
        if (listeners.size > 0 || activeListeners !== listeners) return
        activeListeners = undefined
        if (timer !== undefined) window.clearInterval(timer)
        timer = undefined
      }
    },
  }
}

function createClock(now: () => number): Clock {
  const stores = new Map<number, ClockStore>()
  return {
    now,
    storeFor(interval) {
      let store = stores.get(interval)
      if (!store) {
        store = createClockStore(interval)
        stores.set(interval, store)
      }
      return store
    },
  }
}

/** Supplies one injected time source to the clock-driven labels below it. */
export function ClockProvider({
  now,
  children,
}: {
  now: () => number
  children: ReactNode
}) {
  const clock = useMemo(() => createClock(now), [now])
  return <ClockContext.Provider value={clock}>{children}</ClockContext.Provider>
}

/** The injected time now, read again when the interval publishes a tick. */
export function useNow(interval: number): number {
  const clock = useContext(ClockContext)
  if (!clock)
    throw new Error("useNow needs a ClockProvider: the clock comes from composition")
  const store = clock.storeFor(interval)
  const subscribe = useCallback(
    (listener: Listener) => store.subscribe(listener),
    [store],
  )
  useSyncExternalStore(subscribe, store.getSnapshot)
  return clock.now()
}
