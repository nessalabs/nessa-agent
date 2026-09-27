/**
 * The pane layout's rules that need pixels: where each pane is drawn, whether
 * a pane still fits beside another, and how far an edge between two may be
 * dragged. Pixels arrive as plain numbers a caller measured; nothing here
 * reads the DOM.
 */
import {
  isFull,
  isHorizontal,
  locate,
  paneLimits,
  removePane,
  type Column,
  type Pane,
  type PaneKey,
  type PaneLayout,
  type Side,
} from "./pane-layout"

/** A pane's rectangle as fractions of the workspace, and its place in the grid. */
export interface PanePlacement {
  readonly key: PaneKey
  readonly x: number
  readonly width: number
  readonly column: number
  readonly columns: number
  readonly y: number
  readonly height: number
  readonly row: number
  readonly rows: number
  /** The top-left pane, which clears the window's controls when nothing is beside it. */
  readonly corner: boolean
}

/** A draggable boundary between two columns, or two panes in a column. */
export type PaneEdge =
  | { readonly axis: "x"; readonly column: number }
  | { readonly axis: "y"; readonly column: number; readonly row: number }

export interface EdgePlacement {
  readonly id: string
  readonly edge: PaneEdge
  /** A pane on each side of the edge, whose sizes the drag measures. */
  readonly before: PaneKey
  readonly after: PaneKey
  readonly x: number
  readonly width: number
  readonly column: number
  readonly columns: number
  readonly y: number
  readonly row: number
  readonly rows: number
}

const total = (items: readonly { share: number }[]) =>
  items.reduce((sum, item) => sum + item.share, 0)

/**
 * Where every pane and edge is drawn, as fractions the stylesheet turns into
 * boxes. Only the columns decide it: which pane is focused does not move any.
 */
export function placements(columns: PaneLayout["columns"]): {
  panes: PanePlacement[]
  edges: EdgePlacement[]
} {
  const width = total(columns)
  const panes: PanePlacement[] = []
  const edges: EdgePlacement[] = []
  let x = 0
  columns.forEach((column: Column, c) => {
    const w = column.share / width
    const height = total(column.panes)
    let y = 0
    column.panes.forEach((pane: Pane, r) => {
      const h = pane.share / height
      panes.push({
        key: pane.key,
        x,
        width: w,
        column: c,
        columns: columns.length,
        y,
        height: h,
        row: r,
        rows: column.panes.length,
        corner: c === 0 && r === 0,
      })
      y += h
      if (r < column.panes.length - 1)
        edges.push({
          id: `row-${column.key}-${pane.key}`,
          edge: { axis: "y", column: c, row: r },
          before: pane.key,
          after: column.panes[r + 1].key,
          x,
          width: w,
          column: c,
          columns: columns.length,
          y,
          row: r,
          rows: column.panes.length,
        })
    })
    x += w
    if (c < columns.length - 1)
      edges.push({
        id: `column-${column.key}`,
        edge: { axis: "x", column: c },
        before: column.panes[0].key,
        after: columns[c + 1].panes[0].key,
        x,
        width: 0,
        column: c,
        columns: columns.length,
        y: 0,
        row: 0,
        rows: 1,
      })
  })
  return { panes, edges }
}

/**
 * What a caller measured about the pane something would be placed beside:
 * its size, and the width the sidebar would give up if it stepped aside.
 */
export interface PaneRoom {
  readonly width: number
  readonly height: number
  /** The open sidebar's width and gutter; zero when it is already closed. */
  readonly spare: number
}

/**
 * Whether a pane could go on `side` of `target`: within the limits, a column
 * needs room for a readable width (the sidebar stepping aside if that is what
 * it takes), a row needs half the target's height to stay readable. `moving`
 * is a pane already in the layout being moved there. Without a measurement
 * only the limits apply.
 */
export function canPlace(
  layout: PaneLayout,
  side: Side,
  target: PaneKey,
  room?: PaneRoom,
  moving?: PaneKey,
): boolean {
  if (!locate(layout, target)) return false
  const base = moving === undefined ? layout : removePane(layout, moving)
  if (moving === undefined && isFull(layout)) return false
  if (isHorizontal(side)) {
    if (base.columns.length >= paneLimits.maxColumns) return false
    return !room || (room.width + room.spare) / 2 >= paneLimits.minWidth
  }
  return !room || room.height / 2 >= paneLimits.minHeight
}

/** Whether a new column only fits once the sidebar steps aside. */
export function needsSidebarRoom(side: Side, room?: PaneRoom): boolean {
  return isHorizontal(side) && room !== undefined && room.width / 2 < paneLimits.minWidth
}

/**
 * Where a split asked for `side` lands: that side when it fits, else the
 * other axis (a column that will not fit stacks instead), else nowhere.
 */
export function placeBeside(
  layout: PaneLayout,
  target: PaneKey,
  side: Side,
  room?: PaneRoom,
): Side | null {
  if (canPlace(layout, side, target, room)) return side
  const other: Side = isHorizontal(side) ? "bottom" : "right"
  return canPlace(layout, other, target, room) ? other : null
}

/**
 * Moves the edge `edge` so the pane (or column) before it takes `fraction` of
 * the two sides' room, `pair` pixels, held so neither side drops below the
 * readable minimum. Shares elsewhere are untouched.
 */
export function resizeEdge(
  layout: PaneLayout,
  edge: PaneEdge,
  fraction: number,
  pair: number,
): PaneLayout {
  const min = edge.axis === "x" ? paneLimits.minWidth : paneLimits.minHeight
  const lowest = pair > 0 ? Math.min(0.5, min / pair) : 0.5
  const leading = Math.min(Math.max(fraction, lowest), 1 - lowest)
  if (edge.axis === "x") {
    const first = layout.columns[edge.column]
    const second = layout.columns[edge.column + 1]
    if (!first || !second) return layout
    const both = first.share + second.share
    return {
      ...layout,
      columns: layout.columns.map((column, i) =>
        i === edge.column
          ? { ...column, share: both * leading }
          : i === edge.column + 1
            ? { ...column, share: both * (1 - leading) }
            : column,
      ),
    }
  }
  const column = layout.columns[edge.column]
  const first = column?.panes[edge.row]
  const second = column?.panes[edge.row + 1]
  if (!column || !first || !second) return layout
  const both = first.share + second.share
  return {
    ...layout,
    columns: layout.columns.map((other, i) =>
      i !== edge.column
        ? other
        : {
            ...other,
            panes: other.panes.map((pane, j) =>
              j === edge.row
                ? { ...pane, share: both * leading }
                : j === edge.row + 1
                  ? { ...pane, share: both * (1 - leading) }
                  : pane,
            ),
          },
    ),
  }
}
