import { useLayoutEffect, useRef, useState } from "react"
import { useStore } from "react-redux"
import type { DesktopState, DesktopStore } from "../../../store"
import { selectColumnCount, selectPaneShape } from "./selectors"

/**
 * A painted read of the workspace. The click that opens a pane folds the
 * sidebar in the same dispatch (`arranged`). Rendering the window for that
 * dispatch, and painting the fold on the next frame, each missed the frame
 * budget: the click was the window's render, and the next frame restyled
 * the session list.
 *
 * A change that arrives with a new number of pane columns, or the same
 * number in a new shape (a swap, a drop, a pane moved under another), is
 * painted on the third frame, which is when the grid paints the new boxes
 * (`split-panes/ui/split-panes.tsx`). The next frame shows scene text that
 * was already laid out. The session list's measure and its tab stop wait
 * one frame more (`listPaintFrame`), so that commit does not restyle the
 * list. The count and the shape are recorded as painted on the box frame,
 * so a command that puts the old arrangement back waits for the grid again.
 * A change that does neither — a person's toggle, a focus move between
 * panes already open — is painted in the turn it happens. The value a row
 * or a pane compares is its own boolean, so a focus move renders the rows
 * that gained or lost it.
 */

/**
 * The frame before the boxes. The window's fold is recorded then, while the
 * spare hold still keeps the sidebar in the row, so the fold does not lay
 * the panes out on its own.
 */
export const spareFoldFrame = 2

/** The frame, after the commit, on which the grid paints new boxes. */
export const paneBoxFrame = 3

/**
 * The frame after the scene text returns. The session list keeps the measure
 * it had through the box paint and that reveal, and takes its new one here,
 * with its tab stop.
 */
export const listPaintFrame = 5

interface Listener {
  frames: number
  notify: () => void
}

interface Arrangement {
  columns: number
  shape: string
  tab: string | null
}

interface Gate {
  /** The window tab last seen: another one is a cut, painted at once. */
  tab: string | null
  painted: number
  /** Which pane is where, as last recorded on the box frame. */
  shape: string
  /** The arrangement the wait is already counting down, so a later dispatch of that same arrangement does not start the wait over. */
  waiting: number | null
  waitingShape: string | null
  /** How far that wait has run. A listener at or before this has already painted it. */
  at: number
  /**
   * The column count the session list has measured. The box frame does not
   * move it; the list frame of a wait does.
   */
  settled: number
  /** Bumps when a wait reaches its last frame, including one that returns to `settled`. */
  epoch: number
  frames: number[]
  listeners: Set<Listener>
  unsubscribe: () => void
}

const gates = new WeakMap<DesktopStore, Gate>()

function arrangement(store: DesktopStore): Arrangement {
  const state = store.getState()
  return {
    columns: selectColumnCount(state),
    shape: selectPaneShape(state),
    tab: state.workspace.tabs.activeTab,
  }
}

function sameArrangement(
  columns: number,
  shape: string,
  otherColumns: number,
  otherShape: string,
) {
  return columns === otherColumns && shape === otherShape
}

function cancel(gate: Gate) {
  for (const id of gate.frames) cancelAnimationFrame(id)
  gate.frames = []
  gate.waiting = null
  gate.waitingShape = null
  gate.at = 0
}

/** Records the arrangement the grid is showing once the box frame has run. */
function recordPainted(gate: Gate, now: Arrangement) {
  gate.painted = now.columns
  gate.shape = now.shape
}

