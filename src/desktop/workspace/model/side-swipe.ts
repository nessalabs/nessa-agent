/**
 * A two-finger sideways swipe across the chat folds or opens the window's side
 * columns, one column per swipe. From the left edge the columns stand rail,
 * sidebar, session list. A swipe left folds the open one nearest the chat; a
 * swipe right opens the closed one nearest the window's edge, so the columns
 * come back in the order they went: list, sidebar, rail away; rail, sidebar,
 * list back.
 *
 * Pure: the wheel events are read in `adapters/dom/side-swipe.ts`, and the
 * columns are toggled by the window (`use-workspace-window.ts`).
 */

/** Which way the fingers moved: left folds a column, right opens one. */
export type SwipeDirection = "left" | "right"

/** The side columns, from the window's left edge toward the chat. */
export const SIDE_COLUMNS = ["rail", "sidebar", "list"] as const
export type SideColumn = (typeof SIDE_COLUMNS)[number]

/**
 * Each column as a swipe sees it. `absent` is one this window cannot draw
 * now: the rail while its preview is off or the window has no room for it,
 * the session list in a layout without one.
 */
export type ColumnState = "open" | "closed" | "absent"
export type SwipeColumns = Readonly<Record<SideColumn, ColumnState>>

/** What the window draws now, which a swipe reads its columns from. */
export interface DrawnColumns {
  /** The rail's preview is on (Settings › Advanced › Experimental). */
  readonly railOffered: boolean
  readonly railDrawn: boolean
  /** The window has room for the rail beside what it holds open. */
  readonly railRoom: boolean
  readonly sidebarOpen: boolean
  /** The layout has a session list at all. */
  readonly listOffered: boolean
  readonly listOpen: boolean
}

/** Each column as a swipe sees it: one the window cannot draw now is absent. */
export function swipeColumnsOf(drawn: DrawnColumns): SwipeColumns {
  return {
    rail: !drawn.railOffered
      ? "absent"
      : drawn.railDrawn
        ? "open"
        : drawn.railRoom
          ? "closed"
          : "absent",
    sidebar: drawn.sidebarOpen ? "open" : "closed",
    list: !drawn.listOffered ? "absent" : drawn.listOpen ? "open" : "closed",
  }
}

/**
 * Whether the chat reads a swipe: not in a browser tab, whose own swipe goes
 * back a page, and not while something covers the panes (an item's full
 * view, the expanded right panel).
 */
export function swipeReads(window: {
  readonly browserSurface: boolean
  readonly fullView: boolean
  readonly panelMaximized: boolean
}): boolean {
  return !window.browserSurface && !window.fullView && !window.panelMaximized
}

/** The column a swipe toggles, or null when every column is already that way. */
export function columnToToggle(
  columns: SwipeColumns,
  direction: SwipeDirection,
): SideColumn | null {
  if (direction === "left")
    return [...SIDE_COLUMNS].reverse().find((each) => columns[each] === "open") ?? null
  return SIDE_COLUMNS.find((each) => columns[each] === "closed") ?? null
}

/** Horizontal travel, in CSS pixels, before a swipe counts. */
export const SWIPE_DISTANCE = 48
/** A swipe is sideways only while its horizontal travel is this many times its vertical. */
export const SWIPE_RATIO = 2
/** Vertical travel past which a gesture is a scroll, never a swipe. */
export const SCROLL_LOCK = 12
/**
 * A pause between wheel events, in milliseconds, longer than this ends the
 * gesture. The trackpad's momentum keeps sending events after the fingers
 * lift, so a fling is still the one gesture: it toggles once.
 */
export const GESTURE_GAP = 180

/** One wheel event, in pixels, with x positive when the fingers moved left. */
export interface SwipeSample {
  readonly at: number
  readonly x: number
  readonly y: number
}

/**
 * Where a gesture is. `tracking` adds up its travel; `scroll` is a gesture
 * that went vertical first, `skipped` one that began over something that
 * scrolls sideways itself (a wide table, a code block) or was a pinch;
 * `done` one that already toggled. None of those three toggles again.
 */
export type SwipeState =
  | { readonly phase: "idle" }
  | {
      readonly phase: "tracking"
      readonly last: number
      readonly x: number
      readonly y: number
    }
  | { readonly phase: "scroll" | "skipped" | "done"; readonly last: number }

export const idleSwipe: SwipeState = { phase: "idle" }

/** Whether a wheel event at `at` begins a new gesture rather than continuing this one. */
export function beginsGesture(state: SwipeState, at: number): boolean {
  return state.phase === "idle" || at - state.last > GESTURE_GAP
}

/**
 * The gesture after one more wheel event. `skip` says the event is one a
 * swipe must leave alone; it is read only for an event that begins a gesture
 * (`beginsGesture`), so the whole gesture follows where it began.
 */
export function swipeStep(
  state: SwipeState,
  sample: SwipeSample,
  skip: boolean,
): { state: SwipeState; swiped: SwipeDirection | null } {
  if (state.phase === "idle" || sample.at - state.last > GESTURE_GAP) {
    if (skip) return { state: { phase: "skipped", last: sample.at }, swiped: null }
    return measure({ phase: "tracking", last: sample.at, x: 0, y: 0 }, sample)
  }
  if (state.phase === "tracking") return measure(state, sample)
  return { state: { phase: state.phase, last: sample.at }, swiped: null }
}

function measure(
  state: Extract<SwipeState, { phase: "tracking" }>,
  sample: SwipeSample,
): { state: SwipeState; swiped: SwipeDirection | null } {
  const x = state.x + sample.x
  const y = state.y + sample.y
  if (Math.abs(y) >= SCROLL_LOCK && Math.abs(y) >= Math.abs(x))
    return { state: { phase: "scroll", last: sample.at }, swiped: null }
  if (Math.abs(x) >= SWIPE_DISTANCE && Math.abs(x) >= Math.abs(y) * SWIPE_RATIO)
    return {
      state: { phase: "done", last: sample.at },
      swiped: x > 0 ? "left" : "right",
    }
  return { state: { phase: "tracking", last: sample.at, x, y }, swiped: null }
}
