/**
 * What a host gives split panes: one source it builds once, which reads the
 * layout the host keeps and carries out every change through the host's own
 * commands. The module keeps no layout of its own — a drop, a resize, an
 * equalize or a fit is asked of the source, and nothing else changes one
 * (held by `split-panes.test.tsx`, "resizes, evens and fits through the
 * source", and the drag's tests, which read every drop from the source) —
 * so the rules a host adds to a change (folding a sidebar for room, say)
 * keep one owner. A host with no such rules applies a drop as the drag
 * previews it: the layout `dropOutcome` gives.
 */
import type { Carried } from "../model/drop"
import type { PaneKey, PaneLayout, Zone } from "../model/pane-layout"
import type { PaneEdge, PaneRoom } from "../model/pane-sizing"

/** A drop a drag commits: what was carried, where, in the room the press read. */
export interface Drop {
  readonly carried: Carried
  readonly target: PaneKey
  readonly zone: Zone
  readonly room: PaneRoom | undefined
}

/** An edge between panes moved: the side before it takes `fraction` of the two sides' `pair` pixels. */
export interface EdgeMove {
  readonly edge: PaneEdge
  readonly fraction: number
  readonly pair: number
}

export interface SplitPanesSource {
  /** The layout as the host holds it now; `null` while it has none. */
  layout(): PaneLayout | null
  /**
   * Calls `onChange` after every change the host makes, as it is made — not a
   * render later — and returns what stops it. A drag compares what it read at
   * the press on each call, so a change ends it at once.
   */
  subscribe(onChange: () => void): () => void
  /**
   * Values of the host's, compared by identity, whose change ends a press or
   * a drag (beside the panes' arrangement, which the drag watches itself):
   * anything a drag reads once that the host can change, such as what fills
   * its content region or which side columns are open.
   */
  watched(): readonly unknown[]
  /** Whether the host still has `item`: a carried item gone ends the drag. */
  holds(item: string): boolean
  /** Whether panes can be aimed at now; not while something covers them all. */
  targetable(): boolean
  /** The panes' room as the page lays it out now, or nothing with no grid on the page. */
  measure(): PaneRoom | undefined
  /** Applies a drop: what a drag shows as the outcome (`dropOutcome`) of the layout it read. */
  commitDrop(drop: Drop): void
  /** Moves an edge between panes. */
  resize(move: EdgeMove): void
  /** Evens every column and every pane in a column. */
  equalize(): void
  /** The panes' room changed: holds every pane to the readable size in the room the host measures now. */
  fit(): void
}
