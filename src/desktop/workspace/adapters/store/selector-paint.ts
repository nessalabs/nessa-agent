import { useCallback, useRef, useSyncExternalStore } from "react"
import { useStore } from "react-redux"
import type { DesktopState, DesktopStore } from "../../../store"
import type { WorkspaceState } from "../../application/workspace-state"

/**
 * A selector's subscription. The click that opens a pane used to call every
 * selector in the window, and that script was the long frame. A selector is
 * called only when a field of `workspace` it read has a new reference, and
 * it renders in that turn (`selector-paint.test.tsx`).
 */

function fieldOf(workspace: WorkspaceState, field: string): unknown {
  return Object.hasOwn(workspace, field)
    ? workspace[field as keyof WorkspaceState]
    : undefined
}

/** The state a selector reads, recording each field of `workspace` it touches. */
function track(state: DesktopState, read: Set<string>): DesktopState {
  const workspace = new Proxy(state.workspace, {
    get(target, prop, receiver) {
      if (typeof prop === "string") read.add(prop)
      return Reflect.get(target, prop, receiver)
    },
  })
  return new Proxy(state, {
    get(target, prop, receiver) {
      if (prop === "workspace") return workspace
      return Reflect.get(target, prop, receiver)
    },
  })
}

export function useWorkspaceSelector<Selected>(
  selector: (state: DesktopState) => Selected,
  equalityFn: (left: Selected, right: Selected) => boolean = Object.is,
): Selected {
  const store = useStore() as DesktopStore
  const selectRef = useRef(selector)
  const equalRef = useRef(equalityFn)
  selectRef.current = selector
  equalRef.current = equalityFn
  const readRef = useRef<ReadonlySet<string>>(new Set())
  const seenRef = useRef<ReadonlyMap<string, unknown>>(new Map())
  const cache = useRef<{ value: Selected } | null>(null)

  const subscribe = useCallback(
    (notify: () => void) =>
      store.subscribe(() => {
        if (changed(store, readRef.current, seenRef.current)) notify()
      }),
    [store],
  )

  const getSnapshot = () => {
    const state = store.getState()
    const read = new Set<string>()
    const value = selectRef.current(track(state, read))
    const seen = new Map<string, unknown>()
    for (const field of read) seen.set(field, fieldOf(state.workspace, field))
    readRef.current = read
    seenRef.current = seen
    if (cache.current && equalRef.current(cache.current.value, value))
      return cache.current.value
    cache.current = { value }
    return value
  }

  return useSyncExternalStore(subscribe, getSnapshot)
}

/** Something this selector read has a new reference, or it read nothing we can see. */
function changed(
  store: DesktopStore,
  read: ReadonlySet<string>,
  seen: ReadonlyMap<string, unknown>,
): boolean {
  if (read.size === 0) return true
  for (const field of read) {
    if (!Object.is(fieldOf(store.getState().workspace, field), seen.get(field)))
      return true
  }
  return false
}
