import { useSyncExternalStore } from "react"
import { useWorkspaceStore } from "../../adapters/store/hooks"
import { selectOverviewOpen } from "../../adapters/store/selectors"

type Watched = {
  getState: () => Parameters<typeof selectOverviewOpen>[0]
  subscribe: (onChange: () => void) => () => void
}

/**
 * Whether the overview is open, published after its list has painted and
 * two frames more. Sidebar rows read it for `aria-current`. The quiet look
 * is CSS under `data-overview-glass` (`source-list.css`). `layouts.test.tsx`
 * waits until the rows have caught up before it reads the current page.
 */
let watched: Watched | null = null
let cached: boolean | null = null
const listeners = new Set<() => void>()
let stopStore: (() => void) | null = null
let outerFrame = 0
let innerFrame = 0

const cancelFrames = () => {
  cancelAnimationFrame(outerFrame)
  cancelAnimationFrame(innerFrame)
  outerFrame = 0
  innerFrame = 0
}

const publish = (store: Watched) => {
  outerFrame = 0
  innerFrame = 0
  const next = selectOverviewOpen(store.getState())
  if (next === cached) return
  cached = next
  for (const listener of [...listeners]) listener()
}

/**
 * After the overview has listed its rows, then two frames more: the list's
 * last row and the focus that follows it stay their own frames
 * (`overview.tsx`). Closing skips the wait and still leaves the key's frame
 * before publishing (`layouts.test.tsx`).
 */
const publishAfterPaint = (store: Watched) => {
  const wait = () => {
    const open = selectOverviewOpen(store.getState())
    if (open && document.querySelector("[data-overview-listed]") === null) {
      outerFrame = requestAnimationFrame(wait)
      return
    }
    outerFrame = requestAnimationFrame(() => {
      innerFrame = requestAnimationFrame(() => publish(store))
    })
  }
  outerFrame = requestAnimationFrame(wait)
}

const watch = (store: Watched) => {
  watched = store
  cached = selectOverviewOpen(store.getState())
  stopStore = store.subscribe(() => {
    const next = selectOverviewOpen(store.getState())
    if (next === cached) {
      cancelFrames()
      return
    }
    cancelFrames()
    publishAfterPaint(store)
  })
}

const read = (store: Watched) =>
  watched === store && cached !== null ? cached : selectOverviewOpen(store.getState())

export function useOverviewQuiet(): boolean {
  const store = useWorkspaceStore()
  return useSyncExternalStore(
    (onChange) => {
      if (listeners.size === 0) watch(store)
      listeners.add(onChange)
      return () => {
        listeners.delete(onChange)
        if (listeners.size > 0) return
        cancelFrames()
        stopStore?.()
        stopStore = null
        watched = null
        cached = null
      }
    },
    () => read(store),
    () => false,
  )
}
