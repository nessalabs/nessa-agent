import {
  createContext,
  createElement,
  useContext,
  useMemo,
  useSyncExternalStore,
  type ReactNode,
  type RefObject,
} from "react"
import { selectOverviewOpen } from "../../adapters/store/selectors"

type Watched = {
  getState: () => Parameters<typeof selectOverviewOpen>[0]
  subscribe: (onChange: () => void) => () => void
}
type Quiet = {
  subscribe: (onChange: () => void) => () => void
  read: () => boolean
}

/** Publication belongs to one workspace store and the root that paints it. */
function quietFor(store: Watched, root: RefObject<HTMLElement | null>): Quiet {
  let cached = false
  let requested: boolean | null = null
  const listeners = new Set<() => void>()
  let stopStore: (() => void) | null = null
  let outerFrame: number | null = null
  let innerFrame: number | null = null
  let generation = 0
  const cancelFrames = () => {
    generation++
    if (outerFrame !== null) cancelAnimationFrame(outerFrame)
    if (innerFrame !== null) cancelAnimationFrame(innerFrame)
    outerFrame = null
    innerFrame = null
  }
  const publishAfterPaint = () => {
    const owner = generation
    const current = () => owner === generation && listeners.size > 0
    const publish = () => {
      if (!current()) return
      outerFrame = null
      innerFrame = null
      const next = selectOverviewOpen(store.getState())
      if (next === cached) return
      cached = next
      for (const listener of [...listeners]) listener()
    }
    const wait = () => {
      if (!current()) return
      const open = selectOverviewOpen(store.getState())
      if (open && root.current?.querySelector("[data-overview-listed]") == null) {
        outerFrame = requestAnimationFrame(wait)
        return
      }
      outerFrame = requestAnimationFrame(() => {
        if (!current()) return
        innerFrame = requestAnimationFrame(publish)
      })
    }
    outerFrame = requestAnimationFrame(wait)
  }
  const changed = () => {
    const next = selectOverviewOpen(store.getState())
    if (next === requested) return
    requested = next
    cancelFrames()
    if (next !== cached) publishAfterPaint()
  }
  return {
    read: () => cached,
    subscribe: (onChange) => {
      listeners.add(onChange)
      if (listeners.size === 1) {
        stopStore = store.subscribe(changed)
        changed()
      }
      return () => {
        listeners.delete(onChange)
        if (listeners.size > 0) return
        cancelFrames()
        requested = null
        stopStore?.()
        stopStore = null
      }
    },
  }
}

const context = createContext<Quiet | null>(null)

/** Sidebar metadata follows this window's list paint, never another root's. */
export function OverviewQuietProvider({
  store,
  root,
  children,
}: {
  store: Watched
  root: RefObject<HTMLElement | null>
  children: ReactNode
}) {
  const source = useMemo(() => quietFor(store, root), [store, root])
  return createElement(context.Provider, { value: source }, children)
}

export function useOverviewQuiet(): boolean {
  const source = useContext(context)
  if (!source) throw new Error("Overview quiet state requires its workspace provider")
  return useSyncExternalStore(source.subscribe, source.read, () => false)
}
