/**
 * The workspace's side columns and the window's width: how wide the sidebar
 * and the session list are drawn, and which of them fold away when the window
 * is too narrow for the panes beside them. The stylesheet draws the widths
 * this module computes; it does not clamp them again.
 */
import { paneLimits } from "./pane-layout"
import { columnsFit } from "./pane-sizing"

export interface ColumnLimits {
  readonly min: number
  readonly max: number
  /** The most of the window the column takes, however wide it was asked to be. */
  readonly windowShare: number
}

/** The sidebar beside a session list: a list of channels, so it can be slim. */
export const sidebarLimits: ColumnLimits = { min: 200, max: 320, windowShare: 0.2 }
/**
 * The sidebar that lists sessions under their channels: their titles need
 * the room. The same sidebar, so the same limits a drag is held to.
 */
export const treeSidebarLimits: ColumnLimits = { ...sidebarLimits, windowShare: 0.26 }
export const sessionListLimits: ColumnLimits = { min: 260, max: 420, windowShare: 0.26 }

/** Holds a width someone dragged to within a column's limits. */
export function clampColumn(width: number, limits: ColumnLimits): number {
  return Math.min(Math.max(width, limits.min), limits.max)
}

/** The width a column is drawn at in a window `windowWidth` wide. */
export function columnWidth(
  asked: number,
  limits: ColumnLimits,
  windowWidth: number,
): number {
  return Math.min(
    clampColumn(asked, limits),
    Math.max(limits.min, windowWidth * limits.windowShare),
  )
}

export interface SideColumns {
  readonly sidebarOpen: boolean
  readonly sessionListOpen: boolean
  /** The sidebar's drawn width, in pixels. */
  readonly sidebarWidth: number
  /** The session list's drawn width; zero where the layout has no list. */
  readonly sessionListWidth: number
}

/**
 * Which side columns a window `windowWidth` wide can draw beside `columns`
 * columns of panes, each at the readable width (`columnsFit`, the same rule
 * the panes are fitted by): the sidebar folds first, then the session list,
 * as Mail does. Returns the same value when nothing folds.
 */
export function foldToFit(
  side: SideColumns,
  windowWidth: number,
  columns: number,
): SideColumns {
  const gutter = paneLimits.gutter
  const outside = 2 * gutter
  const sidebar = side.sidebarOpen ? side.sidebarWidth + gutter : 0
  const list = side.sessionListOpen ? side.sessionListWidth + gutter : 0
  if (columnsFit(columns, windowWidth - outside - sidebar - list)) return side
  const sessionListOpen =
    side.sessionListOpen && columnsFit(columns, windowWidth - outside - list)
  if (!side.sidebarOpen && sessionListOpen === side.sessionListOpen) return side
  return { ...side, sidebarOpen: false, sessionListOpen }
}
