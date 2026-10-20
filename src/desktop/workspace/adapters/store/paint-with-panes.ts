import { useEffect, useRef, useState } from "react"
import type { DesktopState, DesktopStore } from "../../../store"
import { useWorkspaceStore } from "./hooks"
import { selectColumnCount } from "./selectors"

/**
 * A painted read of the workspace. The click that opens a pane folds the
 * sidebar in the same dispatch (`arranged`). Rendering the window for that
 * dispatch, and painting the fold on the next frame, each missed the frame
 * budget: the click was the window's render, and the next frame restyled
 * the session list.
 *
 * A change that arrives with a new number of pane columns is painted on the
 * third frame, which is when the grid paints the new boxes
 * (`split-panes/ui/split-panes.tsx`). The session list's measure and its tab
 * stop wait one frame more (`listPaintFrame`), so that commit does not
 * restyle the list. A change that does not — a person's toggle, a focus move
 * between panes already open — is painted in the turn it happens. The value
 * a row or a pane compares is its own boolean, so a focus move renders the
 * rows that gained or lost it.
 */

/** The frame, after the commit, on which the grid paints new boxes. */
export const paneBoxFrame = 3

/**
 * The frame after the boxes. The session list keeps the measure it had
 * through the fold, and takes its new one here, with its tab stop.
 */
export const listPaintFrame = 4

interface Listener {
  frames: number
  notify: () => void
}

interface Gate {
  painted: number
  /** The column count the wait is already counting down, so a later dispatch of that same count does not start the wait over. */
  waiting: number | null
  frames: number[]
  listeners: Set<Listener>
  unsubscribe: () => void
}

const gates = new WeakMap<DesktopStore, Gate>()

function cancel(gate: Gate) {
  for (const id of gate.frames) cancelAnimationFrame(id)
  gate.frames = []
  gate.waiting = null
}

/** Notifies each painted read on its frame. The last frame ends the wait. */
function arm(gate: Gate, store: DesktopStore) {
  const columns = selectColumnCount(store.getState())
  if (gate.waiting === columns) return
  cancel(gate)
  gate.waiting = columns
  let span = paneBoxFrame
  for (const listener of gate.listeners) span = Math.max(span, listener.frames)
  const queue = (left: number) => {
    const id = requestAnimationFrame(() => {
      const at = span - left + 1
      if (left > 1) queue(left - 1)
      for (const listener of gate.listeners) {
        if (listener.frames === at) listener.notify()
      }
      if (left > 1) return
      gate.frames = []
      gate.waiting = null
      gate.painted = selectColumnCount(store.getState())
    })
    gate.frames.push(id)
  }
  queue(span)
}

function gateFor(store: DesktopStore): Gate {
  const existing = gates.get(store)
  if (existing) return existing
  const gate: Gate = {
    painted: selectColumnCount(store.getState()),
    waiting: null,
    frames: [],
    listeners: new Set<Listener>(),
    unsubscribe: () => {},
  }
  gate.unsubscribe = store.subscribe(() => {
    const columns = selectColumnCount(store.getState())
    if (columns === gate.painted) {
      cancel(gate)
      for (const listener of gate.listeners) listener.notify()
      return
    }
    arm(gate, store)
  })
  gates.set(store, gate)
  return gate
}

function release(store: DesktopStore, gate: Gate) {
  if (gate.listeners.size > 0) return
  cancel(gate)
  gate.unsubscribe()
  gates.delete(store)
}

/**
 * `select` as last painted. The function may close over the caller's props;
 * it is read when the store notifies, not when this hook renders.
 */
export function usePaintedWithPanes<T>(
  select: (state: DesktopState) => T,
  frames = paneBoxFrame,
): T {
  const store = useWorkspaceStore()
  const [value, setValue] = useState(() => select(store.getState()))
  const shown = useRef(value)
  const selectRef = useRef(select)
  selectRef.current = select
  useEffect(() => {
    const gate = gateFor(store)
    const listener: Listener = {
      frames,
      notify: () => {
        const next = selectRef.current(store.getState())
        if (Object.is(shown.current, next)) return
        shown.current = next
        setValue(next)
      },
    }
    gate.listeners.add(listener)
    return () => {
      gate.listeners.delete(listener)
      release(store, gate)
    }
  }, [store, frames])
  return value
}