/** Notifies each painted read on its frame. The last frame ends the wait. */
function arm(gate: Gate, store: DesktopStore) {
  const now = arrangement(store)
  if (gate.waiting === now.columns && gate.waitingShape === now.shape) return
  cancel(gate)
  const paint = now
  gate.waiting = paint.columns
  gate.waitingShape = paint.shape
  let span = paneBoxFrame
  for (const listener of gate.listeners) span = Math.max(span, listener.frames)
  const queue = (left: number) => {
    const id = requestAnimationFrame(() => {
      // A newer wait replaced this one. Its own frames notify.
      if (gate.waiting !== paint.columns || gate.waitingShape !== paint.shape) return
      const at = span - left + 1
      gate.at = at
      if (left > 1) queue(left - 1)
      // The boxes paint on this frame. A later command that restores the
      // previous count has to wait again, with the grid.
      if (at === paneBoxFrame) recordPainted(gate, paint)
      if (left === 1) {
        gate.frames = []
        gate.waiting = null
        gate.waitingShape = null
        gate.settled = paint.columns
        gate.epoch += 1
        recordPainted(gate, paint)
      }
      for (const listener of gate.listeners) {
        if (listener.frames === at) listener.notify()
      }
    })
    gate.frames.push(id)
  }
  queue(span)
}

function gateFor(store: DesktopStore): Gate {
  const existing = gates.get(store)
  if (existing) return existing
  const now = arrangement(store)
  const gate: Gate = {
    tab: now.tab,
    painted: now.columns,
    shape: now.shape,
    waiting: null,
    waitingShape: null,
    at: 0,
    settled: now.columns,
    epoch: 0,
    frames: [],
    listeners: new Set<Listener>(),
    unsubscribe: () => {},
  }
  gate.unsubscribe = store.subscribe(() => {
    const next = arrangement(store)
    // Another window tab: the grid draws its layout at once
    // (`SplitPanesSource.cut`), and so does everything painted with it.
    if (next.tab !== gate.tab) {
      gate.tab = next.tab
      cancel(gate)
      recordPainted(gate, next)
      gate.settled = next.columns
      gate.epoch += 1
      for (const listener of gate.listeners) listener.notify()
      return
    }
    // The same arrangement is still waiting. This check is first: the box
    // frame records that arrangement as painted while the list's frame is
    // still ahead, and a toggle then must not cancel the rest of the wait.
    // Listeners that already painted it take the toggle now; the ones still
    // waiting read the store on their own frame.
    if (gate.waiting === next.columns && gate.waitingShape === next.shape) {
      for (const listener of gate.listeners) {
        if (listener.frames <= gate.at) listener.notify()
      }
      return
    }
    if (sameArrangement(next.columns, next.shape, gate.painted, gate.shape)) {
      cancel(gate)
      for (const listener of gate.listeners) listener.notify()
      return
    }
    arm(gate, store)
  })
  gates.set(store, gate)
  return gate
}

/**
 * Whether the session list keeps the measure it had. True through a column
 * transition's list frame, including a command that puts the previous count
 * back after the boxes have painted: the list's own value never left that
 * count, and the boxes are still the ones just painted.
 */
export function listMeasureHeld(store: DesktopStore): boolean {
  const gate = gates.get(store)
  if (!gate) return false
  if (gate.waiting !== null) return true
  return selectColumnCount(store.getState()) !== gate.settled
}

/** Changes when a column transition reaches the frame that releases the list measure. */
export function listMeasureEpoch(store: DesktopStore): number {
  return gates.get(store)?.epoch ?? 0
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
  // `useStore` directly: `hooks.ts` re-exports the selector, which reads the
  // frame numbers from this module.
  const store = useStore() as DesktopStore
  const [value, setValue] = useState(() => select(store.getState()))
  const shown = useRef(value)
  const selectRef = useRef(select)
  selectRef.current = select
  useLayoutEffect(() => {
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
    // A mount folds the side columns in a layout effect before this gate
    // exists, so the subscription never hears it. The store has already
    // moved, and that fold is this frame. A change the gate is counting
    // keeps its own frames.
    const live = selectRef.current(store.getState())
    const mounted = arrangement(store)
    if (
      gate.waiting === null &&
      sameArrangement(mounted.columns, mounted.shape, gate.painted, gate.shape) &&
      !Object.is(shown.current, live)
    ) {
      shown.current = live
      setValue(live)
    }
    return () => {
      gate.listeners.delete(listener)
      release(store, gate)
    }
  }, [store, frames])
  return value
}
