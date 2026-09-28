/**
 * The pane layout's rules that need pixels: where each pane is drawn, whether
 * the panes are readable in the room they have — the one rule every change
 * of layout is held to (`arrange`), and the one a resized window is fitted
 * by (`fitted`) — and how far an edge between two may be dragged. Pixels
 * arrive as plain numbers a caller measured; nothing here reads the DOM.
 */
import {
  paneLimits,
  type Column,
  type Pane,
  type PaneKey,
  type PaneLayout,
} from "./pane-layout"

/** A pane's rectangle as fractions of the grid, and its place in it. */
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
  /** The part of the two sides' room the side before the edge takes, 0 to 1. */
  readonly leading: number
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
          leading: pane.share / (pane.share + column.panes[r + 1].share),
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
        leading: column.share / (column.share + columns[c + 1].share),
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
 * The room the panes have: the grid's size as laid out — never as a flight
 * of motion draws it mid-way — and `spare`, the width the host could give
 * the grid beside it if a change needs it (the workspace's sidebar, folding:
 * its width and gutter; zero when there is none to give). Measured by the
 * host's page and handed to every change of layout, whoever asks for it.
 */
export interface PaneRoom {
  readonly width: number
  readonly height: number
  readonly spare: number
}

/** Half a pixel either way is the same size. */
const slack = 0.5

/** What `count` items and the gutters between them need of `size` pixels, each at `min`. */
function roomFor(count: number, min: number): number {
  return count * min + (count - 1) * paneLimits.gutter
}

/** Whether `count` columns can each be readable in a grid `width` wide. */
export function columnsFit(count: number, width: number): boolean {
  return roomFor(count, paneLimits.minWidth) <= width + slack
}

/**
 * Shares of `size` pixels (gutters between them taken out first) that keep
 * each at least `min`: the same shares when they do already; otherwise the
 * ones below it raised to it, the room they take coming from the others in
 * proportion. `null` when even at the minimum they cannot all fit.
 */
function sharesFitted(
  shares: readonly number[],
  size: number,
  min: number,
): readonly number[] | null {
  if (roomFor(shares.length, min) > size + slack) return null
  const available = size - (shares.length - 1) * paneLimits.gutter
  const total = shares.reduce((sum, share) => sum + share, 0)
  if (shares.every((share) => (available * share) / total >= min - slack)) return shares
  const raised = new Set<number>()
  for (;;) {
    const rest = available - raised.size * min
    const free = shares.flatMap((share, index) => (raised.has(index) ? [] : [share]))
    const freeTotal = free.reduce((sum, share) => sum + share, 0)
    const below = shares.flatMap((share, index) =>
      !raised.has(index) && (rest * share) / freeTotal < min ? [index] : [],
    )
    if (below.length === 0)
      return shares.map((share, index) =>
        raised.has(index) ? min / available : (rest * share) / freeTotal / available,
      )
    below.forEach((index) => raised.add(index))
  }
}

/**
 * The layout with every column at least the readable width and every pane
 * at least the readable height in `room`, rebalancing shares where it must:
 * the same layout when it fits already, `null` when it cannot.
 */
export function fitted(
  layout: PaneLayout,
  room: { readonly width: number; readonly height: number },
): PaneLayout | null {
  const widths = sharesFitted(
    layout.columns.map((column) => column.share),
    room.width,
    paneLimits.minWidth,
  )
  if (!widths) return null
  let changed = false
  const columns: Column[] = []
  for (const [index, column] of layout.columns.entries()) {
    const heights = sharesFitted(
      column.panes.map((pane) => pane.share),
      room.height,
      paneLimits.minHeight,
    )
    if (!heights) return null
    const share = widths[index]
    const panesChanged = column.panes.some((pane, row) => heights[row] !== pane.share)
    if (share === column.share && !panesChanged) {
      columns.push(column)
      continue
    }
    changed = true
    columns.push({
      ...column,
      share,
      panes: panesChanged
        ? column.panes.map((pane, row) => ({ ...pane, share: heights[row] }))
        : column.panes,
    })
  }
  return changed ? { ...layout, columns } : layout
}

/** Whether every pane of `layout` is readable in `room` as it stands. */
export function fits(
  layout: PaneLayout,
  room: { readonly width: number; readonly height: number },
): boolean {
  return fitted(layout, room) === layout
}

/** Whether two layouts have the same columns of the same number of panes: a swap. */
function sameGrid(a: PaneLayout, b: PaneLayout): boolean {
  return (
    a.columns.length === b.columns.length &&
    a.columns.every(
      (column, index) => column.panes.length === b.columns[index].panes.length,
    )
  )
}

/** A change of layout the room allows, and whether it takes the host's spare room to make it. */
export interface Arranged {
  readonly layout: PaneLayout
  readonly takesSpare: boolean
}

/**
 * The one rule every command that changes the layout goes through — split,
 * open beside, drop, move, nudge — whoever dispatches it: `candidate` is
 * taken if it leaves every pane readable in the room, rebalanced where it
 * must be; else if it does in the room and its spare together; else not at all. A
 * change that only swaps panes keeps every size, and is always taken. With
 * no measurement there is no room to speak of, and nothing new is placed.
 */
export function arrange(
  before: PaneLayout,
  candidate: PaneLayout,
  room: PaneRoom | undefined,
): Arranged | null {
  if (candidate === before) return null
  if (sameGrid(before, candidate)) return { layout: candidate, takesSpare: false }
  if (!room) return null
  const here = fitted(candidate, room)
  if (here) return { layout: here, takesSpare: false }
  if (room.spare <= 0) return null
  const widened = fitted(candidate, {
    width: room.width + room.spare,
    height: room.height,
  })
  return widened ? { layout: widened, takesSpare: true } : null
}

/** The two sides of an edge, in pixels along its axis, as the room lays them out. */
export function edgeSides(
  layout: PaneLayout,
  edge: PaneEdge,
  room: { readonly width: number; readonly height: number },
): { before: number; after: number } | null {
  const sizes = (shares: readonly number[], size: number) => {
    const available = size - (shares.length - 1) * paneLimits.gutter
    const total = shares.reduce((sum, share) => sum + share, 0)
    return shares.map((share) => (available * share) / total)
  }
  if (edge.axis === "x") {
    const widths = sizes(
      layout.columns.map((column) => column.share),
      room.width,
    )
    const [before, after] = [widths[edge.column], widths[edge.column + 1]]
    return before === undefined || after === undefined ? null : { before, after }
  }
  const column = layout.columns[edge.column]
  if (!column) return null
  const heights = sizes(
    column.panes.map((pane) => pane.share),
    room.height,
  )
  const [before, after] = [heights[edge.row], heights[edge.row + 1]]
  return before === undefined || after === undefined ? null : { before, after }
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
