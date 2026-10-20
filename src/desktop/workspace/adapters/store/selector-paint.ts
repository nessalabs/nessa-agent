import { useCallback, useRef, useSyncExternalStore } from "react"
import { useStore } from "react-redux"
import type { DesktopState, DesktopStore } from "../../../store"
import type { WorkspaceState } from "../../application/workspace-state"
import { paneBoxFrame } from "./paint-with-panes"
import { selectColumnCount } from "./selectors"

/**
 * A selector's subscription. The click that opens a pane used to call every
 * selector in the window, and that script was the long frame. A selector is
 * called only when a field of `workspace` it read has a new reference. When
 * that change is the new layout (`panes` or `chrome`) and the column count
 * changed, the call waits until the new boxes paint (`paneBoxFrame`), so
 * neither the click nor the frame after it renders the window. Anything
 * else renders in the turn (`selector-paint.test.tsx`). The session list's
 * own marks wait one frame more (`listPaintFrame`).
 */

const layoutFields = new Set(["panes", "chrome"])

let deferred: Set<() => void> | null = null
let deferredFrames: number[] = []

function defer(notify: () => void) {
  if (deferred?.has(notify)) return
  if (!deferred) {
    deferred = new Set()
    const queue = (left: number) => {
      const id = requestAnimationFrame(() => {
        // A frame cancelled by the last listener leaving must not queue the next.
        if (!deferred) return
        if (left > 1) {
          queue(left - 1)
          return
        }
        const pending = deferred
        deferred = null
        deferredFrames = []
        if (!pending) return
        for (const tell of pending) tell()
      })
      deferredFrames.push(id)
    }
    queue(paneBoxFrame)
  }
  deferred.add(notify)
}

function forget(notify: () => void) {
  if (!deferred?.delete(notify)) return
  if (deferred.size > 0) return
  for (const id of deferredFrames) cancelAnimationFrame(id)
  deferredFrames = []
  deferred = null
}

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
  const columnsRef = useRef<number | null>(null)
  const cache = useRef<{ value: Selected } | null>(null)

  const subscribe = useCallback(
    (notify: () => void) => {
      const unsubscribe = store.subscribe(() => {
        const read = readRef.current
        const seen = seenRef.current
        if (!changed(store, read, seen)) return
        if (layoutChange(store, read, seen, columnsRef.current)) {
          defer(notify)
          return
        }
        notify()
      })
      return () => {
        forget(notify)
        unsubscribe()
      }
    },
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
    columnsRef.current = selectColumnCount(state)
    if (cache.current && equalRef.current(cache.current.value, value))
      return cache.current.value
    cache.current = { value }
    return value
  }

  return useSyncExternalStore(subscribe, getSnapshot)
}

/**
 * The new column count, and every field that changed is the layout. Paint
 * it with the new boxes. A focus move keeps the count, so it paints in the turn.
 */
function layoutChange(
  store: DesktopStore,
  read: ReadonlySet<string>,
  seen: ReadonlyMap<string, unknown>,
  columnsThen: number | null,
): boolean {
  if (columnsThen === null || read.size === 0) return false
  if (selectColumnCount(store.getState()) === columnsThen) return false
  let layout = false
  const workspace = store.getState().workspace
  for (const field of read) {
    if (Object.is(fieldOf(workspace, field), seen.get(field))) continue
    if (!layoutFields.has(field)) return false
    layout = true
  }
  return layout
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
